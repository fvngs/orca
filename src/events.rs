use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use crate::app::{Action, App, ConfirmAction, View};
use crate::docker::BackendKind;

pub enum EventOutcome {
    Continue,
    Quit,
    ExecShell { container_id: String },
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
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Enter | KeyCode::Char('l') => app.open_logs(),
        KeyCode::Char('e') => {
            if let Some(c) = app.selected_container() {
                if c.state == "running" {
                    return Some(EventOutcome::ExecShell {
                        container_id: c.id.clone(),
                    });
                } else {
                    app.status_message = Some("container is not running".to_string());
                }
            }
        }
        KeyCode::Char('s') => {
            if let Some(c) = app.selected_container() {
                let id = c.id.clone();
                let state = c.state.clone();
                let backend = std::sync::Arc::clone(&app.backend);
                if state == "running" {
                    let result = backend.stop(&id).await;
                    app.status_message = Some(match result {
                        Ok(_) => format!("stopped {id}"),
                        Err(e) => format!("error: {e}"),
                    });
                } else {
                    let result = backend.start(&id).await;
                    app.status_message = Some(match result {
                        Ok(_) => format!("started {id}"),
                        Err(e) => format!("error: {e}"),
                    });
                }
            }
        }
        KeyCode::Char('r') => {
            if let Some(c) = app.selected_container() {
                let id = c.id.clone();
                let backend = std::sync::Arc::clone(&app.backend);
                let result = backend.restart(&id).await;
                app.status_message = Some(match result {
                    Ok(_) => format!("restarted {id}"),
                    Err(e) => format!("error: {e}"),
                });
            }
        }
        KeyCode::Char('d') => {
            if let Some(c) = app.selected_container() {
                app.pending_action = Action::Confirm {
                    message: format!("Remove container '{}'? (y/n)", c.name),
                    action: ConfirmAction::Remove(c.id.clone()),
                };
            }
        }
        _ => {}
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
        KeyCode::Esc => app.view = View::Containers,
        _ => {}
    }
}

fn handle_images(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        _ => {}
    }
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
    }
}

/// Spawn an interactive shell inside a container, suspending the TUI while it runs.
/// Caller is responsible for restoring terminal state before/after this call.
pub fn exec_shell_blocking(container_id: &str, kind: &BackendKind) -> Result<()> {
    let (cmd, args): (&str, Vec<&str>) = match kind {
        BackendKind::Docker => ("docker", vec!["exec", "-it", container_id, "sh"]),
        BackendKind::Podman => ("podman", vec!["exec", "-it", container_id, "sh"]),
    };
    std::process::Command::new(cmd).args(&args).status()?;
    Ok(())
}
