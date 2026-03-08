mod app;
mod config;
mod launchd;
mod ui;

use anyhow::Result;
use app::{AddField, App, Mode};
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
                    Mode::Normal => handle_normal(app, key.code),
                    Mode::Adding { .. } => handle_adding(app, key.code),
                    Mode::Editing { .. } => handle_editing(app, key.code),
                    Mode::Renaming { .. } => handle_renaming(app, key.code),
                    Mode::Confirming { .. } => handle_confirming(app, key.code),
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
        KeyCode::Char('I') => app.import_existing(),
        KeyCode::Char('?') => app.mode = Mode::Help,
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
    for (name, token) in found {
        if !config.tunnels.iter().any(|t| t.name == name) {
            println!("  Imported '{}'", name);
            config.add(name, token)?;
            count += 1;
        }
    }
    println!("{} tunnel(s) imported.", count);
    Ok(())
}
