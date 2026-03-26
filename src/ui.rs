use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Sparkline, Table,
        TableState,
    },
};

use crate::app::{Action, App, LogFilter, View};
use crate::docker::{BackendKind, ContainerInfo};

const HEADER_COLOR: Color = Color::Cyan;
const SELECTED_COLOR: Color = Color::Yellow;
const RUNNING_COLOR: Color = Color::Green;
const STOPPED_COLOR: Color = Color::Red;
const PAUSED_COLOR: Color = Color::Magenta;

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
        View::Containers => {
            if app.inspect_view {
                draw_inspect(f, area, app);
            } else {
                draw_containers(f, area, app);
            }
        }
        View::Logs => draw_logs(f, area, app),
        View::Images => draw_images(f, area, app),
        View::Contexts => draw_contexts(f, area, app),
    }
}

fn draw_containers(f: &mut Frame, area: Rect, app: &mut App) {
    // Split area: top for table (+ filter bar), bottom for sparklines
    let selected_id = app.containers.get(app.container_selected).map(|c| c.id.clone());
    let has_sparkline = selected_id.as_ref().map(|id| {
        app.cpu_history.contains_key(id) || app.mem_history.contains_key(id)
    }).unwrap_or(false);

    let (table_area, spark_area) = if has_sparkline {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(6)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    // Build sorted+filtered list
    let containers: Vec<&ContainerInfo> = app.sorted_filtered_containers();

    // Determine visual selection index in filtered list
    let visual_selected = selected_id.as_ref().and_then(|id| {
        containers.iter().position(|c| &c.id == id)
    }).unwrap_or(0);

    let sort_label = app.container_sort.label();
    let wide = app.wide_mode;
    let multi_count = app.container_selected_ids.len();

    let title = if multi_count > 0 {
        format!(" Containers [sort: {sort_label}] [{multi_count} selected] ")
    } else {
        format!(" Containers [sort: {sort_label}] ")
    };

    let header = if wide {
        Row::new(vec![
            Cell::from("  Name").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Image").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Status").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("CPU").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Mem").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Ports").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        ])
    } else {
        Row::new(vec![
            Cell::from("  Name").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Image").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Status").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("CPU").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
            Cell::from("Mem").style(Style::default().fg(HEADER_COLOR).add_modifier(Modifier::BOLD)),
        ])
    };

    let selected_ids = &app.container_selected_ids;

    let rows: Vec<Row> = containers.iter().map(|c| {
        let state_color = match c.state.as_str() {
            "running" => RUNNING_COLOR,
            "paused" => PAUSED_COLOR,
            _ => STOPPED_COLOR,
        };
        let is_multi_selected = selected_ids.contains(&c.id);
        let prefix = if is_multi_selected { "● " } else { "  " };
        let name_cell = if is_multi_selected {
            Cell::from(format!("{prefix}{}", c.name))
                .style(Style::default().fg(Color::Cyan))
        } else {
            Cell::from(format!("{prefix}{}", c.name))
        };
        let image_cell = Cell::from(truncate(&c.image, 28));
        let status_cell = Cell::from(c.status.clone()).style(Style::default().fg(state_color));
        let cpu_cell = Cell::from(format!("{:.1}%", c.cpu_percent));
        let mem_cell = Cell::from(format_bytes(c.mem_usage));

        if wide {
            let ports_str = if c.ports.is_empty() {
                "-".to_string()
            } else {
                c.ports.join(", ")
            };
            let ports_cell = Cell::from(truncate(&ports_str, 28));
            Row::new(vec![name_cell, image_cell, status_cell, cpu_cell, mem_cell, ports_cell])
        } else {
            Row::new(vec![name_cell, image_cell, status_cell, cpu_cell, mem_cell])
        }
    }).collect();

    let mut state = TableState::default();
    state.select(Some(visual_selected));

    let constraints: Vec<Constraint> = if wide {
        vec![
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(15),
            Constraint::Percentage(8),
            Constraint::Percentage(8),
            Constraint::Percentage(29),
        ]
    } else {
        vec![
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ]
    };

    // Check if filter bar is needed
    let filter_text = app.filter_input.clone();
    let filter_active = app.filter_active;
    let exec_shell_active = app.exec_shell_active;
    let exec_shell_input = app.exec_shell_input.clone();

    // Split for filter bar if needed
    let (actual_table_area, overlay_area) = if filter_text.is_some() || filter_active || exec_shell_active {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(table_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (table_area, None)
    };

    let table = Table::new(rows, constraints)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_alignment(Alignment::Left),
        )
        .row_highlight_style(
            Style::default()
                .fg(SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(table, actual_table_area, &mut state);

    // Render filter bar / exec shell bar
    if let Some(bar_area) = overlay_area {
        let bar_text = if exec_shell_active {
            let input = exec_shell_input.as_deref().unwrap_or("");
            format!("Shell: {input}_")
        } else {
            let input = filter_text.as_deref().unwrap_or("");
            let cursor = if filter_active { "_" } else { "" };
            format!("Filter: {input}{cursor}")
        };
        let bar = Paragraph::new(bar_text)
            .style(Style::default().fg(Color::White).bg(Color::DarkGray));
        f.render_widget(bar, bar_area);
    }

    // Render sparklines
    if let (Some(spark_area), Some(sel_id)) = (spark_area, selected_id) {
        draw_sparklines(f, spark_area, app, &sel_id);
    }
}

fn draw_sparklines(f: &mut Frame, area: Rect, app: &App, container_id: &str) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // CPU sparkline
    if let Some(cpu_hist) = app.cpu_history.get(container_id) {
        let data: Vec<u64> = cpu_hist.iter().map(|v| (*v * 10.0) as u64).collect();
        let spark = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" CPU history (60s) ")
                    .title_alignment(Alignment::Left),
            )
            .data(&data)
            .style(Style::default().fg(Color::Green));
        f.render_widget(spark, chunks[0]);
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" CPU history (60s) ");
        f.render_widget(block, chunks[0]);
    }

    // Mem sparkline
    if let Some(mem_hist) = app.mem_history.get(container_id) {
        let data: Vec<u64> = mem_hist.iter().map(|v| v / 1024 / 1024).collect(); // MB
        let spark = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Mem history (60s) ")
                    .title_alignment(Alignment::Left),
            )
            .data(&data)
            .style(Style::default().fg(Color::Blue));
        f.render_widget(spark, chunks[1]);
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Mem history (60s) ");
        f.render_widget(block, chunks[1]);
    }
}

