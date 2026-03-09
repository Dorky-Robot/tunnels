mod app;
mod cloudflare;
mod config;
mod launchd;
mod scan;
mod ui;

use anyhow::Result;
use app::{AddField, App, Mode, RouteField, ServiceField, Tab};
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
        let json_flag = args.iter().any(|a| a == "--json" || a == "-j");
        match args[1].as_str() {
            "list" | "ls" => return cli_list(json_flag),
            "import" => return cli_import(),
            "routes" => return cli_routes(args.get(2).map(|s| s.as_str()), json_flag),
            "route" => {
                if args.len() < 3 {
                    eprintln!("Usage: tunnels route <add|rm|mv> [args]");
                    std::process::exit(1);
                }
                match args[2].as_str() {
                    "add" => return cli_route_add(&args[3..]),
                    "rm" | "remove" => return cli_route_rm(&args[3..]),
                    "mv" | "rename" => return cli_route_mv(&args[3..]),
                    _ => {
                        eprintln!("Unknown route command: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            }
            "start" => return cli_start(args.get(2).map(|s| s.as_str())),
            "stop" => return cli_stop(args.get(2).map(|s| s.as_str())),
            "restart" => return cli_restart(args.get(2).map(|s| s.as_str())),
            "logs" => return cli_logs(args.get(2).map(|s| s.as_str()), &args[2..]),
            "add" => return cli_add(&args[2..]),
            "rm" | "remove" => return cli_rm(args.get(2).map(|s| s.as_str())),
            "rename" => return cli_rename(&args[2..]),
            "token" => {
                if args.len() < 3 {
                    eprintln!("Usage: tunnels token <add|edit> [args]");
                    std::process::exit(1);
                }
                match args[2].as_str() {
                    "add" => return cli_token_add(args.get(3).map(|s| s.as_str())),
                    "edit" => return cli_token_edit(&args[3..]),
                    _ => {
                        eprintln!("Unknown token command: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            }
            "service" => {
                if args.len() < 3 {
                    eprintln!("Usage: tunnels service <list|add|rm|edit|scan> [args]");
                    std::process::exit(1);
                }
                match args[2].as_str() {
                    "list" | "ls" => return cli_service_list(json_flag),
                    "add" => return cli_service_add(&args[3..]),
                    "rm" | "remove" => return cli_service_rm(&args[3..]),
                    "edit" => return cli_service_edit(&args[3..]),
                    "scan" => return cli_service_scan(),
                    _ => {
                        eprintln!("Unknown service command: {}", args[2]);
                        std::process::exit(1);
                    }
                }
            }
            "sync" => return cli_sync(),
            "--version" | "-v" | "-V" => {
                println!("tunnels {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "help" | "--help" | "-h" => {
                print_help();
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
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
        app.poll_bg();

        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if app.loading.is_some() {
                    continue;
                }

                match &app.mode {
                    Mode::Normal if app.tab == Tab::Tunnels => handle_normal(app, key.code),
                    Mode::Normal => handle_services_normal(app, key.code),
                    Mode::Adding { .. } => handle_adding(app, key.code),
                    Mode::Editing { .. } => handle_editing(app, key.code),
                    Mode::Renaming { .. } => handle_renaming(app, key.code),
                    Mode::Confirming { .. } => handle_confirming(app, key.code),
                    Mode::Migrating { .. } => handle_migrating(app, key.code),
                    Mode::AddingService { .. } => handle_adding_service(app, key.code),
                    Mode::EditingService { .. } => handle_editing_service(app, key.code),
                    Mode::ConfirmingServiceDelete { .. } => handle_confirming_service_delete(app, key.code),
                    Mode::AddingApiToken { .. } => handle_adding_api_token(app, key.code),
                    Mode::Routes { .. } => handle_routes(app, key.code),
                    Mode::AddingRoute { .. } => handle_adding_route(app, key.code),
                    Mode::RenamingRoute { .. } => handle_renaming_route(app, key.code),
                    Mode::ConfirmingRouteDelete { .. } => handle_confirming_route_delete(app, key.code),
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
        KeyCode::Char('1') => { app.tab = Tab::Services; app.submenu = false; }
        KeyCode::Char('2') => { app.tab = Tab::Tunnels; app.submenu = false; }
        KeyCode::Left | KeyCode::Right => {
            app.tab = if app.tab == Tab::Services { Tab::Tunnels } else { Tab::Services };
            app.submenu = false;
        }
        KeyCode::Char('.') => app.submenu = !app.submenu,
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
        KeyCode::Char('m') => app.begin_routes(),
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
        KeyCode::Char('1') => { app.tab = Tab::Services; app.submenu = false; }
        KeyCode::Char('2') => { app.tab = Tab::Tunnels; app.submenu = false; }
        KeyCode::Left | KeyCode::Right => {
            app.tab = if app.tab == Tab::Services { Tab::Tunnels } else { Tab::Services };
            app.submenu = false;
        }
        KeyCode::Char('.') => app.submenu = !app.submenu,
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
        KeyCode::Char('m') => app.begin_rename_service_route(),
        KeyCode::Char('d') => app.confirm_delete_service(),
        KeyCode::Char('S') => app.scan_services(),
        KeyCode::Char('R') => app.refresh_cf(),
        KeyCode::Char('T') => app.begin_add_api_token(),
        KeyCode::Char('?') => app.mode = Mode::Help,
        _ => {}
    }
}

fn handle_adding_service(app: &mut App, code: KeyCode) {
    let Mode::AddingService { field, name, port, tunnel, memo } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                ServiceField::Name => ServiceField::Port,
                ServiceField::Port => ServiceField::Tunnel,
                ServiceField::Tunnel => ServiceField::Memo,
                ServiceField::Memo => ServiceField::Name,
            };
        }
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() {
                let (n, p, t, m) = (name.clone(), port.clone(), tunnel.clone(), memo.clone());
                app.finish_add_service(n, p, t, m);
            }
        }
        KeyCode::Backspace => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => port,
                ServiceField::Tunnel => tunnel,
                ServiceField::Memo => memo,
            };
            s.pop();
        }
        KeyCode::Char(c) => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => {
                    if c.is_ascii_digit() { port } else { return; }
                }
                ServiceField::Tunnel => tunnel,
                ServiceField::Memo => memo,
            };
            s.push(c);
        }
        _ => {}
    }
}

fn handle_editing_service(app: &mut App, code: KeyCode) {
    let Mode::EditingService { idx, field, name, port, tunnel, memo } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                ServiceField::Name => ServiceField::Port,
                ServiceField::Port => ServiceField::Tunnel,
                ServiceField::Tunnel => ServiceField::Memo,
                ServiceField::Memo => ServiceField::Name,
            };
        }
        KeyCode::Enter => {
            if !name.is_empty() && !port.is_empty() {
                let (i, n, p, t, m) = (*idx, name.clone(), port.clone(), tunnel.clone(), memo.clone());
                app.finish_edit_service(i, n, p, t, m);
            }
        }
        KeyCode::Backspace => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => port,
                ServiceField::Tunnel => tunnel,
                ServiceField::Memo => memo,
            };
            s.pop();
        }
        KeyCode::Char(c) => {
            let s = match field {
                ServiceField::Name => name,
                ServiceField::Port => {
                    if c.is_ascii_digit() { port } else { return; }
                }
                ServiceField::Tunnel => tunnel,
                ServiceField::Memo => memo,
            };
            s.push(c);
        }
        _ => {}
    }
}

