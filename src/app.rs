use crate::cloudflare::{self, CfTunnel, IngressRoute};
use crate::config::{self, Config, Tunnel};
use crate::launchd;
use crate::scan;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Tunnels,
    Services,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Adding { field: AddField, name: String, token: String },
    Editing { name: String, token: String },
    Renaming { old_name: String, new_name: String },
    Confirming { action: String, target: String },
    Logs { name: String, content: String },
    Migrating { daemon_plists: Vec<std::path::PathBuf> },
    AddingService { field: ServiceField, name: String, port: String, machine: String, tunnel: String },
    EditingService { idx: usize, field: ServiceField, name: String, port: String, machine: String, tunnel: String },
    ConfirmingServiceDelete { name: String, machine: String },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddField {
    Name,
    Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceField {
    Name,
    Port,
    Machine,
    Tunnel,
}

#[derive(Debug, Clone)]
pub struct TunnelRow {
    pub name: String,
    pub status: launchd::Status,
    pub cf_name: String,
    pub cf_conns: String,
}

#[derive(Debug, Clone)]
pub struct ServiceRow {
    pub name: String,
    pub port: u16,
    pub tunnel: String,
    pub tunnel_status: String,
    pub url: String,
}

pub struct App {
    pub config: Config,
    pub tab: Tab,
    pub rows: Vec<TunnelRow>,
    pub service_rows: Vec<ServiceRow>,
    pub cf_tunnels: Vec<CfTunnel>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub selected: usize,
    pub service_selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let tunnel_tokens: Vec<(String, String)> = config.tunnels.iter()
            .map(|t| (t.name.clone(), t.token.clone()))
            .collect();
        let sync = cloudflare::sync(config.cf_api_token.as_deref(), &tunnel_tokens);
        let mut app = Self {
            config,
            tab: Tab::Tunnels,
            rows: Vec::new(),
            service_rows: Vec::new(),
            cf_tunnels: sync.tunnels,
            ingress_routes: sync.ingress_routes,
            selected: 0,
            service_selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        self.rows = self
            .config
            .tunnels
            .iter()
            .map(|t| {
                let status = launchd::status(&t.name);
                let tunnel_id = config::decode_token(&t.token)
                    .map(|p| p.tunnel_id)
                    .unwrap_or_default();

                let (cf_name, cf_conns) = cloudflare::find_by_id(&self.cf_tunnels, &tunnel_id)
                    .map(|cf| (cf.name.clone(), cloudflare::connection_summary(cf)))
                    .unwrap_or_else(|| ("—".into(), "—".into()));

                TunnelRow {
                    name: t.name.clone(),
                    status,
                    cf_name,
                    cf_conns,
                }
            })
            .collect();

        if self.selected >= self.rows.len() && !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
        }

        self.refresh_services();
    }

    fn refresh_services(&mut self) {
        self.service_rows = self
            .config
            .services
            .iter()
            .map(|s| {
                // Try to resolve from ingress routes by port first
                let routes = self.ingress_routes.get(&s.port);

                let (tunnel_display, tunnel_status, url) = if let Some(routes) = routes {
                    // Pick the route whose tunnel has active connections, or first
                    let best = routes.iter()
                        .find(|r| {
                            cloudflare::find_by_id(&self.cf_tunnels, &r.tunnel_id)
                                .map_or(false, |t| !t.connections.is_empty())
                        })
                        .or(routes.first());

                    if let Some(route) = best {
                        let cf = cloudflare::find_by_id(&self.cf_tunnels, &route.tunnel_id);
                        let status = if cf.map_or(false, |t| !t.connections.is_empty()) {
                            "connected".to_string()
                        } else {
                            "no edge".to_string()
                        };
                        (
                            route.tunnel_name.clone(),
                            status,
                            format!("https://{}", route.hostname),
                        )
                    } else {
                        ("—".to_string(), "—".to_string(), "—".to_string())
                    }
                } else if let Some(name) = &s.tunnel {
                    // Manual tunnel link (no ingress route found)
                    let st = launchd::status(name);
                    let status_str = match &st {
                        launchd::Status::Running { .. } => "running".to_string(),
                        launchd::Status::Stopped => "stopped".to_string(),
                        launchd::Status::Inactive => "inactive".to_string(),
                    };
                    (name.clone(), status_str, "—".to_string())
                } else {
                    ("—".to_string(), "—".to_string(), "—".to_string())
                };

                ServiceRow {
                    name: s.name.clone(),
                    port: s.port,
                    tunnel: tunnel_display,
                    tunnel_status,
                    url,
                }
            })
            .collect();

        if self.service_selected >= self.service_rows.len() && !self.service_rows.is_empty() {
            self.service_selected = self.service_rows.len() - 1;
        }
    }

    pub fn refresh_cf(&mut self) {
        let tunnel_tokens: Vec<(String, String)> = self.config.tunnels.iter()
            .map(|t| (t.name.clone(), t.token.clone()))
            .collect();
        let sync = cloudflare::sync(self.config.cf_api_token.as_deref(), &tunnel_tokens);
        self.cf_tunnels = sync.tunnels;
        self.ingress_routes = sync.ingress_routes;
        self.status_msg = Some(sync.status);
        self.refresh();
    }

    pub fn selected_tunnel(&self) -> Option<&Tunnel> {
        self.config.tunnels.get(self.selected)
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.rows.is_empty() && self.selected < self.rows.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn start_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::start(&t.name, &t.token) {
                Ok(()) => self.status_msg = Some(format!("Started '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    pub fn stop_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::stop(&t.name) {
                Ok(()) => self.status_msg = Some(format!("Stopped '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    pub fn restart_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::restart(&t.name, &t.token) {
                Ok(()) => self.status_msg = Some(format!("Restarted '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    pub fn delete_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            let _ = launchd::stop(&t.name);
            match self.config.remove(&t.name) {
                Ok(()) => self.status_msg = Some(format!("Deleted '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    pub fn show_logs(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            let content = launchd::read_logs(&t.name, 40).unwrap_or_default();
            self.mode = Mode::Logs {
                name: t.name,
                content,
            };
        }
    }

    pub fn begin_add(&mut self) {
        self.mode = Mode::Adding {
            field: AddField::Name,
            name: String::new(),
            token: String::new(),
        };
    }

    pub fn begin_rename(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            self.mode = Mode::Renaming {
                old_name: t.name.clone(),
                new_name: t.name,
            };
        }
    }

    pub fn finish_rename(&mut self, old_name: String, new_name: String) {
        // Stop if running under old name, rename, restart under new name
        let was_running = matches!(launchd::status(&old_name), launchd::Status::Running { .. });
        if was_running {
            let _ = launchd::stop(&old_name);
        }
        match self.config.rename(&old_name, new_name.clone()) {
            Ok(()) => {
                self.status_msg = Some(format!("Renamed '{}' -> '{}'", old_name, new_name));
                if was_running {
                    if let Some(t) = self.config.tunnels.iter().find(|t| t.name == new_name) {
                        let _ = launchd::start(&t.name, &t.token);
                    }
                }
            }
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn begin_edit(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            self.mode = Mode::Editing {
                name: t.name,
                token: String::new(),
            };
        }
    }

    pub fn confirm_delete(&mut self) {
        if let Some(t) = self.selected_tunnel() {
            self.mode = Mode::Confirming {
                action: "delete".into(),
                target: t.name.clone(),
            };
        }
    }

    pub fn finish_add(&mut self, name: String, token: String) {
        match self.config.add(name.clone(), token) {
            Ok(()) => self.status_msg = Some(format!("Added '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn finish_edit(&mut self, name: String, token: String) {
        match self.config.update_token(&name, token) {
            Ok(()) => {
                self.status_msg = Some(format!("Updated '{}'", name));
                if matches!(launchd::status(&name), launchd::Status::Running { .. }) {
                    if let Some(t) = self.config.tunnels.iter().find(|t| t.name == name) {
                        let _ = launchd::restart(&t.name, &t.token);
                    }
                }
            }
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn import_existing(&mut self) {
        let found = launchd::discover_existing();
        let mut count = 0;
        let mut daemon_plists = Vec::new();

        for d in &found {
            if !self.config.tunnels.iter().any(|t| t.name == d.name) {
                if self.config.add(d.name.clone(), d.token.clone()).is_ok() {
                    count += 1;
                }
                if d.is_daemon {
                    daemon_plists.push(d.plist_path.clone());
                }
            }
        }

        if daemon_plists.is_empty() {
            self.status_msg = Some(format!("Imported {} tunnel(s)", count));
        } else {
            self.status_msg = Some(format!(
                "Imported {} tunnel(s) — {} from system LaunchDaemons",
                count, daemon_plists.len()
            ));
            self.mode = Mode::Migrating { daemon_plists };
            self.refresh();
            return;
        }
        self.refresh();
    }

    // --- Service tab methods ---

    pub fn begin_add_service(&mut self) {
        let machine = hostname();
        self.mode = Mode::AddingService {
            field: ServiceField::Name,
            name: String::new(),
            port: String::new(),
            machine,
            tunnel: String::new(),
        };
    }

    pub fn finish_add_service(&mut self, name: String, port: String, machine: String, tunnel: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        let tunnel = if tunnel.is_empty() { None } else { Some(tunnel) };
        match self.config.add_service(name.clone(), port, machine, tunnel) {
            Ok(()) => self.status_msg = Some(format!("Added service '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn begin_edit_service(&mut self) {
        if let Some(s) = self.config.services.get(self.service_selected) {
            self.mode = Mode::EditingService {
                idx: self.service_selected,
                field: ServiceField::Name,
                name: s.name.clone(),
                port: s.port.to_string(),
                machine: s.machine.clone(),
                tunnel: s.tunnel.clone().unwrap_or_default(),
            };
        }
    }

    pub fn finish_edit_service(&mut self, idx: usize, name: String, port: String, machine: String, tunnel: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        let tunnel = if tunnel.is_empty() { None } else { Some(tunnel) };
        match self.config.update_service(idx, name.clone(), port, machine, tunnel) {
            Ok(()) => self.status_msg = Some(format!("Updated service '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn scan_services(&mut self) {
        let found = scan::scan_services();
        let machine = hostname();
        let mut count = 0;

        for s in &found {
            // Skip if we already have this service on this machine
            if self.config.services.iter().any(|existing| {
                existing.port == s.port && existing.machine == machine
            }) {
                continue;
            }

            // Check if any tunnel is configured that might map to this service
            // (heuristic: tunnel name contains the service name)
            let tunnel = self.config.tunnels.iter()
                .find(|t| t.name.contains(&s.name) || s.name.contains(&t.name))
                .map(|t| t.name.clone());

            if self.config.add_service(s.name.clone(), s.port, machine.clone(), tunnel).is_ok() {
                count += 1;
            }
        }

        self.status_msg = Some(format!("Scanned — found {} new service(s)", count));
        self.refresh();
    }

    pub fn confirm_delete_service(&mut self) {
        if let Some(s) = self.config.services.get(self.service_selected) {
            self.mode = Mode::ConfirmingServiceDelete {
                name: s.name.clone(),
                machine: s.machine.clone(),
            };
        }
    }

    pub fn delete_service(&mut self, name: &str, machine: &str) {
        match self.config.remove_service(name, machine) {
            Ok(()) => self.status_msg = Some(format!("Untracked '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.refresh();
    }

    pub fn do_migrate(&mut self, plists: Vec<std::path::PathBuf>) {
        let mut migrated = 0;
        let mut errors = Vec::new();

        for plist in &plists {
            match launchd::migrate_daemon(plist) {
                Ok(()) => migrated += 1,
                Err(e) => errors.push(format!("{}", e)),
            }
        }

        // Restart imported tunnels as LaunchAgents
        for t in &self.config.tunnels {
            if !matches!(launchd::status(&t.name), launchd::Status::Running { .. }) {
                let _ = launchd::start(&t.name, &t.token);
            }
        }

        if errors.is_empty() {
            self.status_msg = Some(format!("Migrated {} plist(s) to user-level", migrated));
        } else {
            self.status_msg = Some(format!("Migrated {} — errors: {}", migrated, errors.join(", ")));
        }
        self.mode = Mode::Normal;
        self.refresh();
    }
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "localhost".into())
}
