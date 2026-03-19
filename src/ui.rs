use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::app::{AddField, AddPortField, App, Health, Mode, SettingsItemKind};

const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const RED: Color = Color::Red;
const YELLOW: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Min(5),   // port list
        Constraint::Length(3), // status bar
        Constraint::Length(1), // keybindings
    ])
    .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_port_list(f, app, chunks[1]);
    draw_status_bar(f, app, chunks[2]);
    draw_keybindings(f, app, chunks[3]);

    // Overlays
    match &app.mode {
        Mode::Linking { port, name: _, hostname, tunnel_name, old_hostname } => {
            let title = if old_hostname.is_some() {
                format!(" Edit :{} ", port)
            } else {
                format!(" Link :{} ", port)
            };
            draw_link_dialog(f, &title, tunnel_name, hostname);
        }
        Mode::ConfirmingUnlink { hostname, .. } => {
            draw_confirm_dialog(f, "unlink", hostname);
        }
        Mode::AddingPort { field, port, name } => {
            draw_add_port_dialog(f, field, port, name);
        }
        Mode::Settings { items, selected } => {
            draw_settings_modal(f, items, *selected);
        }
        Mode::Adding { field, name, token } => {
            draw_input_dialog(f, "Add Tunnel", field, name, token);
        }
        Mode::Editing { name, token } => {
            draw_edit_dialog(f, name, token);
        }
        Mode::AddingApiToken { input } => {
            draw_add_api_token_dialog(f, &app.unreached, input);
        }
        Mode::Confirming { action, target } => {
            draw_confirm_dialog(f, action, target);
        }
        Mode::ConfirmingServiceDelete { name, port, .. } => {
            let label = format!("{} :{}", name, port);
            draw_confirm_dialog(f, "remove", &label);
        }
        Mode::Migrating { daemon_plists } => {
            draw_migrate_dialog(f, daemon_plists.len());
        }
        Mode::Logs { name, content } => {
            draw_logs_dialog(f, name, content);
        }
        Mode::Help => {
            draw_help(f);
        }
        Mode::Normal => {}
    }
}

fn draw_header(f: &mut Frame, _app: &App, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(" tunnels ", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(DIM)),
    );
    f.render_widget(header, area);
}