fn handle_confirming_service_delete(app: &mut App, code: KeyCode) {
    let Mode::ConfirmingServiceDelete { idx, .. } = &app.mode else {
        return;
    };
    let idx = *idx;

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.delete_service(idx);
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

fn handle_routes(app: &mut App, code: KeyCode) {
    let Mode::Routes { routes, selected, .. } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = Mode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if !routes.is_empty() && *selected < routes.len() - 1 {
                *selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if *selected > 0 {
                *selected -= 1;
            }
        }
        KeyCode::Char('a') => {
            app.begin_add_route();
        }
        KeyCode::Char('n') => {
            app.begin_rename_route();
        }
        KeyCode::Char('d') => {
            app.confirm_delete_route();
        }
        _ => {}
    }
}

fn handle_adding_route(app: &mut App, code: KeyCode) {
    let Mode::AddingRoute { tunnel_name, api_token, account_id, tunnel_id, field, hostname, service } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab | KeyCode::BackTab => {
            *field = match field {
                RouteField::Hostname => RouteField::Service,
                RouteField::Service => RouteField::Hostname,
            };
        }
        KeyCode::Enter => {
            if !hostname.is_empty() && !service.is_empty() {
                let (tn, at, ai, ti, h, s) = (
                    tunnel_name.clone(), api_token.clone(),
                    account_id.clone(), tunnel_id.clone(),
                    hostname.clone(), service.clone(),
                );
                app.finish_add_route(tn, at, ai, ti, h, s);
            }
        }
        KeyCode::Backspace => {
            let s = match field {
                RouteField::Hostname => hostname,
                RouteField::Service => service,
            };
            s.pop();
        }
        KeyCode::Char(c) => {
            let s = match field {
                RouteField::Hostname => hostname,
                RouteField::Service => service,
            };
            s.push(c);
        }
        _ => {}
    }
}

