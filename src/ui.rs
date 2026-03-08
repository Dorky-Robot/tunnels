use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

use crate::app::{AddField, App, Mode};
use crate::launchd::Status;

const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const RED: Color = Color::Red;
const YELLOW: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(5),   // table
        Constraint::Length(3), // status bar
        Constraint::Length(2), // keybindings
    ])
    .split(f.area());

    draw_header(f, chunks[0]);
    draw_table(f, app, chunks[1]);
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
        Mode::Logs { name, content } => {
            draw_logs_dialog(f, name, content);
        }
        Mode::Help => {
            draw_help(f);
        }
        Mode::Normal => {}
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(" tunnels ", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled("— cloudflared tunnel manager", Style::default().fg(DIM)),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(DIM)),
    );
    f.render_widget(header, area);
}

fn draw_table(f: &mut Frame, app: &App, area: Rect) {
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
        Cell::from("TUNNEL ID").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Cell::from("TOKEN").style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
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

            Row::new(vec![
                Cell::from(row.name.clone()),
                Cell::from(status_text.0).style(Style::default().fg(status_text.1)),
                Cell::from(status_color),
                Cell::from(short_id(&row.tunnel_id)).style(Style::default().fg(DIM)),
                Cell::from(row.token_preview.clone()).style(Style::default().fg(DIM)),
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
            Constraint::Length(14),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Rgb(30, 40, 55)));

    f.render_widget(table, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let msg = app
        .status_msg
        .as_deref()
        .unwrap_or("");

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
    let keys = match &app.mode {
        Mode::Normal => vec![
            ("j/k", "navigate"),
            ("s", "start"),
            ("x", "stop"),
            ("r", "restart"),
            ("a", "add"),
            ("e", "edit token"),
            ("n", "rename"),
            ("d", "delete"),
            ("l", "logs"),
            ("I", "import"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Mode::Adding { .. } | Mode::Editing { .. } | Mode::Renaming { .. } => vec![
            ("Enter", "confirm"),
            ("Tab", "next field"),
            ("Esc", "cancel"),
        ],
        Mode::Confirming { .. } => vec![("y", "confirm"), ("n/Esc", "cancel")],
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
        Line::from(vec![
            Span::styled("  j/↓  ", Style::default().fg(CYAN)),
            Span::raw("Move down"),
        ]),
        Line::from(vec![
            Span::styled("  k/↑  ", Style::default().fg(CYAN)),
            Span::raw("Move up"),
        ]),
        Line::from(""),
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
        Line::from(""),
        Line::from(vec![
            Span::styled("  a     ", Style::default().fg(CYAN)),
            Span::raw("Add new tunnel"),
        ]),
        Line::from(vec![
            Span::styled("  e     ", Style::default().fg(CYAN)),
            Span::raw("Edit selected token"),
        ]),
        Line::from(vec![
            Span::styled("  d     ", Style::default().fg(RED)),
            Span::raw("Delete selected tunnel"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  l     ", Style::default().fg(CYAN)),
            Span::raw("View logs"),
        ]),
        Line::from(vec![
            Span::styled("  I     ", Style::default().fg(CYAN)),
            Span::raw("Import existing plists"),
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

fn fixed_centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let x_margin = (area.width as u32 * (100 - percent_x as u32) / 100 / 2) as u16;
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

fn short_id(id: &str) -> String {
    if id.len() > 8 {
        format!("{}...", &id[..8])
    } else {
        id.to_string()
    }
}
