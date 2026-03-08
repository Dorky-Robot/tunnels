mod app;
mod cloudflare;
mod config;
mod launchd;
mod scan;
mod ui;

use anyhow::Result;
use app::{AddField, App, Mode, ServiceField, Tab};
use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io::stdout;
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "list" | "ls" => return cli_list(),
            "import" => return cli_import(),
            "help" | "--help" | "-h" => {
                println!("tunnels — cloudflared tunnel manager");
                println!();
                println!("  tunnels          Launch TUI");
                println!("  tunnels list     List tunnels");
                println!("  tunnels import   Import existing plists");
                return Ok(());
            }
            _ => {}
        }
    }

    let mut app = App::new();

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match &app.mode {
                Mode::Normal => match app.tab {
                    Tab::Tunnels => handle_tunnels(app, key.code),
                    Tab::Services => handle_services(app, key.code),
                    Tab::Routes => handle_routes(app, key.code),
                },
                Mode::ContextMenu { .. } => handle_context_menu(app, key.code),
                Mode::Adding { .. } => handle_adding(app, key.code),
                Mode::Editing { .. } => handle_editing(app, key.code),
                Mode::Renaming { .. } => handle_renaming(app, key.code),
                Mode::ConfirmingDelete { .. } => handle_confirming(app, key.code),
                Mode::Migrating { .. } => handle_migrating(app, key.code),
                Mode::AddingService { .. } => handle_adding_service(app, key.code),
                Mode::EditingService { .. } => handle_editing_service(app, key.code),
                Mode::ConfirmingServiceDelete { .. } => {
                    handle_confirming_service_delete(app, key.code)
                }
                Mode::AddingApiToken { .. } => handle_adding_api_token(app, key.code),
                Mode::Logs { .. } | Mode::Help => {
                    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                        app.mode = Mode::Normal;
                    }
                }
            }
        }

        // Check for completed background CF sync
        app.poll_cf_sync();

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Global keybindings shared across all tabs (quit, tab switch, navigation, help)
fn handle_global(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
            true
        }
        KeyCode::Char('1') => {
            app.tab = Tab::Tunnels;
            true
        }
        KeyCode::Char('2') => {
            app.tab = Tab::Services;
            true
        }
        KeyCode::Char('3') => {
            app.tab = Tab::Routes;
            true
        }
        KeyCode::Right => {
            app.next_tab();
            true
        }
        KeyCode::Left => {
            app.prev_tab();
            true
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_down();
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_up();
            true
        }
        KeyCode::Char('?') => {
            app.mode = Mode::Help;
            true
        }
        _ => false,
    }
}

fn handle_tunnels(app: &mut App, code: KeyCode) {
    if handle_global(app, code) {
        return;
    }
    match code {
        KeyCode::Enter => app.open_context_menu(),
        KeyCode::Char('a') => app.begin_add(),
        KeyCode::Char('d') => app.confirm_delete(),
        KeyCode::Char('R') => app.refresh_cf(),
        KeyCode::Char('I') => app.import_existing(),
        _ => {}
    }
}

fn handle_services(app: &mut App, code: KeyCode) {
    if handle_global(app, code) {
        return;
    }
    match code {
        KeyCode::Enter => app.open_context_menu(),
        KeyCode::Char('a') => app.begin_add_service(),
        KeyCode::Char('d') => app.confirm_delete_service(),
        KeyCode::Char('S') => app.scan_services(),
        _ => {}
    }
}

fn handle_routes(app: &mut App, code: KeyCode) {
    if handle_global(app, code) {
        return;
    }
    if let KeyCode::Char('R') = code {
        app.refresh_cf();
    }
}

fn handle_context_menu(app: &mut App, code: KeyCode) {
    let Mode::ContextMenu { items, selected } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Char('j') | KeyCode::Down => {
            if *selected < items.len() - 1 {
                *selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if *selected > 0 {
                *selected -= 1;
            }
        }
        KeyCode::Enter => {
            let key = items[*selected].0;
            app.execute_context_action(key);
        }
        KeyCode::Char(c) => {
            if items.iter().any(|(k, _)| *k == c) {
                app.execute_context_action(c);
            }
        }
        _ => {}
    }
}

fn handle_adding_api_token(app: &mut App, code: KeyCode) {
    let Mode::AddingApiToken { tunnel_name, input } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !input.is_empty() {
                let name = tunnel_name.clone();
                let token = input.clone();
                app.finish_add_api_token(name, token);
            }
        }
        KeyCode::Backspace => {
            input.pop();
        }
        KeyCode::Char(c) => {
            input.push(c);
        }
        _ => {}
    }
}

/// Shared handler for service form fields (add and edit use the same input logic)
fn handle_service_form(
    field: &mut ServiceField,
    name: &mut String,
    port: &mut String,
    machine: &mut String,
    code: KeyCode,
) -> bool {
    match code {
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                ServiceField::Name => ServiceField::Port,
                ServiceField::Port => ServiceField::Machine,
                ServiceField::Machine => ServiceField::Name,
            };
            true
        }
        KeyCode::Backspace => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => port,
                ServiceField::Machine => machine,
            };
            s.pop();
            true
        }
        KeyCode::Char(c) => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => {
                    if c.is_ascii_digit() {
                        port
                    } else {
                        return true;
                    }
                }
                ServiceField::Machine => machine,
            };
            s.push(c);
            true
        }
        _ => false,
    }
}