fn handle_renaming_route(app: &mut App, code: KeyCode) {
    let Mode::RenamingRoute { tunnel_name, api_token, account_id, tunnel_id, old_hostname, service, new_subdomain, domain_suffix } = &mut app.mode else {
        return;
    };

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            if !new_subdomain.is_empty() {
                let full_hostname = format!("{}{}", new_subdomain, domain_suffix);
                let (tn, at, ai, ti, oh, svc) = (
                    tunnel_name.clone(), api_token.clone(),
                    account_id.clone(), tunnel_id.clone(),
                    old_hostname.clone(), service.clone(),
                );
                app.finish_rename_route(tn, at, ai, ti, oh, svc, full_hostname);
            }
        }
        KeyCode::Backspace => {
            new_subdomain.pop();
        }
        KeyCode::Char(c) => {
            new_subdomain.push(c);
        }
        _ => {}
    }
}

fn handle_confirming_route_delete(app: &mut App, code: KeyCode) {
    let Mode::ConfirmingRouteDelete { tunnel_name, api_token, account_id, tunnel_id, hostname } = &app.mode else {
        return;
    };
    let (tn, at, ai, ti, h) = (
        tunnel_name.clone(), api_token.clone(),
        account_id.clone(), tunnel_id.clone(),
        hostname.clone(),
    );

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.finish_delete_route(tn, at, ai, ti, h);
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn cli_list(json: bool) -> Result<()> {
    let config = config::Config::load()?;
    if config.tunnels.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No tunnels configured.");
        }
        return Ok(());
    }

    if json {
        let items: Vec<serde_json::Value> = config.tunnels.iter().map(|t| {
            let status = launchd::status(&t.name);
            let (status_str, pid) = match &status {
                launchd::Status::Running { pid } => ("running", *pid),
                launchd::Status::Stopped => ("stopped", None),
                launchd::Status::Inactive => ("inactive", None),
            };
            let tunnel_id = config::decode_token(&t.token)
                .map(|p| p.tunnel_id)
                .unwrap_or_default();
            serde_json::json!({
                "name": t.name,
                "status": status_str,
                "pid": pid,
                "tunnel_id": tunnel_id,
            })
        }).collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
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

/// Resolve a tunnel name to its (api_token, account_id, tunnel_id).
/// Tries all configured API tokens to find one that works.
fn resolve_tunnel(config: &config::Config, tunnel_name: &str) -> Result<(String, String, String)> {
    let tunnel = config.tunnels.iter()
        .find(|t| t.name == tunnel_name)
        .ok_or_else(|| anyhow::anyhow!("tunnel '{}' not found", tunnel_name))?;

    let payload = config::decode_token(&tunnel.token)?;
    let api_tokens = config.all_cf_api_tokens();

    for api_token in &api_tokens {
        if cloudflare::verify_token(api_token, &payload.account_id, &payload.tunnel_id) {
            return Ok((api_token.to_string(), payload.account_id, payload.tunnel_id));
        }
    }

    anyhow::bail!("No API token works for tunnel '{}'. Add one with: tunnels (TUI) → T", tunnel_name)
}

fn cli_routes(tunnel_filter: Option<&str>, json: bool) -> Result<()> {
    let config = config::Config::load()?;

    // If a tunnel name is given and it looks like a flag, skip it
    let tunnel_filter = tunnel_filter.filter(|s| !s.starts_with('-'));

    let tunnels_to_query: Vec<&config::Tunnel> = if let Some(name) = tunnel_filter {
        let t = config.tunnels.iter().find(|t| t.name == name)
            .ok_or_else(|| anyhow::anyhow!("tunnel '{}' not found", name))?;
        vec![t]
    } else {
        config.tunnels.iter().collect()
    };

    let api_tokens = config.all_cf_api_tokens();
    if api_tokens.is_empty() {
        eprintln!("No API tokens configured. Add one in the TUI with T.");
        std::process::exit(1);
    }

    let mut all_routes: Vec<serde_json::Value> = Vec::new();

    for tunnel in &tunnels_to_query {
        let payload = match config::decode_token(&tunnel.token) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Find a working API token
        let api_token = api_tokens.iter()
            .find(|t| cloudflare::verify_token(t, &payload.account_id, &payload.tunnel_id));

        let api_token = match api_token {
            Some(t) => t,
            None => continue,
        };

        let routes = cloudflare::list_routes(api_token, &payload.account_id, &payload.tunnel_id);
        for route in &routes {
            let hostname = route.hostname.as_deref().unwrap_or("(catch-all)");
            all_routes.push(serde_json::json!({
                "tunnel": tunnel.name,
                "hostname": hostname,
                "service": route.service,
            }));
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_routes)?);
    } else {
        if all_routes.is_empty() {
            println!("No routes found.");
            return Ok(());
        }
        println!("{:<20} {:<35} {}", "TUNNEL", "HOSTNAME", "SERVICE");
        println!("{:<20} {:<35} {}", "────────────────────", "───────────────────────────────────", "───────────────────────");
        for r in &all_routes {
            println!("{:<20} {:<35} {}",
                r["tunnel"].as_str().unwrap_or(""),
                r["hostname"].as_str().unwrap_or(""),
                r["service"].as_str().unwrap_or(""),
            );
        }
    }

    Ok(())
}