fn draw_inspect(f: &mut Frame, area: Rect, app: &App) {
    let title = format!(" Inspect: {} ", app.inspect_container_name);
    let visible_height = area.height.saturating_sub(2) as usize;
    let total = app.inspect_data.len();

    let start = if total > visible_height {
        app.inspect_scroll.min(total - visible_height)
    } else {
        0
    };

    let lines: Vec<Line> = app.inspect_data
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

fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let follow_indicator = if app.log_follow { " [follow]" } else { "" };
    let filter_label = if app.log_filter != LogFilter::NoFilter {
        format!(" [filter: {}]", app.log_filter.label())
    } else {
        String::new()
    };
    let title = format!(" Logs{follow_indicator}{filter_label} ");

    // Check if search bar is needed
    let search_active = app.log_search_active;
    let search_query = app.log_search.clone();

    let (log_area, search_bar_area) = if search_active || search_query.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let visible_height = log_area.height.saturating_sub(2) as usize;

    // Filter lines according to log_filter
    let filtered_lines: Vec<(usize, &String)> = app.log_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| match app.log_filter {
            LogFilter::NoFilter => true,
            LogFilter::HideDebug => !line.to_lowercase().contains("debug"),
            LogFilter::ErrorOnly => {
                let lower = line.to_lowercase();
                lower.contains("error") || lower.contains("warn")
            }
        })
        .collect();

    let total_filtered = filtered_lines.len();

    let start = if total_filtered > visible_height {
        app.log_scroll.min(total_filtered.saturating_sub(visible_height))
    } else {
        0
    };

    let search_term = app.log_search.clone().filter(|s| !s.is_empty());

    let lines: Vec<Line> = filtered_lines
        .iter()
        .skip(start)
        .take(visible_height)
        .map(|(orig_idx, l)| {
            let is_match_line = search_term.as_ref().map(|q| {
                let lower_line = l.to_lowercase();
                let lower_q = q.to_lowercase();
                lower_line.contains(&lower_q)
            }).unwrap_or(false);

            let is_current_match = app.log_search_matches
                .get(app.log_search_idx)
                .map(|&mi| mi == *orig_idx)
                .unwrap_or(false);

            if let Some(ref q) = search_term {
                if is_match_line {
                    ansi_to_line_with_highlight(l, q, is_current_match)
                } else {
                    ansi_to_line(l)
                }
            } else {
                ansi_to_line(l)
            }
        })
        .collect();

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_alignment(Alignment::Left),
        );

    f.render_widget(paragraph, log_area);

    if let Some(bar_area) = search_bar_area {
        let query = search_query.as_deref().unwrap_or("");
        let match_info = if !app.log_search_matches.is_empty() {
            format!(" ({}/{})", app.log_search_idx + 1, app.log_search_matches.len())
        } else if !query.is_empty() {
            " (no matches)".to_string()
        } else {
            String::new()
        };
        let cursor = if search_active { "_" } else { "" };
        let bar_text = format!("/{query}{cursor}{match_info}");
        let bar = Paragraph::new(bar_text)
            .style(Style::default().fg(Color::White).bg(Color::DarkGray));
        f.render_widget(bar, bar_area);
    }
}

