use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

use crate::app::{AddField, App, ContextAction, Mode, PrefixKey, RouteField, ServiceField, UnifiedRow};
use crate::launchd::Status;

const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const RED: Color = Color::Red;
const YELLOW: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(5),   // tree table
        Constraint::Length(3), // status bar
        Constraint::Length(2), // keybindings
    ])
    .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_unified_table(f, app, chunks[1]);
    draw_status_bar(f, app, chunks[2]);
    draw_keybindings(f, app, chunks[3]);

    // Overlays
    match &app.mode {
        Mode::Adding { field, name, token } => {
            draw_input_dialog(f, "Add Tunnel", field, name, token);
        }
        Mode::Editing { name, token } => {
            draw_edit_dialog(f, name, token);
        }
        Mode::Renaming { old_name, new_name } => {
            draw_rename_dialog(f, old_name, new_name);
        }
        Mode::Confirming { action, target } => {
            draw_confirm_dialog(f, action, target);
        }
        Mode::Migrating { daemon_plists } => {
            draw_migrate_dialog(f, daemon_plists.len());
        }
        Mode::Logs { name, content } => {
            draw_logs_dialog(f, name, content);
        }
        Mode::AddingService { field, name, port, tunnel, memo } => {
            draw_service_dialog(f, "Add Service", field, name, port, tunnel, memo);
        }
        Mode::EditingService { field, name, port, tunnel, memo, .. } => {
            draw_service_dialog(f, "Edit Service", field, name, port, tunnel, memo);
        }
        Mode::ConfirmingServiceDelete { name, port, .. } => {
            let label = format!("{} :{}", name, port);
            draw_confirm_dialog(f, "untrack", &label);
        }
        Mode::AddingApiToken { input } => {
            draw_add_api_token_dialog(f, &app.unreached, input);
        }
        Mode::Routes { tunnel_name, routes, selected, .. } => {
            draw_routes_dialog(f, tunnel_name, routes, *selected);
        }
        Mode::AddingRoute { tunnel_name, field, hostname, service, .. } => {
            draw_add_route_dialog(f, tunnel_name, field, hostname, service);
        }
        Mode::RenamingRoute { old_hostname, new_subdomain, domain_suffix, .. } => {
            draw_rename_route_dialog(f, old_hostname, new_subdomain, domain_suffix);
        }
        Mode::ConfirmingRouteDelete { hostname, .. } => {
            draw_confirm_dialog(f, "remove route", hostname);
        }
        Mode::ContextMenu { items, selected } => {
            draw_context_menu(f, app, items, *selected);
        }
        Mode::Help => {
            draw_help(f);
        }
        Mode::Normal | Mode::Prefix(_) => {}
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

fn draw_unified_table(f: &mut Frame, app: &App, area: Rect) {
    if app.unified_rows.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No tunnels or services. Press ", Style::default().fg(DIM)),
            Span::styled("a", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to add.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("NAME").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("EDGE").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("URL").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("MEMO").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .unified_rows
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

            match row {
                UnifiedRow::Tunnel { name, status, cf_name: _, cf_conns, service_count } => {
                    let indicator = if app.collapsed.contains(name) {
                        "▶"
                    } else {
                        "▼"
                    };
                    let name_display = if *service_count > 0 {
                        format!("{} {}", indicator, name)
                    } else {
                        format!("  {}", name)
                    };

                    let (status_text, status_color) = match status {
                        Status::Running { .. } => ("running", GREEN),
                        Status::Stopped => ("stopped", YELLOW),
                        Status::Inactive => ("inactive", DIM),
                    };

                    let edge_color = if cf_conns.starts_with("—") || cf_conns.starts_with("no ") {
                        DIM
                    } else {
                        GREEN
                    };

                    Row::new(vec![
                        Cell::from(name_display),
                        Cell::from(status_text).style(Style::default().fg(status_color)),
                        Cell::from(cf_conns.clone()).style(Style::default().fg(edge_color)),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                    .style(style)
                }
                UnifiedRow::Service { name, port, tunnel_status, url, memo, is_last, .. } => {
                    let prefix = if *is_last { "  └ " } else { "  ├ " };
                    let name_display = format!("{}{}", prefix, name);

                    let port_display = format!(":{}", port);

                    let status_color = match tunnel_status.as_str() {
                        "connected" | "running" => GREEN,
                        "stopped" | "no edge" => YELLOW,
                        _ => DIM,
                    };

                    let url_color = if url != "—" { CYAN } else { DIM };

                    Row::new(vec![
                        Cell::from(name_display),
                        Cell::from(port_display).style(Style::default().fg(DIM)),
                        Cell::from(tunnel_status.clone()).style(Style::default().fg(status_color)),
                        Cell::from(url.clone()).style(Style::default().fg(url_color)),
                        Cell::from(memo.clone()).style(Style::default().fg(DIM)),
                    ])
                    .style(style)
                }
                UnifiedRow::Separator => {
                    Row::new(vec![
                        Cell::from("── Unlinked ──").style(Style::default().fg(DIM)),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                    .style(Style::default())
                }
            }
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Length(16),
            Constraint::Length(30),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    f.render_widget(table, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (msg, color) = if let Some(ref loading_msg) = app.loading {
        let frame = crate::app::SPINNER[app.spinner_tick % crate::app::SPINNER.len()];
        (format!("{} {}", frame, loading_msg), CYAN)
    } else {
        (app.status_msg.as_deref().unwrap_or("").to_string(), YELLOW)
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
        Mode::Prefix(PrefixKey::Add) => vec![
            ("a ▸", "add"),
            ("t", "tunnel"),
            ("s", "service"),
            ("r", "route"),
            ("Esc", "cancel"),
        ],
        Mode::Prefix(PrefixKey::Token) => vec![
            ("t ▸", "token"),
            ("c", "connector"),
            ("a", "API token"),
            ("Esc", "cancel"),
        ],
        Mode::Prefix(PrefixKey::Global) => vec![
            ("g ▸", "global"),
            ("s", "sync CF"),
            ("p", "scan ports"),
            ("i", "import"),
            ("Esc", "cancel"),
        ],
        Mode::Normal => {
            if app.is_tunnel_selected() {
                vec![
                    ("j/k", "nav"),
                    ("s", "start"),
                    ("x", "stop"),
                    ("r", "restart"),
                    ("m", "routes"),
                    ("a", "add..."),
                    ("t", "token..."),
                    ("g", "global..."),
                    ("Enter", "actions"),
                    ("q", "quit"),
                ]
            } else {
                vec![
                    ("j/k", "nav"),
                    ("e", "edit"),
                    ("n", "rename url"),
                    ("d", "untrack"),
                    ("a", "add..."),
                    ("t", "token..."),
                    ("g", "global..."),
                    ("Enter", "actions"),
                    ("q", "quit"),
                ]
            }
        }
        Mode::ContextMenu { .. } => vec![
            ("j/k", "nav"),
            ("Enter", "select"),
            ("Esc", "cancel"),
        ],
        Mode::Routes { .. } => vec![
            ("j/k", "nav"),
            ("a", "add"),
            ("n", "rename"),
            ("d", "delete"),
            ("Esc", "back"),
        ],
        Mode::RenamingRoute { .. } => vec![
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::AddingRoute { .. } => vec![
            ("Tab", "next field"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::ConfirmingRouteDelete { .. } => vec![("y", "confirm"), ("n/Esc", "cancel")],
        Mode::AddingService { .. } | Mode::EditingService { .. } => vec![
            ("Tab", "next field"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::AddingApiToken { .. } => vec![
            ("Enter", "save"),
            ("Esc", "cancel"),
        ],
        Mode::Adding { .. } | Mode::Editing { .. } | Mode::Renaming { .. } => vec![
            ("Enter", "confirm"),
            ("Tab", "next field"),
            ("Esc", "cancel"),
        ],
        Mode::Confirming { .. } | Mode::Migrating { .. } | Mode::ConfirmingServiceDelete { .. } => vec![("y", "confirm"), ("n/Esc", "cancel")],
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

fn draw_context_menu(f: &mut Frame, app: &App, items: &[(char, String, ContextAction)], selected: usize) {
    // Determine title from selected row
    let title = match app.selected_row() {
        Some(UnifiedRow::Tunnel { name, .. }) => name.clone(),
        Some(UnifiedRow::Service { name, port, .. }) => format!("{} :{}", name, port),
        _ => "Actions".to_string(),
    };

    let width = 28u16;
    let height = (items.len() as u16) + 2; // +2 for borders

    let area = fixed_centered_rect(width, height, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let menu_rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, (key, label, _action))| {
            let style = if i == selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(format!("  {}  ", key)).style(Style::default().fg(CYAN)),
                Cell::from(label.clone()),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        menu_rows,
        [Constraint::Length(5), Constraint::Min(15)],
    );

    f.render_widget(table, inner);
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
    let area = fixed_centered_rect(60, 7, f.area());
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
    ])
    .split(inner);

    let display = if token.is_empty() {
        "_".to_string()
    } else if token.len() > 40 {
        format!("...{}_", &token[token.len()-37..])
    } else {
        format!("{}_", token)
    };

    f.render_widget(
        Paragraph::new("  Paste or type the new token:").style(Style::default().fg(DIM)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!("  > {}", display))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[2],
    );
}

fn draw_rename_dialog(f: &mut Frame, old_name: &str, new_name: &str) {
    let area = fixed_centered_rect(50, 7, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Rename '{}' ", old_name))
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
        Paragraph::new("  New name:").style(Style::default().fg(DIM)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!("  > {}_", new_name))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[2],
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

fn draw_help(f: &mut Frame) {
    let area = centered_rect(55, 70, f.area());
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
            Span::styled("  j/↓  ", Style::default().fg(CYAN)),
            Span::raw("Move down"),
        ]),
        Line::from(vec![
            Span::styled("  k/↑  ", Style::default().fg(CYAN)),
            Span::raw("Move up"),
        ]),
        Line::from(vec![
            Span::styled("  Space ", Style::default().fg(CYAN)),
            Span::raw("Expand/collapse tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  Enter ", Style::default().fg(CYAN)),
            Span::raw("Open context menu"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Tunnel keys —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  s     ", Style::default().fg(GREEN)),
            Span::raw("Start tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  x     ", Style::default().fg(RED)),
            Span::raw("Stop tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  r     ", Style::default().fg(YELLOW)),
            Span::raw("Restart tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  m     ", Style::default().fg(CYAN)),
            Span::raw("Manage routes"),
        ]),
        Line::from(vec![
            Span::styled("  l     ", Style::default().fg(CYAN)),
            Span::raw("View logs"),
        ]),
        Line::from(vec![
            Span::styled("  e     ", Style::default().fg(CYAN)),
            Span::raw("Edit connector token"),
        ]),
        Line::from(vec![
            Span::styled("  n     ", Style::default().fg(CYAN)),
            Span::raw("Rename tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  d     ", Style::default().fg(RED)),
            Span::raw("Delete tunnel"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Service keys —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  e     ", Style::default().fg(CYAN)),
            Span::raw("Edit service"),
        ]),
        Line::from(vec![
            Span::styled("  n     ", Style::default().fg(CYAN)),
            Span::raw("Rename URL"),
        ]),
        Line::from(vec![
            Span::styled("  d     ", Style::default().fg(RED)),
            Span::raw("Untrack service"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Prefix keys —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  a...  ", Style::default().fg(CYAN)),
            Span::raw("Add (t=tunnel, s=service, r=route)"),
        ]),
        Line::from(vec![
            Span::styled("  t...  ", Style::default().fg(CYAN)),
            Span::raw("Token (c=connector, a=API token)"),
        ]),
        Line::from(vec![
            Span::styled("  g...  ", Style::default().fg(CYAN)),
            Span::raw("Global (s=sync, p=scan, i=import)"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  q     ", Style::default().fg(DIM)),
            Span::raw("Quit"),
        ]),
    ];

    f.render_widget(
        Paragraph::new(help_text).style(Style::default().fg(Color::White)),
        inner,
    );
}

fn draw_routes_dialog(f: &mut Frame, tunnel_name: &str, routes: &[crate::app::RouteRow], selected: usize) {
    use crate::app::DnsStatus;

    let area = centered_rect(80, 70, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Routes: {} ", tunnel_name))
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if routes.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No routes. Press ", Style::default().fg(DIM)),
            Span::styled("a", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to add one.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, inner);
        return;
    }

    let header = Row::new(vec![
        Cell::from("HOSTNAME").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("SERVICE").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("DNS").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = routes
        .iter()
        .enumerate()
        .map(|(i, route)| {
            let is_catchall = route.hostname == "(catch-all)";
            let hostname_color = if is_catchall { DIM } else { Color::White };
            let (dns_text, dns_color) = match route.dns {
                DnsStatus::Ok => ("✓", GREEN),
                DnsStatus::Missing => ("✗ missing", RED),
                DnsStatus::Unknown => ("?", YELLOW),
            };
            let style = if i == selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(route.hostname.clone()).style(Style::default().fg(hostname_color)),
                Cell::from(route.service.clone()).style(Style::default().fg(DIM)),
                Cell::from(dns_text).style(Style::default().fg(dns_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Percentage(20),
        ],
    )
    .header(header);

    f.render_widget(table, inner);
}

fn draw_add_route_dialog(f: &mut Frame, tunnel_name: &str, field: &RouteField, hostname: &str, service: &str) {
    let area = fixed_centered_rect(65, 9, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Add Route to '{}' ", tunnel_name))
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
    ])
    .split(inner);

    let field_style = |f: RouteField, active: &RouteField| {
        if f == *active {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        }
    };
    let cursor = |f: RouteField, active: &RouteField| {
        if f == *active { "_" } else { "" }
    };

    f.render_widget(
        Paragraph::new(format!("  Hostname: {}{}", hostname, cursor(RouteField::Hostname, field)))
            .style(field_style(RouteField::Hostname, field)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(format!("  Service:  {}{}", service, cursor(RouteField::Service, field)))
            .style(field_style(RouteField::Service, field)),
        chunks[3],
    );
}

fn draw_rename_route_dialog(f: &mut Frame, old_hostname: &str, new_subdomain: &str, domain_suffix: &str) {
    let area = fixed_centered_rect(65, 7, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Rename '{}' ", old_hostname))
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
        Paragraph::new("  New subdomain:").style(Style::default().fg(DIM)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  > "),
            Span::styled(format!("{}_", new_subdomain), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(domain_suffix, Style::default().fg(DIM)),
        ])),
        chunks[2],
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
    let names_display = all_names.join(", ");

    let area = fixed_centered_rect(70, 13, f.area());
    f.render_widget(Clear, area);

    let title = format!(" Add CF API Token ({} account(s) need tokens) ", num_accounts);

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

    f.render_widget(
        Paragraph::new(format!("  Needs: {}", names_display))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("  Paste a token — we'll match it to the right account")
            .style(Style::default().fg(DIM)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new("  Create at: dash.cloudflare.com/profile/api-tokens")
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

fn draw_service_dialog(f: &mut Frame, title: &str, field: &ServiceField, name: &str, port: &str, tunnel: &str, memo: &str) {
    let area = fixed_centered_rect(60, 13, f.area());
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
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let field_style = |f: ServiceField, active: &ServiceField| {
        if f == *active {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        }
    };
    let cursor = |f: ServiceField, active: &ServiceField| {
        if f == *active { "_" } else { "" }
    };

    f.render_widget(
        Paragraph::new(format!("  Name:    {}{}", name, cursor(ServiceField::Name, field)))
            .style(field_style(ServiceField::Name, field)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(format!("  Port:    {}{}", port, cursor(ServiceField::Port, field)))
            .style(field_style(ServiceField::Port, field)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new(format!("  Tunnel:  {}{}", tunnel, cursor(ServiceField::Tunnel, field)))
            .style(field_style(ServiceField::Tunnel, field)),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new(format!("  Memo:    {}{}", memo, cursor(ServiceField::Memo, field)))
            .style(field_style(ServiceField::Memo, field)),
        chunks[7],
    );
}

fn fixed_centered_rect(width_or_pct: u16, height: u16, area: Rect) -> Rect {
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let x_margin = if width_or_pct > 100 {
        // treat as absolute width
        (area.width.saturating_sub(width_or_pct)) / 2
    } else {
        // treat as percentage
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
