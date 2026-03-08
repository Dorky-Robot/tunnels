mod app;
mod cloudflare;
mod config;
mod launchd;
mod scan;
mod ui;

use anyhow::Result;
use app::{AddField, App, Mode, ServiceField, Tab};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::time::Duration;

fn main() -> Result<()> {
    // Handle CLI args for non-interactive use
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

    // TUI mode
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match &app.mode {
                    Mode::Normal if app.tab == Tab::Services => handle_services_normal(app, key.code),
                    Mode::Normal => handle_normal(app, key.code),
                    Mode::Adding { .. } => handle_adding(app, key.code),
                    Mode::Editing { .. } => handle_editing(app, key.code),
                    Mode::Renaming { .. } => handle_renaming(app, key.code),
                    Mode::Confirming { .. } => handle_confirming(app, key.code),
                    Mode::Migrating { .. } => handle_migrating(app, key.code),
                    Mode::AddingService { .. } => handle_adding_service(app, key.code),
                    Mode::EditingService { .. } => handle_editing_service(app, key.code),
                    Mode::ConfirmingServiceDelete { .. } => handle_confirming_service_delete(app, key.code),
                    Mode::AddingApiToken { .. } => handle_adding_api_token(app, key.code),
                    Mode::Logs { .. } | Mode::Help => {
                        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                            app.mode = Mode::Normal;
                        }
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_normal(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('1') => app.tab = Tab::Tunnels,
        KeyCode::Char('2') => app.tab = Tab::Services,
        KeyCode::Char('j') | KeyCode::Down => app.move_down(),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(),
        KeyCode::Char('s') => app.start_selected(),
        KeyCode::Char('x') => app.stop_selected(),
        KeyCode::Char('r') => app.restart_selected(),
        KeyCode::Char('a') => app.begin_add(),
        KeyCode::Char('e') => app.begin_edit(),
        KeyCode::Char('n') => app.begin_rename(),
        KeyCode::Char('d') => app.confirm_delete(),
        KeyCode::Char('l') | KeyCode::Enter => app.show_logs(),
        KeyCode::Char('R') => app.refresh_cf(),
        KeyCode::Char('I') => app.import_existing(),
        KeyCode::Char('T') => app.begin_add_api_token(),
        KeyCode::Char('?') => app.mode = Mode::Help,
        _ => {}
    }
}

fn handle_services_normal(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('1') => app.tab = Tab::Tunnels,
        KeyCode::Char('2') => app.tab = Tab::Services,
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.service_rows.is_empty() && app.service_selected < app.service_rows.len() - 1 {
                app.service_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.service_selected > 0 {
                app.service_selected -= 1;
            }
        }
        KeyCode::Char('a') => app.begin_add_service(),
        KeyCode::Char('e') => app.begin_edit_service(),
        KeyCode::Char('d') => app.confirm_delete_service(),
        KeyCode::Char('S') => app.scan_services(),
        KeyCode::Char('R') => app.refresh_cf(),
        KeyCode::Char('T') => app.begin_add_api_token(),
        KeyCode::Char('?') => app.mode = Mode::Help,
        _ => {}
    }
}

fn handle_adding_service(app: &mut App, code: KeyCode) {
    let Mode::AddingService { field, name, port, machine, tunnel } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                ServiceField::Name => ServiceField::Port,
                ServiceField::Port => ServiceField::Machine,
                ServiceField::Machine => ServiceField::Tunnel,
                ServiceField::Tunnel => ServiceField::Name,
            };
        }
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() && !machine.is_empty() {
                let (n, p, m, t) = (name.clone(), port.clone(), machine.clone(), tunnel.clone());
                app.finish_add_service(n, p, m, t);
            }
        }
        KeyCode::Backspace => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => port,
                ServiceField::Machine => machine,
                ServiceField::Tunnel => tunnel,
            };
            s.pop();
        }
        KeyCode::Char(c) => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => {
                    if c.is_ascii_digit() { port } else { return; }
                }
                ServiceField::Machine => machine,
                ServiceField::Tunnel => tunnel,
            };
            s.push(c);
        }
        _ => {}
    }
}

fn handle_editing_service(app: &mut App, code: KeyCode) {
    let Mode::EditingService { idx, field, name, port, machine, tunnel } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                ServiceField::Name => ServiceField::Port,
                ServiceField::Port => ServiceField::Machine,
                ServiceField::Machine => ServiceField::Tunnel,
                ServiceField::Tunnel => ServiceField::Name,
            };
        }
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() && !machine.is_empty() {
                let (i, n, p, m, t) = (*idx, name.clone(), port.clone(), machine.clone(), tunnel.clone());
                app.finish_edit_service(i, n, p, m, t);
            }
        }
        KeyCode::Backspace => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => port,
                ServiceField::Machine => machine,
                ServiceField::Tunnel => tunnel,
            };
            s.pop();
        }
        KeyCode::Char(c) => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => {
                    if c.is_ascii_digit() { port } else { return; }
                }
                ServiceField::Machine => machine,
                ServiceField::Tunnel => tunnel,
            };
            s.push(c);
        }
        _ => {}
    }
}

fn handle_confirming_service_delete(app: &mut App, code: KeyCode) {
    let Mode::ConfirmingServiceDelete { name, port, machine } = &app.mode else {
        return;
    };
    let (name, port, machine) = (name.clone(), *port, machine.clone());

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.delete_service(&name, port, &machine);
            app.mode = Mode::Normal;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn handle_adding_api_token(app: &mut App, code: KeyCode) {
    let Mode::AddingApiToken { input } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            if !input.is_empty() {
                let token = input.clone();
                app.finish_add_api_token(token);
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

fn handle_adding(app: &mut App, code: KeyCode) {
    let Mode::Adding { field, name, token } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
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
        KeyCode::Backspace => {
            match field {
                AddField::Name => { name.pop(); }
                AddField::Token => { token.pop(); }
            }
        }
        KeyCode::Char(c) => {
            match field {
                AddField::Name => name.push(c),
                AddField::Token => token.push(c),
            }
        }
        _ => {}
    }
}

fn handle_editing(app: &mut App, code: KeyCode) {
    let Mode::Editing { name, token } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
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
        KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
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
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.do_migrate(plists);
        }
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
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn cli_list() -> Result<()> {
    let config = config::Config::load()?;
    if config.tunnels.is_empty() {
        println!("No tunnels configured.");
        return Ok(());
    }

    println!("{:<18} {:<10} {:<10} {}", "NAME", "STATUS", "PID", "TUNNEL ID");
    println!("{:<18} {:<10} {:<10} {}", "──────────────────", "──────────", "──────────", "──────────────");

    for t in &config.tunnels {
        let status = launchd::status(&t.name);
        let (status_str, pid_str) = match &status {
            launchd::Status::Running { pid } => {
                ("running", pid.map(|p| p.to_string()).unwrap_or("-".into()))
            }
            launchd::Status::Stopped => ("stopped", "-".into()),
            launchd::Status::Inactive => ("inactive", "-".into()),
        };
        let tunnel_id = config::decode_token(&t.token)
            .map(|p| p.tunnel_id)
            .unwrap_or("-".into());

        println!("{:<18} {:<10} {:<10} {}", t.name, status_str, pid_str, tunnel_id);
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