/// Normalize service: "3000" → "http://localhost:3000", passthrough URLs
fn normalize_service(input: &str) -> String {
    if input.parse::<u16>().is_ok() {
        format!("http://localhost:{}", input)
    } else {
        input.to_string()
    }
}

fn cli_route_add(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        eprintln!("Usage: tunnels route add <hostname> <port|service> --tunnel <name>");
        eprintln!("  e.g. tunnels route add levee2.everyday.vet 3000 --tunnel myapp");
        eprintln!("       tunnels route add levee2.everyday.vet http://localhost:3000 --tunnel myapp");
        eprintln!();
        eprintln!("Idempotent — safe to re-run to fix DNS if it failed the first time.");
        std::process::exit(1);
    }

    let hostname = &args[0];
    let service = normalize_service(&args[1]);
    let tunnel_name = parse_flag(args, "--tunnel")
        .ok_or_else(|| anyhow::anyhow!("--tunnel <name> is required"))?;

    let config = config::Config::load()?;
    let (api_token, account_id, tunnel_id) = resolve_tunnel(&config, &tunnel_name)?;

    match cloudflare::add_route(&api_token, &account_id, &tunnel_id, hostname, &service) {
        Ok(cloudflare::RouteResult::Ok) => {
            println!("✓ {} → {} via {}", hostname, service, tunnel_name);
            println!("  Route: created");
            println!("  DNS:   created");
        }
        Ok(cloudflare::RouteResult::AlreadyExists) => {
            println!("✓ {} → {} via {}", hostname, service, tunnel_name);
            println!("  Route: already exists");
            println!("  DNS:   ok");
        }
        Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
            println!("⚠ {} → {} via {}", hostname, service, tunnel_name);
            println!("  Route: ok");
            println!("  DNS:   FAILED — {}", e);
            println!();
            println!("{}", cloudflare::DNS_PERMISSION_HINT);
            println!();
            println!("Or manually add a CNAME:");
            println!("  {} → {}.cfargotunnel.com", hostname, tunnel_id);
            println!();
            println!("Then re-run this command to verify.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("✗ Failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

fn cli_route_rm(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels route rm <hostname> --tunnel <name>");
        std::process::exit(1);
    }

    let hostname = &args[0];
    let tunnel_name = parse_flag(args, "--tunnel")
        .ok_or_else(|| anyhow::anyhow!("--tunnel <name> is required"))?;

    let config = config::Config::load()?;
    let (api_token, account_id, tunnel_id) = resolve_tunnel(&config, &tunnel_name)?;

    match cloudflare::remove_route(&api_token, &account_id, &tunnel_id, hostname) {
        Ok(cloudflare::RouteResult::Ok) => {
            println!("✓ Removed {}", hostname);
            println!("  Route: removed");
            println!("  DNS:   removed");
        }
        Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
            println!("⚠ Removed {} (route only)", hostname);
            println!("  Route: removed");
            println!("  DNS:   FAILED — {}", e);
            println!();
            println!("Manually delete the CNAME record for: {}", hostname);
            println!("Or update your API token permissions:");
            println!("{}", cloudflare::DNS_PERMISSION_HINT);
        }
        Ok(cloudflare::RouteResult::AlreadyExists) => unreachable!(),
        Err(e) => {
            eprintln!("✗ Failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

fn cli_route_mv(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        eprintln!("Usage: tunnels route mv <old-hostname> <new-hostname> --tunnel <name>");
        std::process::exit(1);
    }

    let old_hostname = &args[0];
    let new_hostname = &args[1];
    let tunnel_name = parse_flag(args, "--tunnel")
        .ok_or_else(|| anyhow::anyhow!("--tunnel <name> is required"))?;

    let config = config::Config::load()?;
    let (api_token, account_id, tunnel_id) = resolve_tunnel(&config, &tunnel_name)?;

    // Find the existing route's service
    let routes = cloudflare::list_routes(&api_token, &account_id, &tunnel_id);
    let old_route = routes.iter()
        .find(|r| r.hostname.as_deref() == Some(old_hostname.as_str()))
        .ok_or_else(|| anyhow::anyhow!("route '{}' not found on tunnel '{}'", old_hostname, tunnel_name))?;
    let service = old_route.service.clone();

    println!("Renaming {} → {}", old_hostname, new_hostname);
    println!("  Service: {}", service);
    println!();

    // Add new route first (idempotent)
    match cloudflare::add_route(&api_token, &account_id, &tunnel_id, new_hostname, &service) {
        Ok(cloudflare::RouteResult::Ok) => {
            println!("✓ {} created (route + DNS)", new_hostname);
        }
        Ok(cloudflare::RouteResult::AlreadyExists) => {
            println!("✓ {} already exists", new_hostname);
        }
        Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
            println!("⚠ {} route ok, DNS failed: {}", new_hostname, e);
            println!("  Re-run to retry DNS.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("✗ Failed to create {}: {}", new_hostname, e);
            std::process::exit(1);
        }
    }

    // Remove old route
    match cloudflare::remove_route(&api_token, &account_id, &tunnel_id, old_hostname) {
        Ok(cloudflare::RouteResult::Ok) => {
            println!("✓ {} removed (route + DNS)", old_hostname);
        }
        Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
            println!("⚠ {} route removed, DNS cleanup failed: {}", old_hostname, e);
        }
        Ok(cloudflare::RouteResult::AlreadyExists) => unreachable!(),
        Err(e) => {
            eprintln!("⚠ New route ok but failed to remove old: {}", e);
            std::process::exit(1);
        }
    }

    println!();
    println!("✓ Renamed {} → {}", old_hostname, new_hostname);

    Ok(())
}

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn print_help() {
    println!("tunnels — k9s-style TUI for managing cloudflared tunnels and local services");
    println!();
    println!("USAGE:");
    println!("  tunnels                              Launch TUI");
    println!("  tunnels <command> [args]              Run a CLI command");
    println!();
    println!("TUNNEL COMMANDS:");
    println!("  list [--json]                         List tunnels");
    println!("  start <name>                          Start a tunnel");
    println!("  stop <name>                           Stop a tunnel");
    println!("  restart <name>                        Restart a tunnel");
    println!("  logs <name> [--lines N]               View tunnel logs (default 50 lines)");
    println!("  add <name> --token <token>            Add a new tunnel");
    println!("  rm <name>                             Delete a tunnel");
    println!("  rename <old> <new>                    Rename a tunnel");
    println!("  import                                Import existing cloudflared plists");
    println!();
    println!("ROUTE COMMANDS:");
    println!("  routes [tunnel] [--json]              List ingress routes");
    println!("  route add <host> <port> --tunnel <n>  Add a route (idempotent)");
    println!("  route rm <host> --tunnel <name>       Remove a route");
    println!("  route mv <old> <new> --tunnel <name>  Rename a route");
    println!();
    println!("SERVICE COMMANDS:");
    println!("  service list [--json]                 List tracked services");
    println!("  service add <name> --port <p> [--tunnel <t>] [--memo <m>]");
    println!("                                        Add a service");
    println!("  service rm <name>                     Remove a service");
    println!("  service edit <name> [--port <p>] [--tunnel <t>] [--memo <m>]");
    println!("                                        Edit a service");
    println!("  service scan                          Scan for listening ports");
    println!();
    println!("TOKEN COMMANDS:");
    println!("  token add <token>                     Add a Cloudflare API token");
    println!("  token edit <tunnel> --token <token>   Set per-tunnel token");
    println!();
    println!("OTHER:");
    println!("  sync                                  Sync from Cloudflare API");
    println!("  help, --help, -h                      Show this help");
    println!("  --version, -v                         Show version");
}

fn cli_start(name: Option<&str>) -> Result<()> {
    let name = name.ok_or_else(|| anyhow::anyhow!("Usage: tunnels start <name>"))?;
    let config = config::Config::load()?;
    let tunnel = config.tunnels.iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("tunnel '{}' not found", name))?;

    launchd::start(name, &tunnel.token)?;
    println!("✓ Started {}", name);
    Ok(())
}

fn cli_stop(name: Option<&str>) -> Result<()> {
    let name = name.ok_or_else(|| anyhow::anyhow!("Usage: tunnels stop <name>"))?;
    let config = config::Config::load()?;
    if !config.tunnels.iter().any(|t| t.name == name) {
        anyhow::bail!("tunnel '{}' not found", name);
    }

    launchd::stop(name)?;
    println!("✓ Stopped {}", name);
    Ok(())
}

fn cli_restart(name: Option<&str>) -> Result<()> {
    let name = name.ok_or_else(|| anyhow::anyhow!("Usage: tunnels restart <name>"))?;
    let config = config::Config::load()?;
    let tunnel = config.tunnels.iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("tunnel '{}' not found", name))?;

    launchd::restart(name, &tunnel.token)?;
    println!("✓ Restarted {}", name);
    Ok(())
}

