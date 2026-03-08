use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
};

use crate::app::{AddField, App, Mode, RouteStatus, ServiceField, Tab};
use crate::launchd::Status;

const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const RED: Color = Color::Red;
const YELLOW: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(5),    // table
        Constraint::Length(3), // status bar
        Constraint::Length(2), // keybindings
    ])
    .split(f.area());

    draw_header(f, app, chunks[0]);
    match app.tab {
        Tab::Tunnels => draw_tunnels_table(f, app, chunks[1]),
        Tab::Services => draw_services_table(f, app, chunks[1]),
        Tab::Routes => draw_routes_table(f, app, chunks[1]),
    }
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
        Mode::ConfirmingDelete { target } => {
            draw_confirm_dialog(f, "delete", target);
        }
        Mode::Migrating { daemon_plists } => {
            draw_migrate_dialog(f, daemon_plists.len());
        }
        Mode::Logs { name, content } => {
            draw_logs_dialog(f, name, content);
        }
        Mode::AddingService {
            field,
            name,
            port,
            machine,
        } => {
            draw_service_dialog(f, "Add Service", field, name, port, machine);
        }
        Mode::EditingService {
            field,
            name,
            port,
            machine,
            ..
        } => {
            draw_service_dialog(f, "Edit Service", field, name, port, machine);
        }
        Mode::ConfirmingServiceDelete {
            name,
            port,
            machine,
        } => {
            let label = format!("{} :{} on {}", name, port, machine);
            draw_confirm_dialog(f, "untrack", &label);
        }
        Mode::AddingApiToken { tunnel_name, input } => {
            draw_add_api_token_dialog(f, tunnel_name, input);
        }
        Mode::ContextMenu { items, selected } => {
            draw_context_menu(f, app, items, *selected);
        }
        Mode::Help => {
            draw_help(f);
        }
        Mode::Normal => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tab_badge = |num: &str, active: bool| -> Vec<Span> {
        vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {} ", num),
                Style::default().fg(Color::Black).bg(if active {
                    Color::Cyan
                } else {
                    Color::Rgb(80, 90, 100)
                }),
            ),
        ]
    };
    let tab_label = |label: &str, active: bool| -> Span {
        if active {
            Span::styled(
                format!(" {} ", label),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {} ", label), Style::default().fg(DIM))
        }
    };

    let mut spans = vec![Span::styled(
        " tunnels ",
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
    )];
    spans.extend(tab_badge("1", app.tab == Tab::Tunnels));
    spans.push(tab_label("Tunnels", app.tab == Tab::Tunnels));
    spans.extend(tab_badge("2", app.tab == Tab::Services));
    spans.push(tab_label("Services", app.tab == Tab::Services));
    spans.extend(tab_badge("3", app.tab == Tab::Routes));
    spans.push(tab_label("Routes", app.tab == Tab::Routes));

    let header = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(DIM)),
    );
    f.render_widget(header, area);
}