fn draw_port_list(f: &mut Frame, app: &App, area: Rect) {
    if app.rows.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No services. Press ", Style::default().fg(DIM)),
            Span::styled("a", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to add, ", Style::default().fg(DIM)),
            Span::styled(".", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" for settings.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let style = if i == app.selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let port_display = format!(":{}", row.port);

            let (glyph, glyph_color) = match row.health {
                Health::Healthy => ("✓", GREEN),
                Health::Unhealthy => ("✗", RED),
                Health::Active => ("●", YELLOW),
            };

            let url_display = row.url.as_deref().unwrap_or("—");
            let url_color = if row.url.is_some() { CYAN } else { DIM };

            let memo_display = row.memo.as_deref().unwrap_or("");

            Row::new(vec![
                Cell::from(port_display).style(Style::default().fg(DIM)),
                Cell::from(row.name.clone()),
                Cell::from(url_display.to_string()).style(Style::default().fg(url_color)),
                Cell::from(memo_display.to_string()).style(Style::default().fg(DIM)),
                Cell::from(glyph).style(Style::default().fg(glyph_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(20),
            Constraint::Min(30),
            Constraint::Length(16),
            Constraint::Length(3),
        ],
    )
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    let mut state = TableState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (msg, color) = if let Some(ref loading_msg) = app.loading {
        let frame = crate::app::SPINNER[app.spinner_tick % crate::app::SPINNER.len()];
        (format!("{} {}", frame, loading_msg), CYAN)
    } else if let Some(ref status) = app.status_msg {
        let sync_ago = app.last_sync.map(|t| {
            let secs = t.elapsed().as_secs();
            if secs < 60 { format!("{}s ago", secs) }
            else { format!("{}m ago", secs / 60) }
        });
        let right = sync_ago.map(|s| format!("  synced {}", s)).unwrap_or_default();
        (format!("{}{}", status, right), YELLOW)
    } else {
        let sync_ago = app.last_sync.map(|t| {
            let secs = t.elapsed().as_secs();
            if secs < 60 { format!("synced {}s ago", secs) }
            else { format!("synced {}m ago", secs / 60) }
        });
        (sync_ago.unwrap_or_default(), DIM)
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(msg, Style::default().fg(color)),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(DIM)),
    );
    f.render_widget(status, area);
}

fn draw_keybindings(f: &mut Frame, app: &App, area: Rect) {
    let keys: Vec<(&str, &str)> = match &app.mode {
        Mode::Normal => vec![
            ("j/k", "nav"),
            ("Enter", "link"),
            ("d", "unlink"),
            ("a", "add"),
            ("l", "logs"),
            (".", "settings"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Mode::Settings { .. } => vec![
            ("j/k", "nav"),
            ("Enter", "select"),
            ("d", "remove"),
            ("Esc", "close"),
        ],
        Mode::Linking { .. } => vec![
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::AddingPort { .. } => vec![
            ("Tab", "next"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::AddingApiToken { .. } => vec![
            ("Enter", "save"),
            ("Esc", "cancel"),
        ],
        Mode::Adding { .. } | Mode::Editing { .. } => vec![
            ("Tab", "next"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::Confirming { .. } | Mode::Migrating { .. }
        | Mode::ConfirmingServiceDelete { .. } | Mode::ConfirmingUnlink { .. } => {
            vec![("y", "confirm"), ("n/Esc", "cancel")]
        }
        Mode::Logs { .. } | Mode::Help => vec![("Esc/q", "close")],
    };

    let spans: Vec<Span> = keys
        .iter()
        .flat_map(|(key, desc)| {
            vec![
                Span::styled(
                    format!(" {} ", key),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Rgb(80, 90, 100)),
                ),
                Span::styled(format!(" {} ", desc), Style::default().fg(DIM)),
            ]
        })
        .collect();

    let bar = Paragraph::new(Line::from(spans));
    f.render_widget(bar, area);
}

// --- Dialogs ---

fn draw_link_dialog(f: &mut Frame, title: &str, tunnel_name: &str, hostname: &str) {
    let area = fixed_centered_rect(55, 7, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(format!("  Tunnel:   {}", tunnel_name))
            .style(Style::default().fg(DIM)),
        chunks[0],
    );

    let display = if hostname.is_empty() {
        "_".to_string()
    } else {
        format!("{}_", hostname)
    };

    f.render_widget(
        Paragraph::new(format!("  Hostname: {}", display))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[2],
    );
}

fn draw_add_port_dialog(f: &mut Frame, field: &AddPortField, port: &str, name: &str) {
    let area = fixed_centered_rect(50, 7, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Add Service ")
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let port_style = if *field == AddPortField::Port {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let name_style = if *field == AddPortField::Name {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let cursor_port = if *field == AddPortField::Port { "_" } else { "" };
    let cursor_name = if *field == AddPortField::Name { "_" } else { "" };

    f.render_widget(
        Paragraph::new(format!("  Port: {}{}", port, cursor_port)).style(port_style),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!("  Name: {}{}", name, cursor_name)).style(name_style),
        chunks[2],
    );
}

fn draw_settings_modal(f: &mut Frame, items: &[crate::app::SettingsItem], selected: usize) {
    let height = (items.len() as u16 + 5).min(f.area().height - 4);
    let area = fixed_centered_rect(50, height, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Settings ")
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_selected = i == selected;

            match &item.kind {
                SettingsItemKind::AccountHeader(_) => {
                    let line = Line::from(vec![
                        Span::styled(&item.label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                    ]);
                    Row::new(vec![Cell::from(line)])
                }
                SettingsItemKind::Spacer => {
                    Row::new(vec![Cell::from("")])
                }
                SettingsItemKind::ApiKey(_) => {
                    let base = if is_selected {
                        Style::default().bg(Color::Rgb(30, 40, 55)).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let detail_color = if item.detail == "(none)" { RED } else { DIM };
                    let line = Line::from(vec![
                        Span::styled(prefix, base),
                        Span::styled(&item.label, Style::default().fg(DIM)),
                        Span::styled("   ", base),
                        Span::styled(&item.detail, Style::default().fg(detail_color)),
                    ]);
                    Row::new(vec![Cell::from(line)]).style(base)
                }
                SettingsItemKind::Tunnel(_) => {
                    let base = if is_selected {
                        Style::default().bg(Color::Rgb(30, 40, 55)).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let status_color = match item.detail.as_str() {
                        "running" => GREEN,
                        "stopped" => YELLOW,
                        _ => DIM,
                    };
                    let line = Line::from(vec![
                        Span::styled(prefix, base),
                        Span::styled(&item.label, base),
                        Span::styled("   ", base),
                        Span::styled(&item.detail, Style::default().fg(status_color)),
                    ]);
                    Row::new(vec![Cell::from(line)]).style(base)
                }
                SettingsItemKind::AddAccount => {
                    let base = if is_selected {
                        Style::default().bg(Color::Rgb(30, 40, 55)).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(DIM)
                    };
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let line = Line::from(vec![
                        Span::styled(prefix, base),
                        Span::styled(&item.label, base),
                    ]);
                    Row::new(vec![Cell::from(line)]).style(base)
                }
                _ => {
                    // Action items
                    let base = if is_selected {
                        Style::default().bg(Color::Rgb(30, 40, 55)).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let line = Line::from(vec![
                        Span::styled(prefix, base),
                        Span::styled(&item.label, base),
                    ]);
                    Row::new(vec![Cell::from(line)]).style(base)
                }
            }
        })
        .collect();

    let table = Table::new(rows, [Constraint::Min(40)]);
    f.render_widget(table, chunks[0]);

    let hint_bar = Line::from(vec![
        Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Rgb(80, 90, 100))),
        Span::styled(" edit ", Style::default().fg(DIM)),
        Span::styled(" a ", Style::default().fg(Color::Black).bg(Color::Rgb(80, 90, 100))),
        Span::styled(" add ", Style::default().fg(DIM)),
        Span::styled(" d ", Style::default().fg(Color::Black).bg(Color::Rgb(80, 90, 100))),
        Span::styled(" remove ", Style::default().fg(DIM)),
        Span::styled(" Esc ", Style::default().fg(Color::Black).bg(Color::Rgb(80, 90, 100))),
        Span::styled(" close ", Style::default().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(hint_bar), chunks[1]);
}

fn draw_input_dialog(f: &mut Frame, title: &str, field: &AddField, name: &str, token: &str) {
    let area = fixed_centered_rect(55, 9, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let name_style = if *field == AddField::Name {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let token_style = if *field == AddField::Token {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let cursor_name = if *field == AddField::Name { "_" } else { "" };
    let cursor_token = if *field == AddField::Token { "_" } else { "" };

    f.render_widget(
        Paragraph::new(format!("  Name:  {}{}", name, cursor_name)).style(name_style),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(format!("  Token: {}{}", token, cursor_token)).style(token_style),
        chunks[3],
    );
}

fn draw_edit_dialog(f: &mut Frame, name: &str, token: &str) {
    let area = fixed_centered_rect(68, 13, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Edit '{}' ", name))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("  Paste the connector token for this tunnel")
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("  Find it at: https://one.dash.cloudflare.com/networks/tunnels")
            .style(Style::default().fg(DIM)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new("  Select tunnel → Configure → Install connector → copy token")
            .style(Style::default().fg(DIM)),
        chunks[5],
    );

    let display = if token.is_empty() {
        "_".to_string()
    } else if token.len() > 40 {
        format!("...{}_", &token[token.len()-37..])
    } else {
        format!("{}_", token)
    };

    f.render_widget(
        Paragraph::new(format!("  Token: {}", display))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[8],
    );
}

fn draw_confirm_dialog(f: &mut Frame, action: &str, target: &str) {
    let area = fixed_centered_rect(45, 5, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} '{}' ? ", action, target))
        .title_style(Style::default().fg(RED).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RED));

    let inner = block.inner(area);
    f.render_widget(block, area);

    f.render_widget(
        Paragraph::new("  Press y to confirm, n or Esc to cancel")
            .style(Style::default().fg(YELLOW)),
        inner,
    );
}

fn draw_migrate_dialog(f: &mut Frame, count: usize) {
    let area = fixed_centered_rect(60, 9, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Migrate to user-level? ")
        .title_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(format!("  Found {} tunnel(s) in /Library/LaunchDaemons/", count))
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new("  Migrate to ~/Library/LaunchAgents? (no more sudo)")
            .style(Style::default().fg(DIM)),
        chunks[2],
    );
    f.render_widget(
        Paragraph::new("  This will sudo unload + remove the old plists.")
            .style(Style::default().fg(YELLOW)),
        chunks[4],
    );
}

fn draw_logs_dialog(f: &mut Frame, name: &str, content: &str) {
    let area = centered_rect(85, 80, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Logs: {} ", name))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = if content.is_empty() {
        "  No logs found.".to_string()
    } else {
        content.to_string()
    };

    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::White)),
        inner,
    );
}

fn draw_add_api_token_dialog(
    f: &mut Frame,
    unreached: &[crate::cloudflare::UnreachedAccount],
    input: &str,
) {
    let num_accounts = unreached.len();
    let all_names: Vec<String> = unreached.iter()
        .flat_map(|a| a.tunnel_names.iter().cloned())
        .collect();

    let area = fixed_centered_rect(70, 13, f.area());
    f.render_widget(Clear, area);

    let title = if num_accounts > 0 {
        format!(" Add CF API Token ({} account(s) need tokens) ", num_accounts)
    } else {
        " Add CF API Token ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    if num_accounts > 0 {
        let names_display = all_names.join(", ");
        f.render_widget(
            Paragraph::new(format!("  Needs: {}", names_display))
                .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            chunks[1],
        );
    } else {
        f.render_widget(
            Paragraph::new("  Add or replace a Cloudflare API token")
                .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            chunks[1],
        );
    }
    f.render_widget(
        Paragraph::new("  Paste a token — we'll validate it against your tunnels")
            .style(Style::default().fg(DIM)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new("  Create at: https://dash.cloudflare.com/profile/api-tokens")
            .style(Style::default().fg(DIM)),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new("  Permissions: Account > Tunnel > Read/Edit, Zone > DNS > Edit")
            .style(Style::default().fg(DIM)),
        chunks[6],
    );

    let display = if input.is_empty() {
        "_".to_string()
    } else if input.len() > 40 {
        format!("...{}_", &input[input.len() - 37..])
    } else {
        format!("{}_", input)
    };

    f.render_widget(
        Paragraph::new(format!("  Token: {}", display))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[8],
    );
}

fn draw_help(f: &mut Frame) {
    let area = centered_rect(50, 60, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Help ")
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled("  — Navigation —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  j/↓    ", Style::default().fg(CYAN)),
            Span::raw("Move down"),
        ]),
        Line::from(vec![
            Span::styled("  k/↑    ", Style::default().fg(CYAN)),
            Span::raw("Move up"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Actions —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  Enter  ", Style::default().fg(GREEN)),
            Span::raw("Link port to URL (or edit existing)"),
        ]),
        Line::from(vec![
            Span::styled("  d      ", Style::default().fg(RED)),
            Span::raw("Unlink / remove service"),
        ]),
        Line::from(vec![
            Span::styled("  a      ", Style::default().fg(CYAN)),
            Span::raw("Add a service"),
        ]),
        Line::from(vec![
            Span::styled("  l      ", Style::default().fg(CYAN)),
            Span::raw("View tunnel logs"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Other —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  .      ", Style::default().fg(CYAN)),
            Span::raw("Settings (tokens, tunnels, scan)"),
        ]),
        Line::from(vec![
            Span::styled("  ?      ", Style::default().fg(CYAN)),
            Span::raw("This help"),
        ]),
        Line::from(vec![
            Span::styled("  q      ", Style::default().fg(DIM)),
            Span::raw("Quit"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Health —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  ✓      ", Style::default().fg(GREEN)),
            Span::raw("Linked, tunnel connected"),
        ]),
        Line::from(vec![
            Span::styled("  ✗      ", Style::default().fg(RED)),
            Span::raw("Linked but unhealthy"),
        ]),
        Line::from(vec![
            Span::styled("  ●      ", Style::default().fg(YELLOW)),
            Span::raw("Not linked"),
        ]),
    ];

    f.render_widget(
        Paragraph::new(help_text).style(Style::default().fg(Color::White)),
        inner,
    );
}

fn fixed_centered_rect(width_or_pct: u16, height: u16, area: Rect) -> Rect {
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let x_margin = if width_or_pct > 100 {
        (area.width.saturating_sub(width_or_pct)) / 2
    } else {
        (area.width as u32 * (100 - width_or_pct as u32) / 100 / 2) as u16
    };
    let w = area.width.saturating_sub(x_margin * 2);
    Rect::new(area.x + x_margin, y, w, height.min(area.height))
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
