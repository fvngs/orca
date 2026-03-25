use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use crate::app::{Action, App, ConfirmAction, View};

/// Returns true if the application should quit.
pub async fn handle_events(app: &mut App) -> Result<bool> {
    // Always drain background messages first
    app.process_messages();

    if !event::poll(Duration::from_millis(50))? {
        return Ok(false);
    }

    if let Event::Key(key) = event::read()? {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
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
            return Ok(false);
        }

        // Global keybinds
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Tab => cycle_view(app),
            KeyCode::Char('1') => app.view = View::Containers,
            KeyCode::Char('2') => app.view = View::Logs,
            KeyCode::Char('3') => app.view = View::Images,
            KeyCode::Char('4') => app.view = View::Contexts,
            _ => handle_view_keys(app, key.code).await,
        }
    }

    Ok(false)
}

fn cycle_view(app: &mut App) {
    app.view = match app.view {
        View::Containers => View::Images,
        View::Images => View::Contexts,
        View::Contexts => View::Containers,
        View::Logs => View::Containers,
    };
}

async fn handle_view_keys(app: &mut App, code: KeyCode) {
    match app.view {
        View::Containers => handle_containers(app, code).await,
        View::Logs => handle_logs(app, code),
        View::Images => handle_images(app, code),
        View::Contexts => handle_contexts(app, code).await,
    }
}

async fn handle_containers(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Enter | KeyCode::Char('l') => app.open_logs(),
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

async fn handle_contexts(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
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