fn draw_tunnels_table(f: &mut Frame, app: &App, area: Rect) {
    if app.rows.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No tunnels. Press ", Style::default().fg(DIM)),
            Span::styled("a", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to add or ", Style::default().fg(DIM)),
            Span::styled("I", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to import existing.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("NAME").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("PID").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("API").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("CF NAME").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("EDGE").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (status_text, status_color) = match &row.status {
                Status::Running { pid } => {
                    let pid_str = pid.map(|p| p.to_string()).unwrap_or("-".into());
                    (("running".to_string(), GREEN), pid_str)
                }
                Status::Stopped => (("stopped".to_string(), YELLOW), "-".into()),
                Status::Inactive => (("inactive".to_string(), DIM), "-".into()),
            };

            let style = if i == app.selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let cf_conn_color = if row.cf_conns.starts_with("—") || row.cf_conns.starts_with("no ")
            {
                DIM
            } else {
                GREEN
            };

            let api_marker = if row.has_api_token { "yes" } else { "—" };
            let api_color = if row.has_api_token { GREEN } else { DIM };

            Row::new(vec![
                Cell::from(row.name.clone()),
                Cell::from(status_text.0).style(Style::default().fg(status_text.1)),
                Cell::from(status_color),
                Cell::from(api_marker).style(Style::default().fg(api_color)),
                Cell::from(row.cf_name.clone()).style(Style::default().fg(DIM)),
                Cell::from(row.cf_conns.clone()).style(Style::default().fg(cf_conn_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Length(18),
            Constraint::Min(30),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    f.render_widget(table, area);
}

fn draw_services_table(f: &mut Frame, app: &App, area: Rect) {
    if app.service_rows.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No services. Press ", Style::default().fg(DIM)),
            Span::styled("S", Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
            Span::styled(" to scan or ", Style::default().fg(DIM)),
            Span::styled("a", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to add manually.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("PROJECT").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("PORT").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("MACHINE").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("LISTENING").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .service_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (listen_text, listen_color) = if row.listening {
                ("yes", GREEN)
            } else {
                ("no", DIM)
            };

            let style = if i == app.service_selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(row.name.clone()),
                Cell::from(row.port.to_string()),
                Cell::from(row.machine.clone()).style(Style::default().fg(DIM)),
                Cell::from(listen_text).style(Style::default().fg(listen_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    f.render_widget(table, area);
}

fn draw_routes_table(f: &mut Frame, app: &App, area: Rect) {
    if app.route_rows.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled("  No routes. Press ", Style::default().fg(DIM)),
            Span::styled("R", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(" to sync from Cloudflare.", Style::default().fg(DIM)),
        ]));
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("HOSTNAME").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("PORT").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("TUNNEL").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .route_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let status_color = match row.status {
                RouteStatus::Connected => GREEN,
                RouteStatus::NoEdge => YELLOW,
                RouteStatus::Unknown => DIM,
            };

            let style = if i == app.route_selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(row.hostname.clone()).style(Style::default().fg(CYAN)),
                Cell::from(row.port.to_string()),
                Cell::from(row.tunnel_name.clone()).style(Style::default().fg(DIM)),
                Cell::from(row.status.label()).style(Style::default().fg(status_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(30),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    f.render_widget(table, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let msg = app.status_msg.as_deref().unwrap_or("");

    let status = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(msg, Style::default().fg(YELLOW)),
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
        Mode::Normal => match app.tab {
            Tab::Tunnels => vec![
                ("←/→", "tabs"),
                ("j/k", "navigate"),
                ("Enter", "actions"),
                ("a", "add"),
                ("d", "delete"),
                ("I", "import"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::Services => vec![
                ("←/→", "tabs"),
                ("j/k", "navigate"),
                ("Enter", "actions"),
                ("S", "scan"),
                ("a", "add"),
                ("d", "untrack"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::Routes => vec![
                ("←/→", "tabs"),
                ("j/k", "navigate"),
                ("R", "sync CF"),
                ("?", "help"),
                ("q", "quit"),
            ],
        },
        Mode::ContextMenu { .. } => {
            vec![("j/k", "navigate"), ("Enter", "select"), ("Esc", "close")]
        }
        Mode::AddingService { .. } | Mode::EditingService { .. } => vec![
            ("Tab", "next field"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Mode::AddingApiToken { .. } => vec![("Enter", "save"), ("Esc", "cancel")],
        Mode::Adding { .. } | Mode::Editing { .. } | Mode::Renaming { .. } => vec![
            ("Enter", "confirm"),
            ("Tab", "next field"),
            ("Esc", "cancel"),
        ],
        Mode::ConfirmingDelete { .. }
        | Mode::Migrating { .. }
        | Mode::ConfirmingServiceDelete { .. } => {
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

fn draw_context_menu(f: &mut Frame, app: &App, items: &[(char, String)], selected: usize) {
    let title = match app.tab {
        Tab::Tunnels => " Tunnel ",
        Tab::Services => " Service ",
        Tab::Routes => " Route ",
    };

    let width = 24u16;
    let height = items.len() as u16 + 2; // border top + bottom
    let area = fixed_centered_rect_abs(width, height, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(CYAN).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, (key, label))| {
            let style = if i == selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 55))
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Row::new(vec![
                Cell::from(format!(" {} ", key)).style(Style::default().fg(CYAN)),
                Cell::from(label.clone()),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(4), Constraint::Min(10)]);

    f.render_widget(table, inner);
}

// --- Dialogs (mostly unchanged) ---

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
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let token_style = if *field == AddField::Token {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
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
        format!("...{}_", &token[token.len() - 37..])
    } else {
        format!("{}_", token)
    };

    f.render_widget(
        Paragraph::new("  Paste or type the new token:").style(Style::default().fg(DIM)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!("  > {}", display)).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
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
        Paragraph::new(format!("  > {}_", new_name)).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
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
        Paragraph::new(format!(
            "  Found {} tunnel(s) in /Library/LaunchDaemons/",
            count
        ))
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
        Line::from(vec![
            Span::styled("  ←/→   ", Style::default().fg(CYAN)),
            Span::raw("Switch tabs"),
        ]),
        Line::from(vec![
            Span::styled("  j/↓   ", Style::default().fg(CYAN)),
            Span::raw("Move down"),
        ]),
        Line::from(vec![
            Span::styled("  k/↑   ", Style::default().fg(CYAN)),
            Span::raw("Move up"),
        ]),
        Line::from(vec![
            Span::styled("  Enter  ", Style::default().fg(CYAN)),
            Span::raw("Open actions menu"),
        ]),
        Line::from(""),
        Line::from(Span::styled("  — Tunnels (1) —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  a     ", Style::default().fg(CYAN)),
            Span::raw("Add new tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  d     ", Style::default().fg(RED)),
            Span::raw("Delete selected"),
        ]),
        Line::from(vec![
            Span::styled("  I     ", Style::default().fg(CYAN)),
            Span::raw("Import existing plists"),
        ]),
        Line::from(Span::styled(
            "  Enter → s/x/r/e/n/l/T/X/d",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(Span::styled("  — Services (2) —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  S     ", Style::default().fg(GREEN)),
            Span::raw("Scan listening ports"),
        ]),
        Line::from(vec![
            Span::styled("  a     ", Style::default().fg(CYAN)),
            Span::raw("Add service"),
        ]),
        Line::from(vec![
            Span::styled("  d     ", Style::default().fg(RED)),
            Span::raw("Untrack service"),
        ]),
        Line::from(Span::styled("  Enter → e/d", Style::default().fg(DIM))),
        Line::from(""),
        Line::from(Span::styled("  — Routes (3) —", Style::default().fg(DIM))),
        Line::from(vec![
            Span::styled("  R     ", Style::default().fg(CYAN)),
            Span::raw("Sync from Cloudflare API"),
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

fn draw_service_dialog(
    f: &mut Frame,
    title: &str,
    field: &ServiceField,
    name: &str,
    port: &str,
    machine: &str,
) {
    let area = fixed_centered_rect(60, 11, f.area());
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
    ])
    .split(inner);

    let field_style = |f: ServiceField, active: &ServiceField| {
        if f == *active {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        }
    };
    let cursor = |f: ServiceField, active: &ServiceField| {
        if f == *active { "_" } else { "" }
    };

    f.render_widget(
        Paragraph::new(format!(
            "  Name:    {}{}",
            name,
            cursor(ServiceField::Name, field)
        ))
        .style(field_style(ServiceField::Name, field)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(format!(
            "  Port:    {}{}",
            port,
            cursor(ServiceField::Port, field)
        ))
        .style(field_style(ServiceField::Port, field)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new(format!(
            "  Machine: {}{}",
            machine,
            cursor(ServiceField::Machine, field)
        ))
        .style(field_style(ServiceField::Machine, field)),
        chunks[5],
    );
}

fn draw_add_api_token_dialog(f: &mut Frame, tunnel_name: &str, input: &str) {
    let area = fixed_centered_rect(70, 11, f.area());
    f.render_widget(Clear, area);

    let title = format!(" API Token for '{}' ", tunnel_name);

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
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("  Paste a CF API token for this tunnel's account")
            .style(Style::default().fg(DIM)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("  Create at: dash.cloudflare.com/profile/api-tokens")
            .style(Style::default().fg(DIM)),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new("  Permission: Account > Cloudflare Tunnel > Read")
            .style(Style::default().fg(DIM)),
        chunks[4],
    );

    let display = if input.is_empty() {
        "_".to_string()
    } else if input.len() > 40 {
        format!("...{}_", &input[input.len() - 37..])
    } else {
        format!("{}_", input)
    };

    f.render_widget(
        Paragraph::new(format!("  Token: {}", display)).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        chunks[6],
    );
}

// --- Layout helpers ---

/// Centered rect using percentage width and absolute height
fn fixed_centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let x_margin = (area.width as u32 * (100 - percent_x as u32) / 100 / 2) as u16;
    let w = area.width.saturating_sub(x_margin * 2);
    Rect::new(area.x + x_margin, y, w, height.min(area.height))
}

/// Centered rect using absolute width and height
fn fixed_centered_rect_abs(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
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