fn cli_logs(name: Option<&str>, args: &[String]) -> Result<()> {
    let name = name.ok_or_else(|| anyhow::anyhow!("Usage: tunnels logs <name> [--lines N]"))?;
    let config = config::Config::load()?;
    if !config.tunnels.iter().any(|t| t.name == name) {
        anyhow::bail!("tunnel '{}' not found", name);
    }

    let lines: usize = parse_flag(args, "--lines")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let output = launchd::read_logs(name, lines)?;
    if output.is_empty() {
        println!("No logs found for '{}'.", name);
    } else {
        print!("{}", output);
    }
    Ok(())
}

fn cli_add(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels add <name> --token <token>");
        std::process::exit(1);
    }

    let name = &args[0];
    let token = parse_flag(args, "--token")
        .ok_or_else(|| anyhow::anyhow!("--token <token> is required"))?;

    let mut config = config::Config::load()?;
    config.add(name.clone(), token)?;
    println!("✓ Added tunnel '{}'", name);
    Ok(())
}

fn cli_rm(name: Option<&str>) -> Result<()> {
    let name = name.ok_or_else(|| anyhow::anyhow!("Usage: tunnels rm <name>"))?;
    let mut config = config::Config::load()?;

    // Stop if running
    launchd::stop(name)?;

    config.remove(name)?;
    println!("✓ Removed tunnel '{}'", name);
    Ok(())
}

