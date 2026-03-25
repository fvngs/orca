use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState},
};

use crate::app::{Action, App, View};
use crate::docker::{BackendKind, ContainerInfo};

const HEADER_COLOR: Color = Color::Cyan;
const SELECTED_COLOR: Color = Color::Yellow;
const RUNNING_COLOR: Color = Color::Green;
const STOPPED_COLOR: Color = Color::Red;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(0),    // main content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    draw_title_bar(f, chunks[0], app);
    draw_main(f, chunks[1], app);
    draw_status_bar(f, chunks[2], app);

    // Overlay confirm dialog if needed
    if let Action::Confirm { message, .. } = &app.pending_action.clone() {
        draw_confirm_dialog(f, area, message);
    }
}

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let backend_label = match app.backend.kind {
        BackendKind::Docker => "Docker",
        BackendKind::Podman => "Podman",
    };

    let tabs = vec![
        tab_span("1:Containers", app.view == View::Containers),
        Span::raw("  "),
        tab_span("2:Logs", app.view == View::Logs),
        Span::raw("  "),
        tab_span("3:Images", app.view == View::Images),
        Span::raw("  "),
        tab_span("4:Contexts", app.view == View::Contexts),
        Span::raw(format!("   [{backend_label}]")),
    ];

    let title = Paragraph::new(Line::from(tabs))
        .style(Style::default().bg(Color::DarkGray));
    f.render_widget(title, area);
}

fn tab_span(label: &str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(HEADER_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label.to_string(), Style::default().fg(Color::White))
    }
}

fn draw_main(f: &mut Frame, area: Rect, app: &mut App) {
    match app.view {
        View::Containers => draw_containers(f, area, app),
        View::Logs => draw_logs(f, area, app),
        View::Images => draw_images(f, area, app),
        View::Contexts => draw_contexts(f, area, app),
    }
}

fn draw_containers(f: &mut Frame, area: Rect, app: &mut App) {
    // Group by compose project
    let mut groups: Vec<(Option<String>, Vec<&ContainerInfo>)> = vec![];
    for c in &app.containers {
        if let Some(g) = groups.iter_mut().find(|(p, _)| *p == c.compose_project) {
            g.1.push(c);
        } else {
            groups.push((c.compose_project.clone(), vec![c]));
        }
    }

    let header = Row::new(vec![
        Cell::from("  Name").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("Image").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("Status").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("CPU").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("Mem").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
    ]);

    let mut rows = vec![];
    let mut flat_indices: Vec<usize> = vec![];
    let mut flat_idx = 0usize;

    for (project, containers) in &groups {
        if let Some(name) = project {
            rows.push(
                Row::new(vec![
                    Cell::from(format!("▸ {name}"))
                        .style(Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .height(1),
            );
            flat_indices.push(usize::MAX); // group header — not selectable
        }

        for c in containers {
            let state_color = if c.state == "running" { RUNNING_COLOR } else { STOPPED_COLOR };
            let prefix = if project.is_some() { "  " } else { "" };
            let name_cell = Cell::from(format!("{prefix}{}", c.name));
            let image_cell = Cell::from(truncate(&c.image, 30));
            let status_cell =
                Cell::from(c.status.clone()).style(Style::default().fg(state_color));
            let cpu_cell = Cell::from(format!("{:.1}%", c.cpu_percent));
            let mem_cell = Cell::from(format_bytes(c.mem_usage));

            let row = Row::new(vec![name_cell, image_cell, status_cell, cpu_cell, mem_cell]);
            rows.push(row);
            flat_indices.push(flat_idx);
            flat_idx += 1;
        }
    }

    // Determine which visual row is selected
    let visual_selected = flat_indices
        .iter()
        .position(|&i| i == app.container_selected)
        .unwrap_or(0);

    let mut state = TableState::default();
    state.select(Some(visual_selected));

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Containers ")
            .title_alignment(Alignment::Left),
    )
    .row_highlight_style(
        Style::default()
            .fg(SELECTED_COLOR)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, area, &mut state);
}

fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let follow_indicator = if app.log_follow { " [follow]" } else { "" };
    let title = format!(" Logs{follow_indicator} ");

    let visible_height = area.height.saturating_sub(2) as usize;
    let total = app.log_lines.len();

    let start = if total > visible_height {
        app.log_scroll.min(total - visible_height)
    } else {
        0
    };

    let lines: Vec<Line> = app
        .log_lines
        .iter()
        .skip(start)
        .take(visible_height)
        .map(|l| Line::from(Span::raw(l.clone())))
        .collect();

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_alignment(Alignment::Left),
        );

    f.render_widget(paragraph, area);
}

fn draw_images(f: &mut Frame, area: Rect, app: &mut App) {
    let header = Row::new(vec![
        Cell::from("Repository:Tag")
            .style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("ID")
            .style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("Size")
            .style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        Cell::from("Created")
            .style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
    ]);

    let rows: Vec<Row> = app
        .images
        .iter()
        .map(|img| {
            let tag = img
                .repo_tags
                .first()
                .cloned()
                .unwrap_or_else(|| "<none>".to_string());
            let id = &img.id[7..15.min(img.id.len())]; // strip "sha256:" prefix, show 8 chars
            let size = format_bytes(img.size);
            let age = format_age(img.created);
            Row::new(vec![
                Cell::from(tag),
                Cell::from(id.to_string()),
                Cell::from(size),
                Cell::from(age),
            ])
        })
        .collect();

    let mut state = TableState::default();
    state.select(Some(app.image_selected));

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(50),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Images ")
            .title_alignment(Alignment::Left),
    )
    .row_highlight_style(
        Style::default()
            .fg(SELECTED_COLOR)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, area, &mut state);
}

fn draw_contexts(f: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .contexts
        .iter()
        .map(|ctx| ListItem::new(ctx.clone()))
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.context_selected));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Contexts ")
                .title_alignment(Alignment::Left),
        )
        .highlight_style(
            Style::default()
                .fg(SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut state);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let hints = match app.view {
        View::Containers => "j/k:navigate  enter/l:logs  s:start/stop  r:restart  d:remove  tab:switch  q:quit",
        View::Logs => "j/k:scroll  f:follow  g:top  G:bottom  esc:back  q:quit",
        View::Images => "j/k:navigate  tab:switch  q:quit",
        View::Contexts => "j/k:navigate  enter:select  tab:switch  q:quit",
    };

    let status = if let Some(msg) = &app.status_message {
        format!("{msg}  │  {hints}")
    } else {
        hints.to_string()
    };

    let paragraph = Paragraph::new(status)
        .style(Style::default().fg(Color::DarkGray).bg(Color::Black))
        .alignment(Alignment::Left);

    f.render_widget(paragraph, area);
}

fn draw_confirm_dialog(f: &mut Frame, area: Rect, message: &str) {
    let width = (message.len() + 4).min(area.width as usize) as u16;
    let height = 3u16;
    let x = area.width.saturating_sub(width) / 2;
    let y = area.height.saturating_sub(height) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray).fg(Color::Yellow));

    let inner = block.inner(dialog_area);
    f.render_widget(ratatui::widgets::Clear, dialog_area);
    f.render_widget(block, dialog_area);

    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(Color::White).bg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(paragraph, inner);
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}B")
    }
}

fn format_age(created: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - created).max(0) as u64;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
