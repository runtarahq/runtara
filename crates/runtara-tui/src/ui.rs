// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! UI rendering.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap},
    Frame,
};

use crate::app::{format_datetime, format_duration, status_style, App, Tab, ViewMode};

/// Main draw function
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header + tabs
            Constraint::Min(0),    // Content
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_content(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);

    // Draw modal views on top
    match app.view_mode {
        ViewMode::List => {}
        ViewMode::InstanceDetail => {
            draw_instance_detail_modal(f, app);
        }
        ViewMode::CheckpointsList => {
            draw_checkpoints_list_modal(f, app);
        }
        ViewMode::CheckpointDetail => {
            draw_checkpoint_detail_modal(f, app);
        }
    }

    // Draw error popup if present
    if let Some(ref error) = app.error {
        draw_error_popup(f, error);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(30)])
        .split(area);

    // Tabs
    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| {
            let style = if *t == app.tab {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(t.as_str(), style))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Runtara Monitor "),
        )
        .highlight_style(Style::default().fg(Color::Yellow))
        .select(match app.tab {
            Tab::Instances => 0,
            Tab::Images => 1,
            Tab::Health => 2,
        });

    f.render_widget(tabs, chunks[0]);

    // Connection status
    let status_text = if app.connected {
        Span::styled(" Connected ", Style::default().fg(Color::Green))
    } else {
        Span::styled(" Disconnected ", Style::default().fg(Color::Red))
    };

    let status = Paragraph::new(Line::from(vec![Span::raw("Status: "), status_text]))
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(status, chunks[1]);
}

fn draw_content(f: &mut Frame, app: &App, area: Rect) {
    match app.tab {
        Tab::Instances => draw_instances(f, app, area),
        Tab::Images => draw_images(f, app, area),
        Tab::Health => draw_health(f, app, area),
    }
}

fn draw_instances(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    // Filter info
    let filter_info = Paragraph::new(Line::from(vec![
        Span::raw(" Filter: "),
        Span::styled(
            app.status_filter.as_str(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::raw(format!("Total: {} ", app.instances_total)),
        Span::raw("| Press 'f' to cycle filter"),
    ]));
    f.render_widget(filter_info, chunks[0]);

    // Instances table
    let header = Row::new(vec![
        Cell::from("Instance ID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Status").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Tenant").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Image").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Created").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Finished").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .style(Style::default().fg(Color::Yellow));

    let rows: Vec<Row> = app
        .instances
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let (status_text, status_color) = status_style(inst.status);
            let is_selected = i == app.instances_selected;

            let style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(truncate(&inst.instance_id, 36)),
                Cell::from(status_text).style(Style::default().fg(status_color)),
                Cell::from(truncate(&inst.tenant_id, 20)),
                Cell::from(truncate(&inst.image_id, 20)),
                Cell::from(format_datetime(&inst.created_at)),
                Cell::from(
                    inst.finished_at
                        .as_ref()
                        .map(format_datetime)
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(38),
            Constraint::Length(12),
            Constraint::Length(22),
            Constraint::Length(22),
            Constraint::Length(20),
            Constraint::Length(20),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Instances ({}) ", app.instances.len())),
    );

    f.render_widget(table, chunks[1]);
}

fn draw_images(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Image ID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Name").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Tenant").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Runner").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Created").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Description").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .style(Style::default().fg(Color::Yellow));

    let rows: Vec<Row> = app
        .images
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let is_selected = i == app.images_selected;

            let style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(truncate(&img.image_id, 36)),
                Cell::from(truncate(&img.name, 30)),
                Cell::from(truncate(&img.tenant_id, 20)),
                Cell::from(format!("{:?}", img.runner_type)),
                Cell::from(format_datetime(&img.created_at)),
                Cell::from(truncate(img.description.as_deref().unwrap_or("-"), 30)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(38),
            Constraint::Length(32),
            Constraint::Length(22),
            Constraint::Length(10),
            Constraint::Length(20),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Images ({}) ", app.images.len())),
    );

    f.render_widget(table, area);
}