fn draw_images(f: &mut Frame, area: Rect, app: &mut App) {
    let filter_text = app.filter_input.clone();
    let filter_active = app.filter_active;

    let (table_area, filter_bar_area) = if filter_text.is_some() || filter_active {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

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

    let filtered_images = app.filtered_images();
    let visual_selected = {
        let sel_id = app.images.get(app.image_selected).map(|i| i.id.clone());
        sel_id.as_ref().and_then(|id| filtered_images.iter().position(|i| &i.id == id)).unwrap_or(0)
    };

    let rows: Vec<Row> = filtered_images
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
    state.select(Some(visual_selected));

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

    f.render_stateful_widget(table, table_area, &mut state);

    if let Some(bar_area) = filter_bar_area {
        let input = filter_text.as_deref().unwrap_or("");
        let cursor = if filter_active { "_" } else { "" };
        let bar = Paragraph::new(format!("Filter: {input}{cursor}"))
            .style(Style::default().fg(Color::White).bg(Color::DarkGray));
        f.render_widget(bar, bar_area);
    }
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
        View::Containers => {
            if app.inspect_view {
                "j/k:scroll  esc:back  q:quit"
            } else {
                "j/k:nav  enter:logs  e:exec  E:custom-exec  s:start/stop  r:restart  d:remove  P:pause  i:inspect  S:sort  W:wide  /:filter  space:select  y:copy  q:quit"
            }
        }
        View::Logs => "j/k:scroll  f:follow  g:top  G:bottom  /:search  n/N:next/prev  E:export  F:filter  esc:back  q:quit",
        View::Images => "j/k:navigate  /:filter  y:copy  tab:switch  q:quit",
        View::Contexts => "j/k:navigate  enter:switch context  tab:switch  q:quit",
    };

    let sel_count = app.container_selected_ids.len();
    let sel_info = if sel_count > 0 && app.view == View::Containers {
        format!("  {sel_count} selected  │  ")
    } else {
        String::new()
    };

    let status = if let Some(msg) = &app.status_message {
        format!("{msg}{sel_info}  │  {hints}")
    } else {
        format!("{sel_info}{hints}")
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

/// Parse ANSI escape codes from a log line and return a ratatui Line with styled spans.
pub fn ansi_to_line(s: &str) -> Line<'static> {
    let spans = parse_ansi_spans(s, None, false);
    Line::from(spans)
}

/// Parse ANSI escape codes, also highlighting search matches.
pub fn ansi_to_line_with_highlight(s: &str, query: &str, is_current: bool) -> Line<'static> {
    let spans = parse_ansi_spans(s, Some(query), is_current);
    Line::from(spans)
}

fn parse_ansi_spans(s: &str, highlight_query: Option<&str>, is_current_match: bool) -> Vec<Span<'static>> {
    // First, parse ANSI codes to get (text, style) segments
    let segments = parse_ansi_segments(s);

    if let Some(query) = highlight_query {
        if query.is_empty() {
            return segments.into_iter().map(|(t, st)| Span::styled(t, st)).collect();
        }
        let highlight_style = if is_current_match {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        };
        let lower_query = query.to_lowercase();

        let mut result = vec![];
        for (text, style) in segments {
            let lower_text = text.to_lowercase();
            let mut pos = 0;
            for m in lower_text.match_indices(&lower_query as &str) {
                let (start, _) = m;
                if pos < start {
                    result.push(Span::styled(text[pos..start].to_string(), style));
                }
                result.push(Span::styled(text[start..start + query.len()].to_string(), highlight_style));
                pos = start + query.len();
            }
            if pos < text.len() {
                result.push(Span::styled(text[pos..].to_string(), style));
            }
        }
        result
    } else {
        segments.into_iter().map(|(t, st)| Span::styled(t, st)).collect()
    }
}

