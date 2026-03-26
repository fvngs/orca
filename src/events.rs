use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use crate::app::{Action, App, AppMessage, ConfirmAction, View};
use crate::docker::BackendKind;

pub enum EventOutcome {
    Continue,
    Quit,
    ExecShell { container_id: String, shell: String },
}

pub async fn handle_events(app: &mut App) -> Result<EventOutcome> {
    app.process_messages();

    if !event::poll(Duration::from_millis(50))? {
        return Ok(EventOutcome::Continue);
    }

    if let Event::Key(key) = event::read()? {
        if key.kind != KeyEventKind::Press {
            return Ok(EventOutcome::Continue);
        }

        // Custom exec shell input mode
        if app.exec_shell_active {
            match key.code {
                KeyCode::Esc => {
                    app.exec_shell_active = false;
                    app.exec_shell_input = None;
                }
                KeyCode::Enter => {
                    let shell = app.exec_shell_input.take().unwrap_or_else(|| "sh".to_string());
                    app.exec_shell_active = false;
                    if let Some(c) = app.selected_container() {
                        if c.state == "running" {
                            let cid = c.id.clone();
                            return Ok(EventOutcome::ExecShell { container_id: cid, shell });
                        }
                    }
                }
                KeyCode::Backspace => {
                    if let Some(ref mut s) = app.exec_shell_input {
                        s.pop();
                    }
                }
                KeyCode::Char(c) => {
                    app.exec_shell_input.get_or_insert_with(String::new).push(c);
                }
                _ => {}
            }
            return Ok(EventOutcome::Continue);
        }

        // Filter input mode (containers or images)
        if app.filter_active {
            match key.code {
                KeyCode::Esc => {
                    app.filter_active = false;
                    app.filter_input = None;
                }
                KeyCode::Enter => {
                    app.filter_active = false;
                    // Keep filter_input set (filtering stays active)
                }
                KeyCode::Backspace => {
                    if let Some(ref mut s) = app.filter_input {
                        s.pop();
                        if s.is_empty() {
                            app.filter_input = None;
                        }
                    }
                }
                KeyCode::Char(c) => {
                    app.filter_input.get_or_insert_with(String::new).push(c);
                }
                _ => {}
            }
            return Ok(EventOutcome::Continue);
        }

        // Log search input mode
        if app.log_search_active {
            match key.code {
                KeyCode::Esc => {
                    app.log_search_active = false;
                    app.log_search = None;
                    app.log_search_matches.clear();
                }
                KeyCode::Enter => {
                    app.log_search_active = false;
                    app.compute_log_search_matches();
                    if !app.log_search_matches.is_empty() {
                        let idx = app.log_search_matches[app.log_search_idx];
                        app.log_scroll = idx;
                        app.log_follow = false;
                    }
                }
                KeyCode::Backspace => {
                    if let Some(ref mut s) = app.log_search {
                        s.pop();
                    }
                }
                KeyCode::Char(c) => {
                    app.log_search.get_or_insert_with(String::new).push(c);
                }
                _ => {}
            }
            return Ok(EventOutcome::Continue);
        }

        // Confirmation dialog takes priority
        if let Action::Confirm { action, .. } = app.pending_action.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.pending_action = Action::None;
                    execute_confirmed(app, action).await;
                }
                _ => {
                    app.pending_action = Action::None;
                    app.status_message = Some("cancelled".to_string());
                }
            }
            return Ok(EventOutcome::Continue);
        }

        // Global keybinds
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(EventOutcome::Quit);
        }

        match key.code {
            KeyCode::Char('q') => return Ok(EventOutcome::Quit),
            KeyCode::Tab => cycle_view(app),
            KeyCode::Char('1') => app.view = View::Containers,
            KeyCode::Char('2') => app.view = View::Logs,
            KeyCode::Char('3') => app.view = View::Images,
            KeyCode::Char('4') => app.view = View::Contexts,
            _ => {
                if let Some(outcome) = handle_view_keys(app, key.code).await {
                    return Ok(outcome);
                }
            }
        }
    }

    Ok(EventOutcome::Continue)
}

fn cycle_view(app: &mut App) {
    app.view = match app.view {
        View::Containers => View::Images,
        View::Images => View::Contexts,
        View::Contexts => View::Containers,
        View::Logs => View::Containers,
    };
}

/// Returns Some(outcome) only when special handling is needed (e.g. ExecShell).
async fn handle_view_keys(app: &mut App, code: KeyCode) -> Option<EventOutcome> {
    match app.view {
        View::Containers => handle_containers(app, code).await,
        View::Logs => {
            handle_logs(app, code);
            None
        }
        View::Images => {
            handle_images(app, code);
            None
        }
        View::Contexts => {
            handle_contexts(app, code);
            None
        }
    }
}