fn handle_adding_service(app: &mut App, code: KeyCode) {
    let Mode::AddingService {
        field,
        name,
        port,
        machine,
    } = &mut app.mode
    else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() && !machine.is_empty() {
                let (n, p, m) = (name.clone(), port.clone(), machine.clone());
                app.finish_add_service(n, p, m);
            }
        }
        _ => {
            handle_service_form(field, name, port, machine, code);
        }
    }
}

fn handle_editing_service(app: &mut App, code: KeyCode) {
    let Mode::EditingService {
        idx,
        field,
        name,
        port,
        machine,
    } = &mut app.mode
    else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() && !machine.is_empty() {
                let (i, n, p, m) = (*idx, name.clone(), port.clone(), machine.clone());
                app.finish_edit_service(i, n, p, m);
            }
        }
        _ => {
            handle_service_form(field, name, port, machine, code);
        }
    }
}

fn handle_confirming_service_delete(app: &mut App, code: KeyCode) {
    let Mode::ConfirmingServiceDelete {
        name,
        port,
        machine,
    } = &app.mode
    else {
        return;
    };
    let (name, port, machine) = (name.clone(), *port, machine.clone());

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.delete_service(&name, port, &machine);
            app.mode = Mode::Normal;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.mode = Mode::Normal,
        _ => {}
    }
}

fn handle_adding(app: &mut App, code: KeyCode) {
    let Mode::Adding { field, name, token } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab => {
            *field = match field {
                AddField::Name => AddField::Token,
                AddField::Token => AddField::Name,
            };
        }
        KeyCode::Enter => {
            if *field == AddField::Name && !name.is_empty() {
                *field = AddField::Token;
            } else if *field == AddField::Token && !name.is_empty() && !token.is_empty() {
                let n = name.clone();
                let t = token.clone();
                app.finish_add(n, t);
            }
        }
        KeyCode::Backspace => match field {
            AddField::Name => {
                name.pop();
            }
            AddField::Token => {
                token.pop();
            }
        },
        KeyCode::Char(c) => match field {
            AddField::Name => name.push(c),
            AddField::Token => token.push(c),
        },
        _ => {}
    }
}

fn handle_editing(app: &mut App, code: KeyCode) {
    let Mode::Editing { name, token } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !token.is_empty() {
                let n = name.clone();
                let t = token.clone();
                app.finish_edit(n, t);
            }
        }
        KeyCode::Backspace => {
            token.pop();
        }
        KeyCode::Char(c) => {
            token.push(c);
        }
        _ => {}
    }
}

fn handle_renaming(app: &mut App, code: KeyCode) {
    let Mode::Renaming { old_name, new_name } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !new_name.is_empty() {
                let o = old_name.clone();
                let n = new_name.clone();
                app.finish_rename(o, n);
            }
        }
        KeyCode::Backspace => {
            new_name.pop();
        }
        KeyCode::Char(c) => {
            new_name.push(c);
        }
        _ => {}
    }
}

fn handle_migrating(app: &mut App, code: KeyCode) {
    let Mode::Migrating { daemon_plists } = &app.mode else {
        return;
    };
    let plists = daemon_plists.clone();

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.do_migrate(plists),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.status_msg = Some("Imported — old daemon plists left in place".into());
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn handle_confirming(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.delete_selected();
            app.mode = Mode::Normal;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.mode = Mode::Normal,
        _ => {}
    }
}

fn cli_list() -> Result<()> {
    let config = config::Config::load()?;
    if config.tunnels.is_empty() {
        println!("No tunnels configured.");
        return Ok(());
    }

    println!("{:<18} {:<10} {:<10} TUNNEL ID", "NAME", "STATUS", "PID");
    println!(
        "{:<18} {:<10} {:<10} ──────────────",
        "──────────────────", "──────────", "──────────"
    );

    for t in &config.tunnels {
        let status = launchd::status(&t.name);
        let (status_str, pid_str) = match &status {
            launchd::Status::Running { pid } => {
                ("running", pid.map(|p| p.to_string()).unwrap_or("-".into()))
            }
            launchd::Status::Stopped => ("stopped", "-".into()),
            launchd::Status::Inactive => ("inactive", "-".into()),
        };
        let tunnel_id = if t.tunnel_id.is_empty() {
            "-"
        } else {
            &t.tunnel_id
        };
        println!(
            "{:<18} {:<10} {:<10} {}",
            t.name, status_str, pid_str, tunnel_id
        );
    }
    Ok(())
}

fn cli_import() -> Result<()> {
    let mut config = config::Config::load()?;
    let found = launchd::discover_existing();
    let mut count = 0;
    for d in found {
        if !config.tunnels.iter().any(|t| t.name == d.name) {
            println!("  Imported '{}'", d.name);
            config.add(d.name, d.token)?;
            count += 1;
        }
    }
    println!("{} tunnel(s) imported.", count);
    Ok(())
}