/// Parse ANSI SGR escape sequences and return (text_segment, style) pairs.
fn parse_ansi_segments(s: &str) -> Vec<(String, Style)> {
    let mut result = vec![];
    let mut current_style = Style::default();
    let mut current_text = String::new();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'[' {
            // Start of CSI sequence
            let start = i;
            i += 2; // skip ESC [
            let seq_start = i;
            // Read until final byte (letter or certain chars)
            while i < len && !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'm') {
                i += 1;
            }
            if i < len && bytes[i] == b'm' {
                // SGR sequence
                if !current_text.is_empty() {
                    result.push((current_text.clone(), current_style));
                    current_text.clear();
                }
                let params_str = &s[seq_start..i];
                current_style = apply_sgr(current_style, params_str);
                i += 1; // skip 'm'
            } else {
                // Not an 'm' sequence — skip final byte, treat as plain text
                // Emit the raw escape sequence as plain text
                let raw = &s[start..=i.min(len - 1)];
                current_text.push_str(raw);
                if i < len { i += 1; }
            }
        } else {
            // Regular character
            current_text.push(bytes[i] as char);
            i += 1;
        }
    }

    if !current_text.is_empty() {
        result.push((current_text, current_style));
    }

    if result.is_empty() {
        result.push((String::new(), Style::default()));
    }

    result
}

fn apply_sgr(style: Style, params: &str) -> Style {
    if params.is_empty() {
        return Style::default(); // ESC[m or ESC[0m — reset
    }

    let codes: Vec<u8> = params
        .split(';')
        .filter_map(|p| p.parse::<u8>().ok())
        .collect();

    let mut s = style;
    let mut idx = 0;
    while idx < codes.len() {
        let code = codes[idx];
        match code {
            0 => s = Style::default(),
            1 => s = s.add_modifier(Modifier::BOLD),
            2 => s = s.add_modifier(Modifier::DIM),
            3 => s = s.add_modifier(Modifier::ITALIC),
            4 => s = s.add_modifier(Modifier::UNDERLINED),
            5 => s = s.add_modifier(Modifier::SLOW_BLINK),
            7 => s = s.add_modifier(Modifier::REVERSED),
            9 => s = s.add_modifier(Modifier::CROSSED_OUT),
            22 => s = s.remove_modifier(Modifier::BOLD).remove_modifier(Modifier::DIM),
            23 => s = s.remove_modifier(Modifier::ITALIC),
            24 => s = s.remove_modifier(Modifier::UNDERLINED),
            27 => s = s.remove_modifier(Modifier::REVERSED),
            // Foreground colors
            30 => s = s.fg(Color::Black),
            31 => s = s.fg(Color::Red),
            32 => s = s.fg(Color::Green),
            33 => s = s.fg(Color::Yellow),
            34 => s = s.fg(Color::Blue),
            35 => s = s.fg(Color::Magenta),
            36 => s = s.fg(Color::Cyan),
            37 => s = s.fg(Color::White),
            38 => {
                // Extended foreground color
                if idx + 2 < codes.len() && codes[idx + 1] == 5 {
                    // 256 color
                    s = s.fg(Color::Indexed(codes[idx + 2]));
                    idx += 2;
                } else if idx + 4 < codes.len() && codes[idx + 1] == 2 {
                    // RGB
                    s = s.fg(Color::Rgb(codes[idx + 2], codes[idx + 3], codes[idx + 4]));
                    idx += 4;
                }
            }
            39 => s = s.fg(Color::Reset),
            // Background colors
            40 => s = s.bg(Color::Black),
            41 => s = s.bg(Color::Red),
            42 => s = s.bg(Color::Green),
            43 => s = s.bg(Color::Yellow),
            44 => s = s.bg(Color::Blue),
            45 => s = s.bg(Color::Magenta),
            46 => s = s.bg(Color::Cyan),
            47 => s = s.bg(Color::White),
            48 => {
                if idx + 2 < codes.len() && codes[idx + 1] == 5 {
                    s = s.bg(Color::Indexed(codes[idx + 2]));
                    idx += 2;
                } else if idx + 4 < codes.len() && codes[idx + 1] == 2 {
                    s = s.bg(Color::Rgb(codes[idx + 2], codes[idx + 3], codes[idx + 4]));
                    idx += 4;
                }
            }
            49 => s = s.bg(Color::Reset),
            // Bright foreground colors
            90 => s = s.fg(Color::DarkGray),
            91 => s = s.fg(Color::LightRed),
            92 => s = s.fg(Color::LightGreen),
            93 => s = s.fg(Color::LightYellow),
            94 => s = s.fg(Color::LightBlue),
            95 => s = s.fg(Color::LightMagenta),
            96 => s = s.fg(Color::LightCyan),
            97 => s = s.fg(Color::Gray),
            // Bright background colors
            100 => s = s.bg(Color::DarkGray),
            101 => s = s.bg(Color::LightRed),
            102 => s = s.bg(Color::LightGreen),
            103 => s = s.bg(Color::LightYellow),
            104 => s = s.bg(Color::LightBlue),
            105 => s = s.bg(Color::LightMagenta),
            106 => s = s.bg(Color::LightCyan),
            107 => s = s.bg(Color::Gray),
            _ => {}
        }
        idx += 1;
    }
    s
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