async fn handle_containers(app: &mut App, code: KeyCode) -> Option<EventOutcome> {
    // Inspect overlay navigation
    if app.inspect_view {
        match code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
            KeyCode::Esc => {
                app.inspect_view = false;
            }
            _ => {}
        }
        return None;
    }

    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.last_key = None;
            app.scroll_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.last_key = None;
            app.scroll_down();
        }
        KeyCode::Char('g') => {
            if app.last_key == Some(KeyCode::Char('g')) {
                // gg — jump to top
                let list = app.sorted_filtered_containers();
                if !list.is_empty() {
                    let new_id = list[0].id.clone();
                    if let Some(pos) = app.containers.iter().position(|c| c.id == new_id) {
                        app.container_selected = pos;
                    }
                }
                app.last_key = None;
            } else {
                app.last_key = Some(KeyCode::Char('g'));
            }
            return None;
        }
        KeyCode::Char('G') => {
            let list = app.sorted_filtered_containers();
            if !list.is_empty() {
                let last = list.len() - 1;
                let new_id = list[last].id.clone();
                if let Some(pos) = app.containers.iter().position(|c| c.id == new_id) {
                    app.container_selected = pos;
                }
            }
            app.last_key = None;
        }
        KeyCode::Char('/') => {
            app.filter_active = true;
            app.filter_input = Some(String::new());
            app.last_key = None;
        }
        KeyCode::Esc => {
            // Clear filter or multi-select
            if app.filter_input.is_some() {
                app.filter_input = None;
                app.filter_active = false;
            } else {
                app.container_selected_ids.clear();
            }
            app.last_key = None;
        }
        KeyCode::Char('S') => {
            app.container_sort = app.container_sort.next();
            app.last_key = None;
        }
        KeyCode::Char('W') => {
            app.wide_mode = !app.wide_mode;
            app.last_key = None;
        }
        KeyCode::Char(' ') => {
            if let Some(c) = app.selected_container() {
                let id = c.id.clone();
                if app.container_selected_ids.contains(&id) {
                    app.container_selected_ids.remove(&id);
                } else {
                    app.container_selected_ids.insert(id);
                }
            }
            app.last_key = None;
        }
        KeyCode::Enter | KeyCode::Char('l') => {
            app.last_key = None;
            app.open_logs();
        }
        KeyCode::Char('i') => {
            if let Some(c) = app.selected_container() {
                let id = c.id.clone();
                let name = c.name.clone();
                app.inspect_container_name = name;
                let backend = std::sync::Arc::clone(&app.backend);
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    match backend.inspect_container_json(&id).await {
                        Ok(lines) => {
                            let _ = tx.send(AppMessage::InspectData(lines)).await;
                        }
                        Err(e) => {
                            let _ = tx.send(AppMessage::Error(e.to_string())).await;
                        }
                    }
                });
            }
            app.last_key = None;
        }
        KeyCode::Char('e') => {
            if let Some(c) = app.selected_container() {
                if c.state == "running" {
                    let cid = c.id.clone();
                    app.last_key = None;
                    return Some(EventOutcome::ExecShell { container_id: cid, shell: "sh".to_string() });
                } else {
                    app.status_message = Some("container is not running".to_string());
                }
            }
            app.last_key = None;
        }
        KeyCode::Char('E') => {
            // Custom shell exec
            if let Some(c) = app.selected_container() {
                if c.state == "running" {
                    app.exec_shell_active = true;
                    app.exec_shell_input = Some(String::new());
                } else {
                    app.status_message = Some("container is not running".to_string());
                }
            }
            app.last_key = None;
        }
        KeyCode::Char('y') => {
            if let Some(c) = app.selected_container() {
                let short_id = c.id.chars().take(12).collect::<String>();
                copy_to_clipboard(&short_id);
                app.status_message = Some(format!("copied: {short_id}"));
            }
            app.last_key = None;
        }
        KeyCode::Char('s') => {
            app.last_key = None;
            let selected_ids: Vec<String> = if !app.container_selected_ids.is_empty() {
                app.container_selected_ids.iter().cloned().collect()
            } else if let Some(c) = app.selected_container() {
                vec![c.id.clone()]
            } else {
                vec![]
            };
            let states: Vec<String> = selected_ids.iter().filter_map(|id| {
                app.containers.iter().find(|c| &c.id == id).map(|c| c.state.clone())
            }).collect();
            let backend = std::sync::Arc::clone(&app.backend);
            for (id, state) in selected_ids.iter().zip(states.iter()) {
                let id = id.clone();
                let state = state.clone();
                let b = std::sync::Arc::clone(&backend);
                let tx = app.tx.clone();
                if state == "running" {
                    tokio::spawn(async move {
                        let result = b.stop(&id).await;
                        let msg = match result {
                            Ok(_) => format!("stopped {id}"),
                            Err(e) => format!("error: {e}"),
                        };
                        let _ = tx.send(AppMessage::Error(msg)).await;
                    });
                } else {
                    tokio::spawn(async move {
                        let result = b.start(&id).await;
                        let msg = match result {
                            Ok(_) => format!("started {id}"),
                            Err(e) => format!("error: {e}"),
                        };
                        let _ = tx.send(AppMessage::Error(msg)).await;
                    });
                }
            }
            if !selected_ids.is_empty() {
                app.status_message = Some(format!("toggling {} container(s)...", selected_ids.len()));
            }
        }
        KeyCode::Char('r') => {
            app.last_key = None;
            let ids: Vec<String> = if !app.container_selected_ids.is_empty() {
                app.container_selected_ids.iter().cloned().collect()
            } else if let Some(c) = app.selected_container() {
                vec![c.id.clone()]
            } else {
                vec![]
            };
            let backend = std::sync::Arc::clone(&app.backend);
            for id in &ids {
                let id = id.clone();
                let b = std::sync::Arc::clone(&backend);
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    let result = b.restart(&id).await;
                    let msg = match result {
                        Ok(_) => format!("restarted {id}"),
                        Err(e) => format!("error: {e}"),
                    };
                    let _ = tx.send(AppMessage::Error(msg)).await;
                });
            }
            if !ids.is_empty() {
                app.status_message = Some(format!("restarting {} container(s)...", ids.len()));
            }
        }
        KeyCode::Char('d') => {
            app.last_key = None;
            if !app.container_selected_ids.is_empty() {
                let ids: Vec<String> = app.container_selected_ids.iter().cloned().collect();
                let msg = format!("Remove {} containers? (y/n)", ids.len());
                app.pending_action = Action::Confirm {
                    message: msg,
                    action: ConfirmAction::RemoveMultiple(ids),
                };
            } else if let Some(c) = app.selected_container() {
                app.pending_action = Action::Confirm {
                    message: format!("Remove container '{}'? (y/n)", c.name),
                    action: ConfirmAction::Remove(c.id.clone()),
                };
            }
        }
        KeyCode::Char('P') => {
            app.last_key = None;
            if let Some(c) = app.selected_container() {
                let id = c.id.clone();
                let state = c.state.clone();
                let backend = std::sync::Arc::clone(&app.backend);
                let tx = app.tx.clone();
                if state == "paused" {
                    tokio::spawn(async move {
                        match backend.unpause(&id).await {
                            Ok(_) => { let _ = tx.send(AppMessage::Error(format!("unpaused {id}"))).await; }
                            Err(e) => { let _ = tx.send(AppMessage::Error(format!("error: {e}"))).await; }
                        }
                    });
                } else if state == "running" {
                    tokio::spawn(async move {
                        match backend.pause(&id).await {
                            Ok(_) => { let _ = tx.send(AppMessage::Error(format!("paused {id}"))).await; }
                            Err(e) => { let _ = tx.send(AppMessage::Error(format!("error: {e}"))).await; }
                        }
                    });
                } else {
                    app.status_message = Some("container is not running".to_string());
                }
            }
        }
        _ => {
            app.last_key = None;
        }
    }
    None
}

