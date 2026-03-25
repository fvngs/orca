use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::docker::{ContainerInfo, DockerBackend, ImageInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Containers,
    Logs,
    Images,
    Contexts,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Confirm { message: String, action: ConfirmAction },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    Remove(String),
}

pub enum AppMessage {
    ContainersRefreshed(Vec<ContainerInfo>),
    ImagesRefreshed(Vec<ImageInfo>),
    ContextsRefreshed(Vec<String>),
    StatsUpdated { id: String, cpu: f64, mem: u64, mem_limit: u64 },
    LogLine(String),
    Error(String),
}

pub struct App {
    pub backend: Arc<DockerBackend>,
    pub view: View,

    pub containers: Vec<ContainerInfo>,
    pub container_selected: usize,

    pub images: Vec<ImageInfo>,
    pub image_selected: usize,

    pub log_lines: Vec<String>,
    pub log_scroll: usize,
    pub log_follow: bool,

    pub contexts: Vec<String>,
    pub context_selected: usize,

    pub status_message: Option<String>,
    pub pending_action: Action,

    pub tx: mpsc::Sender<AppMessage>,
    rx: mpsc::Receiver<AppMessage>,

    tasks: Vec<JoinHandle<()>>,
}

impl App {
    pub fn new(backend: DockerBackend) -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            backend: Arc::new(backend),
            view: View::Containers,
            containers: vec![],
            container_selected: 0,
            images: vec![],
            image_selected: 0,
            log_lines: vec![],
            log_scroll: 0,
            log_follow: true,
            contexts: vec![],
            context_selected: 0,
            status_message: None,
            pending_action: Action::None,
            tx,
            rx,
            tasks: vec![],
        }
    }

    pub fn start_background_tasks(&mut self) {
        let backend = Arc::clone(&self.backend);
        let tx = self.tx.clone();

        // Seed contexts once at startup
        let ctx_backend = Arc::clone(&self.backend);
        let ctx_tx = self.tx.clone();
        tokio::spawn(async move {
            let ctxs = ctx_backend.list_contexts().await;
            let _ = ctx_tx.send(AppMessage::ContextsRefreshed(ctxs)).await;
        });

        // Periodically refresh containers + images
        let handle = tokio::spawn(async move {
            loop {
                match backend.list_containers().await {
                    Ok(containers) => {
                        let _ = tx.send(AppMessage::ContainersRefreshed(containers.clone())).await;
                        for c in containers.iter().filter(|c| c.state == "running") {
                            let b = Arc::clone(&backend);
                            let id = c.id.clone();
                            let t = tx.clone();
                            tokio::spawn(async move {
                                if let Ok((cpu, mem, mem_limit)) = b.fetch_stats(&id).await {
                                    let _ = t.send(AppMessage::StatsUpdated { id, cpu, mem, mem_limit }).await;
                                }
                            });
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AppMessage::Error(e.to_string())).await;
                    }
                }

                match backend.list_images().await {
                    Ok(images) => {
                        let _ = tx.send(AppMessage::ImagesRefreshed(images)).await;
                    }
                    Err(e) => {
                        let _ = tx.send(AppMessage::Error(e.to_string())).await;
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        });
        self.tasks.push(handle);
    }

    /// Drain channel messages into app state. Call once per render tick.
    pub fn process_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMessage::ContainersRefreshed(list) => {
                    let selected_id = self
                        .containers
                        .get(self.container_selected)
                        .map(|c| c.id.clone());
                    self.containers = list;
                    if let Some(id) = selected_id {
                        if let Some(pos) = self.containers.iter().position(|c| c.id == id) {
                            self.container_selected = pos;
                        }
                    }
                    self.container_selected =
                        self.container_selected.min(self.containers.len().saturating_sub(1));
                }
                AppMessage::ImagesRefreshed(list) => {
                    self.images = list;
                    self.image_selected =
                        self.image_selected.min(self.images.len().saturating_sub(1));
                }
                AppMessage::ContextsRefreshed(list) => {
                    self.contexts = list;
                    self.context_selected =
                        self.context_selected.min(self.contexts.len().saturating_sub(1));
                }
                AppMessage::StatsUpdated { id, cpu, mem, mem_limit } => {
                    if let Some(c) = self.containers.iter_mut().find(|c| c.id == id) {
                        c.cpu_percent = cpu;
                        c.mem_usage = mem;
                        c.mem_limit = mem_limit;
                    }
                }
                AppMessage::LogLine(line) => {
                    self.log_lines.push(line);
                    if self.log_lines.len() > 5000 {
                        self.log_lines.drain(0..1000);
                    }
                    if self.log_follow {
                        self.log_scroll = self.log_lines.len().saturating_sub(1);
                    }
                }
                AppMessage::Error(e) => {
                    self.status_message = Some(format!("error: {e}"));
                }
            }
        }
    }

    pub fn selected_container(&self) -> Option<&ContainerInfo> {
        self.containers.get(self.container_selected)
    }

    pub fn scroll_up(&mut self) {
        match self.view {
            View::Containers => {
                self.container_selected = self.container_selected.saturating_sub(1);
            }
            View::Logs => {
                self.log_follow = false;
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            View::Images => {
                self.image_selected = self.image_selected.saturating_sub(1);
            }
            View::Contexts => {
                self.context_selected = self.context_selected.saturating_sub(1);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        match self.view {
            View::Containers => {
                if self.container_selected + 1 < self.containers.len() {
                    self.container_selected += 1;
                }
            }
            View::Logs => {
                let max = self.log_lines.len().saturating_sub(1);
                if self.log_scroll < max {
                    self.log_scroll += 1;
                } else {
                    self.log_follow = true;
                }
            }
            View::Images => {
                if self.image_selected + 1 < self.images.len() {
                    self.image_selected += 1;
                }
            }
            View::Contexts => {
                if self.context_selected + 1 < self.contexts.len() {
                    self.context_selected += 1;
                }
            }
        }
    }

    pub fn open_logs(&mut self) {
        if let Some(c) = self.selected_container() {
            let id = c.id.clone();
            self.view = View::Logs;
            self.log_lines.clear();
            self.log_scroll = 0;
            self.log_follow = true;

            let backend = Arc::clone(&self.backend);
            let tx = self.tx.clone();
            let handle = tokio::spawn(async move {
                let (log_tx, mut log_rx) = mpsc::channel::<String>(256);
                tokio::spawn(async move {
                    while let Some(line) = log_rx.recv().await {
                        if tx.send(AppMessage::LogLine(line)).await.is_err() {
                            break;
                        }
                    }
                });
                let _ = backend.stream_logs(&id, log_tx).await;
            });
            self.tasks.push(handle);
        }
    }

    pub async fn shutdown(&mut self) {
        for handle in self.tasks.drain(..) {
            handle.abort();
        }
    }
}