fn cli_rename(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        eprintln!("Usage: tunnels rename <old-name> <new-name>");
        std::process::exit(1);
    }

    let old_name = &args[0];
    let new_name = &args[1];

    let mut config = config::Config::load()?;

    // If running, restart with new name
    let was_running = matches!(launchd::status(old_name), launchd::Status::Running { .. });
    if was_running {
        launchd::stop(old_name)?;
    }

    let token = config.tunnels.iter()
        .find(|t| t.name == *old_name)
        .map(|t| t.token.clone())
        .ok_or_else(|| anyhow::anyhow!("tunnel '{}' not found", old_name))?;

    config.rename(old_name, new_name.clone())?;

    if was_running {
        launchd::start(new_name, &token)?;
    }

    println!("✓ Renamed '{}' → '{}'", old_name, new_name);
    Ok(())
}

fn cli_token_add(token: Option<&str>) -> Result<()> {
    let token = token.ok_or_else(|| anyhow::anyhow!("Usage: tunnels token add <token>"))?;
    let mut config = config::Config::load()?;
    config.add_api_token(token.to_string())?;
    println!("✓ API token added");
    Ok(())
}

fn cli_token_edit(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels token edit <tunnel-name> --token <token>");
        std::process::exit(1);
    }

    let tunnel_name = &args[0];
    let token = parse_flag(args, "--token")
        .ok_or_else(|| anyhow::anyhow!("--token <token> is required"))?;

    let mut config = config::Config::load()?;
    config.update_token(tunnel_name, token)?;
    println!("✓ Token updated for '{}'", tunnel_name);
    Ok(())
}