fn handle_logs(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Char('f') => {
            app.log_follow = !app.log_follow;
            if app.log_follow {
                app.log_scroll = app.log_lines.len().saturating_sub(1);
            }
        }
        KeyCode::Char('g') => {
            app.log_scroll = 0;
            app.log_follow = false;
        }
        KeyCode::Char('G') => {
            app.log_scroll = app.log_lines.len().saturating_sub(1);
            app.log_follow = true;
        }
        KeyCode::Char('/') => {
            app.log_search_active = true;
            app.log_search = Some(String::new());
        }
        KeyCode::Char('n') => {
            if !app.log_search_matches.is_empty() {
                app.log_search_idx = (app.log_search_idx + 1) % app.log_search_matches.len();
                app.log_scroll = app.log_search_matches[app.log_search_idx];
                app.log_follow = false;
            }
        }
        KeyCode::Char('N') => {
            if !app.log_search_matches.is_empty() {
                if app.log_search_idx == 0 {
                    app.log_search_idx = app.log_search_matches.len() - 1;
                } else {
                    app.log_search_idx -= 1;
                }
                app.log_scroll = app.log_search_matches[app.log_search_idx];
                app.log_follow = false;
            }
        }
        KeyCode::Char('E') => {
            export_logs(app);
        }
        KeyCode::Char('F') => {
            app.log_filter = app.log_filter.next();
        }
        KeyCode::Esc => {
            if app.log_search.is_some() {
                app.log_search = None;
                app.log_search_matches.clear();
            } else {
                app.view = View::Containers;
            }
        }
        _ => {}
    }
}