fn draw_health(f: &mut Frame, app: &App, area: Rect) {
    let content = match &app.health {
        Some(health) => {
            let healthy_style = if health.healthy {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            };

            let lines = vec![
                Line::from(vec![
                    Span::raw("  Status:           "),
                    Span::styled(
                        if health.healthy {
                            "Healthy"
                        } else {
                            "Unhealthy"
                        },
                        healthy_style,
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  Version:          "),
                    Span::styled(&health.version, Style::default().fg(Color::Cyan)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  Uptime:           "),
                    Span::styled(
                        format_duration(health.uptime_ms as u64),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  Active Instances: "),
                    Span::styled(
                        health.active_instances.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(""),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  Server:           "),
                    Span::styled(
                        app.server_addr.to_string(),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  Last Refresh:     "),
                    Span::styled(
                        app.last_refresh
                            .map(|t| format!("{}s ago", t.elapsed().as_secs()))
                            .unwrap_or_else(|| "Never".to_string()),
                        Style::default().fg(Color::White),
                    ),
                ]),
            ];

            Text::from(lines)
        }
        None => Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No health data available",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from("  Press 'r' to refresh"),
        ]),
    };

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Health Status "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let help_text = match app.view_mode {
        ViewMode::List => match app.tab {
            Tab::Instances => {
                "q:Quit | Tab:Switch Tab | 1-3:Tab | j/k:Navigate | Enter:Details | f:Filter | r:Refresh"
            }
            Tab::Images => "q:Quit | Tab:Switch Tab | 1-3:Tab | j/k:Navigate | r:Refresh",
            Tab::Health => "q:Quit | Tab:Switch Tab | 1-3:Tab | r:Refresh",
        },
        ViewMode::InstanceDetail => {
            "Esc:Back | c:Checkpoints | j/k:Scroll"
        }
        ViewMode::CheckpointsList => {
            "Esc:Back | Enter:View Data | j/k:Navigate"
        }
        ViewMode::CheckpointDetail => {
            "Esc:Back | j/k:Scroll"
        }
    };

    let tenant_info = app
        .tenant_id
        .as_ref()
        .map(|t| format!(" | Tenant: {}", t))
        .unwrap_or_default();

    let refresh_info = if app.view_mode == ViewMode::List {
        app.last_refresh
            .map(|t| {
                format!(
                    " | Next refresh in {}s",
                    app.refresh_interval
                        .as_secs()
                        .saturating_sub(t.elapsed().as_secs())
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(help_text, Style::default().fg(Color::DarkGray)),
        Span::styled(tenant_info, Style::default().fg(Color::Cyan)),
        Span::styled(refresh_info, Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(footer, area);
}

fn draw_instance_detail_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let info = match &app.instance_detail {
        Some(info) => info,
        None => return,
    };

    let (status_text, status_color) = status_style(info.status);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Instance ID:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(&info.instance_id, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Status:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tenant ID:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(&info.tenant_id, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("  Image ID:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(&info.image_id, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Image Name:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(&info.image_name, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Created At:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_datetime(&info.created_at),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Started At:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                info.started_at
                    .as_ref()
                    .map(format_datetime)
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Finished At:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                info.finished_at
                    .as_ref()
                    .map(format_datetime)
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Heartbeat At:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                info.heartbeat_at
                    .as_ref()
                    .map(format_datetime)
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Checkpoint ID:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                info.checkpoint_id.as_deref().unwrap_or("-"),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Retry Count:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} / {}", info.retry_count, info.max_retries),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    // Add input if present
    if let Some(input) = &info.input {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Input:",
            Style::default().fg(Color::DarkGray),
        )));
        let input_str =
            serde_json::to_string_pretty(input).unwrap_or_else(|_| format!("{:?}", input));
        for line in input_str.lines().take(5) {
            lines.push(Line::from(format!("    {}", line)));
        }
        if input_str.lines().count() > 5 {
            lines.push(Line::from(Span::styled(
                "    ...",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Add output if present
    if let Some(output) = &info.output {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Output:",
            Style::default().fg(Color::DarkGray),
        )));
        let output_str =
            serde_json::to_string_pretty(output).unwrap_or_else(|_| format!("{:?}", output));
        for line in output_str.lines().take(5) {
            lines.push(Line::from(Span::styled(
                format!("    {}", line),
                Style::default().fg(Color::Green),
            )));
        }
        if output_str.lines().count() > 5 {
            lines.push(Line::from(Span::styled(
                "    ...",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Add error if present
    if let Some(error) = &info.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Error:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for line in error.lines().take(5) {
            lines.push(Line::from(Span::styled(
                format!("    {}", line),
                Style::default().fg(Color::Red),
            )));
        }
    }

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Instance Details "),
        )
        .scroll((app.detail_scroll, 0))
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_checkpoints_list_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 70, f.area());
    f.render_widget(Clear, area);

    let instance_id = app
        .instance_detail
        .as_ref()
        .map(|i| i.instance_id.as_str())
        .unwrap_or("Unknown");

    let header = Row::new(vec![
        Cell::from("Checkpoint ID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Created At").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Size").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .style(Style::default().fg(Color::Yellow));

    let rows: Vec<Row> = app
        .checkpoints
        .iter()
        .enumerate()
        .map(|(i, cp)| {
            let is_selected = i == app.checkpoints_selected;

            let style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(truncate(&cp.checkpoint_id, 40)),
                Cell::from(format_datetime(&cp.created_at)),
                Cell::from(format_bytes(cp.data_size_bytes)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(40),
            Constraint::Length(20),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(
                " Checkpoints for {} ({}) ",
                truncate(instance_id, 20),
                app.checkpoints_total
            )),
    );

    f.render_widget(table, area);
}

fn draw_checkpoint_detail_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(85, 85, f.area());
    f.render_widget(Clear, area);

    let checkpoint = match &app.checkpoint_detail {
        Some(cp) => cp,
        None => return,
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Checkpoint ID:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &checkpoint.checkpoint_id,
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Instance ID:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(&checkpoint.instance_id, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Created At:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_datetime(&checkpoint.created_at),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Data:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Pretty print the JSON data
    let json_str = serde_json::to_string_pretty(&checkpoint.data)
        .unwrap_or_else(|_| format!("{:?}", checkpoint.data));

    for line in json_str.lines() {
        lines.push(Line::from(Span::styled(
            format!("  {}", line),
            Style::default().fg(Color::Cyan),
        )));
    }

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(format!(
                    " Checkpoint: {} ",
                    truncate(&checkpoint.checkpoint_id, 30)
                )),
        )
        .scroll((app.detail_scroll, 0))
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_error_popup(f: &mut Frame, error: &str) {
    let area = centered_rect(60, 20, f.area());

    f.render_widget(Clear, area);

    let error_block = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Error",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(error),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" Error "),
    )
    .wrap(Wrap { trim: false })
    .centered();

    f.render_widget(error_block, area);
}

/// Helper to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Truncate a string to a maximum length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Format bytes to human-readable size
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
