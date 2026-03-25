use anyhow::{Context, Result, bail};
use bollard::{
    Docker,
    container::{
        InspectContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
        RemoveContainerOptions, RestartContainerOptions, StartContainerOptions,
        StopContainerOptions,
    },
    exec::{CreateExecOptions, StartExecResults},
    image::ListImagesOptions,
    models::{ContainerSummary, ImageSummary},
};
use futures::StreamExt;
use std::path::Path;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub state: String,
    pub compose_project: Option<String>,
    pub cpu_percent: f64,
    pub mem_usage: u64,
    pub mem_limit: u64,
}

#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: u64,
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendKind {
    Docker,
    Podman,
}

pub struct DockerBackend {
    pub client: Docker,
    pub kind: BackendKind,
}

impl DockerBackend {
    pub fn connect(host: Option<&str>, force_podman: bool) -> Result<Self> {
        if let Some(h) = host {
            let client = Docker::connect_with_socket(h, 120, bollard::API_DEFAULT_VERSION)
                .context("failed to connect to specified socket")?;
            let kind = if force_podman || h.contains("podman") {
                BackendKind::Podman
            } else {
                BackendKind::Docker
            };
            return Ok(Self { client, kind });
        }

        // Auto-detect: try Podman first (rootless), then Docker
        let podman_paths = podman_socket_paths();
        for path in &podman_paths {
            if Path::new(path).exists() {
                let client =
                    Docker::connect_with_socket(path, 120, bollard::API_DEFAULT_VERSION)
                        .context("failed to connect to podman socket")?;
                return Ok(Self {
                    client,
                    kind: BackendKind::Podman,
                });
            }
        }

        // Fall back to Docker default socket
        let client = Docker::connect_with_socket_defaults()
            .context("failed to connect to docker socket")?;
        Ok(Self {
            client,
            kind: BackendKind::Docker,
        })
    }

    pub async fn list_containers(&self) -> Result<Vec<ContainerInfo>> {
        let opts = ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        };
        let raw: Vec<ContainerSummary> = self
            .client
            .list_containers(Some(opts))
            .await
            .context("list containers")?;

        Ok(raw.into_iter().map(container_info_from_summary).collect())
    }

    pub async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let opts = ListImagesOptions::<String> {
            all: false,
            ..Default::default()
        };
        let raw: Vec<ImageSummary> = self
            .client
            .list_images(Some(opts))
            .await
            .context("list images")?;

        Ok(raw.into_iter().map(image_info_from_summary).collect())
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        self.client
            .start_container(id, None::<StartContainerOptions<String>>)
            .await
            .context("start container")
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        self.client
            .stop_container(id, None::<StopContainerOptions>)
            .await
            .context("stop container")
    }

    pub async fn restart(&self, id: &str) -> Result<()> {
        self.client
            .restart_container(id, None::<RestartContainerOptions>)
            .await
            .context("restart container")
    }

    pub async fn remove(&self, id: &str, force: bool) -> Result<()> {
        let opts = RemoveContainerOptions {
            force,
            ..Default::default()
        };
        self.client
            .remove_container(id, Some(opts))
            .await
            .context("remove container")
    }

    pub async fn stream_logs(&self, id: &str, tx: mpsc::Sender<String>) -> Result<()> {
        let opts = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: true,
            tail: "200".into(),
            ..Default::default()
        };
        let mut stream = self.client.logs(id, Some(opts));
        while let Some(item) = stream.next().await {
            match item {
                Ok(LogOutput::StdOut { message } | LogOutput::StdErr { message }) => {
                    let line = String::from_utf8_lossy(&message).into_owned();
                    if tx.send(line).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
                _ => {}
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn exec_shell(&self, id: &str) -> Result<()> {
        // Detect available shell in container
        let inspect = self
            .client
            .inspect_container(id, None::<InspectContainerOptions>)
            .await?;

        let os = inspect
            .platform
            .as_deref()
            .unwrap_or("linux");

        let shell = if os == "windows" { "cmd" } else { "sh" };

        let exec = self
            .client
            .create_exec(
                id,
                CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    cmd: Some(vec![shell]),
                    ..Default::default()
                },
            )
            .await?;

        match self.client.start_exec(&exec.id, None).await? {
            StartExecResults::Attached { .. } => {}
            StartExecResults::Detached => bail!("exec started in detached mode unexpectedly"),
        }

        Ok(())
    }

    pub async fn fetch_stats(&self, id: &str) -> Result<(f64, u64, u64)> {
        use bollard::container::StatsOptions;
        let opts = StatsOptions { stream: false, one_shot: true };
        let mut stream = self.client.stats(id, Some(opts));
        if let Some(Ok(stats)) = stream.next().await {
            let cpu = calculate_cpu_percent(&stats);
            let (mem_usage, mem_limit) = memory_stats(&stats);
            Ok((cpu, mem_usage, mem_limit))
        } else {
            Ok((0.0, 0, 0))
        }
    }

    pub async fn list_contexts(&self) -> Vec<String> {
        // Docker contexts live in ~/.docker/contexts — parse names from filesystem
        let home = std::env::var("HOME").unwrap_or_default();
        let ctx_dir = format!("{home}/.docker/contexts/meta");
        let mut names = vec!["default".to_string()];
        if let Ok(entries) = std::fs::read_dir(&ctx_dir) {
            for entry in entries.flatten() {
                let meta = entry.path().join("meta.json");
                if let Ok(raw) = std::fs::read_to_string(meta) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(name) = v["Name"].as_str() {
                            if name != "default" {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        names
    }
}

fn podman_socket_paths() -> Vec<String> {
    let uid = unsafe { libc::getuid() };
    vec![
        format!("/run/user/{uid}/podman/podman.sock"),
        "/run/podman/podman.sock".to_string(),
        format!("/tmp/podman-run-{uid}/podman/podman.sock"),
    ]
}

fn container_info_from_summary(s: ContainerSummary) -> ContainerInfo {
    let id = s.id.unwrap_or_default();
    let name = s
        .names
        .and_then(|n| n.into_iter().next())
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = s.image.unwrap_or_default();
    let status = s.status.unwrap_or_default();
    let state = s.state.unwrap_or_default();
    let compose_project = s
        .labels
        .as_ref()
        .and_then(|l| l.get("com.docker.compose.project").cloned());

    ContainerInfo {
        id,
        name,
        image,
        status,
        state,
        compose_project,
        cpu_percent: 0.0,
        mem_usage: 0,
        mem_limit: 0,
    }
}

fn image_info_from_summary(s: ImageSummary) -> ImageInfo {
    let id = s.id;
    let repo_tags = s.repo_tags;
    let size = s.size as u64;
    let created = s.created as i64;
    ImageInfo { id, repo_tags, size, created }
}

fn calculate_cpu_percent(stats: &bollard::container::Stats) -> f64 {
    let cpu = &stats.cpu_stats;
    let pre = &stats.precpu_stats;

    let cpu_delta = cpu.cpu_usage.total_usage.saturating_sub(pre.cpu_usage.total_usage);
    let system_delta = cpu
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(pre.system_cpu_usage.unwrap_or(0));
    let num_cpus = cpu.online_cpus.unwrap_or(1) as f64;

    if system_delta > 0 {
        (cpu_delta as f64 / system_delta as f64) * num_cpus * 100.0
    } else {
        0.0
    }
}

fn memory_stats(stats: &bollard::container::Stats) -> (u64, u64) {
    let mem = &stats.memory_stats;
    let usage = mem.usage.unwrap_or(0);
    let limit = mem.limit.unwrap_or(0);
    (usage, limit)
}