fn handle_images(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Char('g') => {
            if app.last_key == Some(KeyCode::Char('g')) {
                app.image_selected = 0;
                app.last_key = None;
            } else {
                app.last_key = Some(KeyCode::Char('g'));
            }
            return;
        }
        KeyCode::Char('G') => {
            let list = app.filtered_images();
            if !list.is_empty() {
                let last_id = list.last().unwrap().id.clone();
                if let Some(pos) = app.images.iter().position(|i| i.id == last_id) {
                    app.image_selected = pos;
                }
            }
        }
        KeyCode::Char('/') => {
            app.filter_active = true;
            app.filter_input = Some(String::new());
        }
        KeyCode::Esc => {
            app.filter_input = None;
            app.filter_active = false;
        }
        KeyCode::Char('y') => {
            if let Some(img) = app.images.get(app.image_selected) {
                let short_id = if img.id.starts_with("sha256:") {
                    img.id[7..].chars().take(12).collect::<String>()
                } else {
                    img.id.chars().take(12).collect::<String>()
                };
                copy_to_clipboard(&short_id);
                app.status_message = Some(format!("copied: {short_id}"));
            }
        }
        _ => {}
    }
    app.last_key = None;
}

fn handle_contexts(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Enter => {
            if let Some(name) = app.contexts.get(app.context_selected).cloned() {
                let result = std::process::Command::new("docker")
                    .args(["context", "use", &name])
                    .status();
                app.status_message = Some(match result {
                    Ok(s) if s.success() => format!("switched to context '{name}'"),
                    Ok(_) => format!("docker context use {name} failed"),
                    Err(e) => format!("error: {e}"),
                });
            }
        }
        _ => {}
    }
}

async fn execute_confirmed(app: &mut App, action: ConfirmAction) {
    match action {
        ConfirmAction::Remove(id) => {
            let backend = std::sync::Arc::clone(&app.backend);
            let result = backend.remove(&id, false).await;
            app.status_message = Some(match result {
                Ok(_) => format!("removed {id}"),
                Err(e) => format!("error: {e}"),
            });
        }
        ConfirmAction::RemoveMultiple(ids) => {
            let backend = std::sync::Arc::clone(&app.backend);
            let mut errors = vec![];
            for id in &ids {
                if let Err(e) = backend.remove(id, false).await {
                    errors.push(format!("{id}: {e}"));
                }
            }
            app.container_selected_ids.clear();
            if errors.is_empty() {
                app.status_message = Some(format!("removed {} containers", ids.len()));
            } else {
                app.status_message = Some(format!("errors: {}", errors.join(", ")));
            }
        }
    }
}

fn export_logs(app: &mut App) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = format!("{home}/.local/share/orca/logs");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        app.status_message = Some(format!("export error: {e}"));
        return;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Use current container name if available
    let container_name = app
        .selected_container()
        .map(|c| c.name.replace('/', "_"))
        .unwrap_or_else(|| "unknown".to_string());
    let path = format!("{dir}/{container_name}_{ts}.log");
    let content = app.log_lines.join("\n");
    match std::fs::write(&path, content) {
        Ok(_) => app.status_message = Some(format!("exported: {path}")),
        Err(e) => app.status_message = Some(format!("export error: {e}")),
    }
}

fn copy_to_clipboard(text: &str) {
    // Try wl-copy, then xclip, then xsel
    let commands: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (cmd, args) in commands {
        if let Ok(mut child) = std::process::Command::new(cmd)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.take() {
                use std::io::Write;
                let mut stdin = stdin;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
}

/// Spawn an interactive shell inside a container, suspending the TUI while it runs.
/// Caller is responsible for restoring terminal state before/after this call.
pub fn exec_shell_blocking(container_id: &str, kind: &BackendKind, shell: &str) -> Result<()> {
    let (cmd, mut args): (&str, Vec<&str>) = match kind {
        BackendKind::Docker => ("docker", vec!["exec", "-it", container_id]),
        BackendKind::Podman => ("podman", vec!["exec", "-it", container_id]),
    };
    args.push(shell);
    std::process::Command::new(cmd).args(&args).status()?;
    Ok(())
}
