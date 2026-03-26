use std::collections::{HashMap, HashSet, VecDeque};
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
    RemoveMultiple(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContainerSort {
    ByName,
    ByCpu,
    ByMem,
    ByStatus,
}

impl ContainerSort {
    pub fn next(&self) -> Self {
        match self {
            ContainerSort::ByName => ContainerSort::ByCpu,
            ContainerSort::ByCpu => ContainerSort::ByMem,
            ContainerSort::ByMem => ContainerSort::ByStatus,
            ContainerSort::ByStatus => ContainerSort::ByName,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ContainerSort::ByName => "name",
            ContainerSort::ByCpu => "cpu",
            ContainerSort::ByMem => "mem",
            ContainerSort::ByStatus => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogFilter {
    NoFilter,
    HideDebug,
    ErrorOnly,
}

impl LogFilter {
    pub fn next(&self) -> Self {
        match self {
            LogFilter::NoFilter => LogFilter::HideDebug,
            LogFilter::HideDebug => LogFilter::ErrorOnly,
            LogFilter::ErrorOnly => LogFilter::NoFilter,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            LogFilter::NoFilter => "none",
            LogFilter::HideDebug => "hide debug",
            LogFilter::ErrorOnly => "errors",
        }
    }
}

pub enum AppMessage {
    ContainersRefreshed(Vec<ContainerInfo>),
    ImagesRefreshed(Vec<ImageInfo>),
    ContextsRefreshed(Vec<String>),
    StatsUpdated { id: String, cpu: f64, mem: u64, mem_limit: u64 },
    LogLine(String),
    Error(String),
    InspectData(Vec<String>),
}

pub struct App {
    pub backend: Arc<DockerBackend>,
    pub view: View,

    pub containers: Vec<ContainerInfo>,
    pub container_selected: usize,
    pub container_sort: ContainerSort,
    pub wide_mode: bool,
    pub last_key: Option<crossterm::event::KeyCode>,

    // Multi-select
    pub container_selected_ids: HashSet<String>,

    // Filter (containers + images)
    pub filter_input: Option<String>,
    pub filter_active: bool,

    pub images: Vec<ImageInfo>,
    pub image_selected: usize,

    pub log_lines: Vec<String>,
    pub log_scroll: usize,
    pub log_follow: bool,
    pub log_filter: LogFilter,

    // Log search
    pub log_search: Option<String>,
    pub log_search_active: bool,
    pub log_search_matches: Vec<usize>,
    pub log_search_idx: usize,

    pub contexts: Vec<String>,
    pub context_selected: usize,

    pub status_message: Option<String>,
    pub pending_action: Action,

    // Inspect overlay
    pub inspect_view: bool,
    pub inspect_data: Vec<String>,
    pub inspect_scroll: usize,
    pub inspect_container_name: String,

    // CPU/Mem sparkline history (keyed by container id)
    pub cpu_history: HashMap<String, VecDeque<f64>>,
    pub mem_history: HashMap<String, VecDeque<u64>>,

    // Custom exec shell input
    pub exec_shell_input: Option<String>,
    pub exec_shell_active: bool,

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
            container_sort: ContainerSort::ByName,
            wide_mode: false,
            last_key: None,
            container_selected_ids: HashSet::new(),
            filter_input: None,
            filter_active: false,
            images: vec![],
            image_selected: 0,
            log_lines: vec![],
            log_scroll: 0,
            log_follow: true,
            log_filter: LogFilter::NoFilter,
            log_search: None,
            log_search_active: false,
            log_search_matches: vec![],
            log_search_idx: 0,
            contexts: vec![],
            context_selected: 0,
            status_message: None,
            pending_action: Action::None,
            inspect_view: false,
            inspect_data: vec![],
            inspect_scroll: 0,
            inspect_container_name: String::new(),
            cpu_history: HashMap::new(),
            mem_history: HashMap::new(),
            exec_shell_input: None,
            exec_shell_active: false,
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
                    // Update sparkline history
                    self.cpu_history
                        .entry(id.clone())
                        .or_default()
                        .push_back(cpu);
                    if self.cpu_history[&id].len() > 60 {
                        self.cpu_history.get_mut(&id).unwrap().pop_front();
                    }
                    self.mem_history
                        .entry(id.clone())
                        .or_default()
                        .push_back(mem);
                    if self.mem_history[&id].len() > 60 {
                        self.mem_history.get_mut(&id).unwrap().pop_front();
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
                    // Update search matches if search active
                    if let Some(query) = &self.log_search.clone() {
                        if !query.is_empty() {
                            let idx = self.log_lines.len() - 1;
                            if self.log_lines[idx].to_lowercase().contains(&query.to_lowercase()) {
                                self.log_search_matches.push(idx);
                            }
                        }
                    }
                }
                AppMessage::Error(e) => {
                    self.status_message = Some(format!("error: {e}"));
                }
                AppMessage::InspectData(lines) => {
                    self.inspect_data = lines;
                    self.inspect_scroll = 0;
                    self.inspect_view = true;
                }
            }
        }
    }

    pub fn selected_container(&self) -> Option<&ContainerInfo> {
        self.containers.get(self.container_selected)
    }

    /// Returns filtered containers (by filter_input, case-insensitive name match)
    pub fn filtered_containers(&self) -> Vec<&ContainerInfo> {
        if let Some(filter) = &self.filter_input {
            if !filter.is_empty() {
                let lower = filter.to_lowercase();
                return self.containers.iter().filter(|c| c.name.to_lowercase().contains(&lower)).collect();
            }
        }
        self.containers.iter().collect()
    }

    /// Returns sorted+filtered containers
    pub fn sorted_filtered_containers(&self) -> Vec<&ContainerInfo> {
        let mut list = self.filtered_containers();
        match self.container_sort {
            ContainerSort::ByName => list.sort_by(|a, b| a.name.cmp(&b.name)),
            ContainerSort::ByCpu => list.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal)),
            ContainerSort::ByMem => list.sort_by(|a, b| b.mem_usage.cmp(&a.mem_usage)),
            ContainerSort::ByStatus => list.sort_by(|a, b| a.status.cmp(&b.status)),
        }
        list
    }

    /// Returns filtered images
    pub fn filtered_images(&self) -> Vec<&ImageInfo> {
        if let Some(filter) = &self.filter_input {
            if !filter.is_empty() {
                let lower = filter.to_lowercase();
                return self.images.iter().filter(|img| {
                    img.repo_tags.iter().any(|t| t.to_lowercase().contains(&lower))
                }).collect();
            }
        }
        self.images.iter().collect()
    }

    pub fn scroll_up(&mut self) {
        match self.view {
            View::Containers => {
                if self.inspect_view {
                    self.inspect_scroll = self.inspect_scroll.saturating_sub(1);
                } else {
                    let list = self.sorted_filtered_containers();
                    let len = list.len();
                    if len == 0 { return; }
                    // Find current position in filtered list
                    let cur_id = self.containers.get(self.container_selected).map(|c| c.id.clone());
                    let cur_pos = cur_id.as_ref().and_then(|id| list.iter().position(|c| &c.id == id)).unwrap_or(0);
                    if cur_pos > 0 {
                        let new_id = list[cur_pos - 1].id.clone();
                        if let Some(pos) = self.containers.iter().position(|c| c.id == new_id) {
                            self.container_selected = pos;
                        }
                    }
                }
            }
            View::Logs => {
                self.log_follow = false;
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            View::Images => {
                let list = self.filtered_images();
                let len = list.len();
                if len == 0 { return; }
                let cur_id = self.images.get(self.image_selected).map(|i| i.id.clone());
                let cur_pos = cur_id.as_ref().and_then(|id| list.iter().position(|i| &i.id == id)).unwrap_or(0);
                if cur_pos > 0 {
                    let new_id = list[cur_pos - 1].id.clone();
                    if let Some(pos) = self.images.iter().position(|i| i.id == new_id) {
                        self.image_selected = pos;
                    }
                }
            }
            View::Contexts => {
                self.context_selected = self.context_selected.saturating_sub(1);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        match self.view {
            View::Containers => {
                if self.inspect_view {
                    let max = self.inspect_data.len().saturating_sub(1);
                    if self.inspect_scroll < max {
                        self.inspect_scroll += 1;
                    }
                } else {
                    let list = self.sorted_filtered_containers();
                    let len = list.len();
                    if len == 0 { return; }
                    let cur_id = self.containers.get(self.container_selected).map(|c| c.id.clone());
                    let cur_pos = cur_id.as_ref().and_then(|id| list.iter().position(|c| &c.id == id)).unwrap_or(0);
                    if cur_pos + 1 < len {
                        let new_id = list[cur_pos + 1].id.clone();
                        if let Some(pos) = self.containers.iter().position(|c| c.id == new_id) {
                            self.container_selected = pos;
                        }
                    }
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
                let list = self.filtered_images();
                let len = list.len();
                if len == 0 { return; }
                let cur_id = self.images.get(self.image_selected).map(|i| i.id.clone());
                let cur_pos = cur_id.as_ref().and_then(|id| list.iter().position(|i| &i.id == id)).unwrap_or(0);
                if cur_pos + 1 < len {
                    let new_id = list[cur_pos + 1].id.clone();
                    if let Some(pos) = self.images.iter().position(|i| i.id == new_id) {
                        self.image_selected = pos;
                    }
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
            self.log_search = None;
            self.log_search_active = false;
            self.log_search_matches.clear();
            self.log_search_idx = 0;

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

    pub fn compute_log_search_matches(&mut self) {
        self.log_search_matches.clear();
        self.log_search_idx = 0;
        if let Some(query) = &self.log_search.clone() {
            if !query.is_empty() {
                let lower = query.to_lowercase();
                for (i, line) in self.log_lines.iter().enumerate() {
                    if line.to_lowercase().contains(&lower) {
                        self.log_search_matches.push(i);
                    }
                }
            }
        }
    }

    pub async fn shutdown(&mut self) {
        for handle in self.tasks.drain(..) {
            handle.abort();
        }
    }
}