fn cli_service_list(json: bool) -> Result<()> {
    let config = config::Config::load()?;
    if config.services.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No services tracked.");
        }
        return Ok(());
    }

    if json {
        let items: Vec<serde_json::Value> = config.services.iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "port": s.port,
                "tunnel": s.tunnel,
                "memo": s.memo,
            })
        }).collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        println!("{:<20} {:<8} {:<18} {}", "NAME", "PORT", "TUNNEL", "MEMO");
        println!("{:<20} {:<8} {:<18} {}", "────────────────────", "────────", "──────────────────", "────────────────");
        for s in &config.services {
            println!("{:<20} {:<8} {:<18} {}",
                s.name,
                s.port,
                s.tunnel.as_deref().unwrap_or("—"),
                s.memo.as_deref().unwrap_or(""),
            );
        }
    }
    Ok(())
}

fn cli_service_add(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels service add <name> --port <port> [--tunnel <tunnel>] [--memo <memo>]");
        std::process::exit(1);
    }

    let name = &args[0];
    let port: u16 = parse_flag(args, "--port")
        .ok_or_else(|| anyhow::anyhow!("--port <port> is required"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid port number"))?;
    let tunnel = parse_flag(args, "--tunnel");
    let memo = parse_flag(args, "--memo");

    let mut config = config::Config::load()?;
    config.add_service(name.clone(), port, tunnel, memo)?;
    println!("✓ Added service '{}' on port {}", name, port);
    Ok(())
}

fn cli_service_rm(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels service rm <name>");
        std::process::exit(1);
    }

    let name = &args[0];
    let mut config = config::Config::load()?;
    let idx = config.services.iter().position(|s| s.name == *name)
        .ok_or_else(|| anyhow::anyhow!("service '{}' not found", name))?;
    config.remove_service_by_idx(idx)?;
    println!("✓ Removed service '{}'", name);
    Ok(())
}

fn cli_service_edit(args: &[String]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: tunnels service edit <name> [--port <port>] [--tunnel <tunnel>] [--memo <memo>]");
        std::process::exit(1);
    }

    let name = &args[0];
    let mut config = config::Config::load()?;
    let idx = config.services.iter().position(|s| s.name == *name)
        .ok_or_else(|| anyhow::anyhow!("service '{}' not found", name))?;

    let existing = &config.services[idx];
    let port: u16 = parse_flag(args, "--port")
        .and_then(|s| s.parse().ok())
        .unwrap_or(existing.port);
    let tunnel = parse_flag(args, "--tunnel").or_else(|| existing.tunnel.clone());
    let memo = parse_flag(args, "--memo").or_else(|| existing.memo.clone());

    config.update_service(idx, name.clone(), port, tunnel, memo)?;
    println!("✓ Updated service '{}'", name);
    Ok(())
}

fn cli_service_scan() -> Result<()> {
    let discovered = scan::scan_services();
    if discovered.is_empty() {
        println!("No listening services found.");
        return Ok(());
    }

    println!("{:<20} {}", "NAME", "PORT");
    println!("{:<20} {}", "────────────────────", "────────");
    for s in &discovered {
        println!("{:<20} {}", s.name, s.port);
    }
    println!();
    println!("{} service(s) found. Use 'tunnels service add' to track them.", discovered.len());
    Ok(())
}

fn cli_sync() -> Result<()> {
    let config = config::Config::load()?;
    let api_tokens = config.all_cf_api_tokens();

    if api_tokens.is_empty() {
        eprintln!("No API tokens configured. Add one with: tunnels token add <token>");
        std::process::exit(1);
    }

    let tunnel_tokens: Vec<(String, String)> = config.tunnels.iter()
        .map(|t| (t.name.clone(), t.token.clone()))
        .collect();

    println!("Syncing from Cloudflare...");
    let result = cloudflare::sync(&api_tokens, &tunnel_tokens);
    println!("{}", result.status);

    if !result.unreached.is_empty() {
        println!();
        for u in &result.unreached {
            println!("  ⚠ Account {} — tunnels: {}", &u.account_id[..8.min(u.account_id.len())], u.tunnel_names.join(", "));
        }
    }

    if !result.ingress_routes.is_empty() {
        println!();
        println!("{:<8} {:<35} {}", "PORT", "HOSTNAME", "TUNNEL");
        println!("{:<8} {:<35} {}", "────────", "───────────────────────────────────", "────────────────");
        let mut routes: Vec<_> = result.ingress_routes.iter().collect();
        routes.sort_by_key(|(port, _)| *port);
        for (port, entries) in routes {
            for entry in entries {
                println!("{:<8} {:<35} {}", port, entry.hostname, entry.tunnel_name);
            }
        }
    }

    Ok(())
}
