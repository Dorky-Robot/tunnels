use crate::cloudflare::{self, IngressRoute, TunnelInfo, UnreachedAccount};
use crate::config::{Config, Tunnel};
use crate::launchd;
use crate::scan;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Tunnels,
    Services,
    Routes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Adding {
        field: AddField,
        name: String,
        token: String,
    },
    Editing {
        name: String,
        token: String,
    },
    Renaming {
        old_name: String,
        new_name: String,
    },
    ConfirmingDelete {
        target: String,
    },
    Logs {
        name: String,
        content: String,
    },
    Migrating {
        daemon_plists: Vec<std::path::PathBuf>,
    },
    AddingService {
        field: ServiceField,
        name: String,
        port: String,
        machine: String,
    },
    EditingService {
        idx: usize,
        field: ServiceField,
        name: String,
        port: String,
        machine: String,
    },
    ConfirmingServiceDelete {
        name: String,
        port: u16,
        machine: String,
    },
    AddingApiToken {
        tunnel_name: String,
        input: String,
    },
    ContextMenu {
        items: Vec<(char, String)>,
        selected: usize,
    },
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
}

#[derive(Debug, Clone)]
pub struct TunnelRow {
    pub name: String,
    pub status: launchd::Status,
    pub cf_name: String,
    pub cf_conns: String,
    pub has_api_token: bool,
}

#[derive(Debug, Clone)]
pub struct ServiceRow {
    pub name: String,
    pub port: u16,
    pub machine: String,
    pub listening: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteStatus {
    Connected,
    NoEdge,
    Unknown,
}

impl RouteStatus {
    pub fn label(&self) -> &'static str {
        match self {
            RouteStatus::Connected => "connected",
            RouteStatus::NoEdge => "no edge",
            RouteStatus::Unknown => "—",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteRow {
    pub hostname: String,
    pub port: u16,
    pub tunnel_name: String,
    pub status: RouteStatus,
}

pub struct App {
    pub config: Config,
    pub tab: Tab,
    pub rows: Vec<TunnelRow>,
    pub service_rows: Vec<ServiceRow>,
    pub route_rows: Vec<RouteRow>,
    pub tunnel_info: HashMap<String, TunnelInfo>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub unreached: Vec<UnreachedAccount>,
    pub listening_ports: HashSet<u16>,
    pub selected: usize,
    pub service_selected: usize,
    pub route_selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub cf_syncing: bool,
    cf_rx: mpsc::Receiver<cloudflare::SyncResult>,
    cf_tx: mpsc::Sender<cloudflare::SyncResult>,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let listening_ports = scan::listening_ports();
        let (cf_tx, cf_rx) = mpsc::channel();
        let mut app = Self {
            config,
            tab: Tab::Tunnels,
            rows: Vec::new(),
            service_rows: Vec::new(),
            route_rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            unreached: Vec::new(),
            listening_ports,
            selected: 0,
            service_selected: 0,
            route_selected: 0,
            mode: Mode::Normal,
            status_msg: Some("Syncing from Cloudflare...".into()),
            should_quit: false,
            cf_syncing: false,
            cf_rx,
            cf_tx,
        };
        app.start_cf_sync();
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
                let (cf_name, cf_conns) = self
                    .tunnel_info
                    .get(&t.tunnel_id)
                    .map(|info| (info.cf_name.clone(), info.connections.clone()))
                    .unwrap_or_else(|| ("—".into(), "—".into()));

                TunnelRow {
                    name: t.name.clone(),
                    status,
                    cf_name,
                    cf_conns,
                    has_api_token: t.api_token.is_some(),
                }
            })
            .collect();

        if self.selected >= self.rows.len() && !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
        }

        self.refresh_services();
        self.refresh_routes();
    }

    fn refresh_services(&mut self) {
        self.service_rows = self
            .config
            .services
            .iter()
            .map(|s| ServiceRow {
                name: s.name.clone(),
                port: s.port,
                machine: s.machine.clone(),
                listening: self.listening_ports.contains(&s.port),
            })
            .collect();

        if self.service_selected >= self.service_rows.len() && !self.service_rows.is_empty() {
            self.service_selected = self.service_rows.len() - 1;
        }
    }

    fn refresh_routes(&mut self) {
        let mut routes = Vec::new();
        for (port, ingress_list) in &self.ingress_routes {
            for route in ingress_list {
                let status = self
                    .tunnel_info
                    .get(&route.tunnel_id)
                    .map(|info| {
                        if info.connections.starts_with("no ") {
                            RouteStatus::NoEdge
                        } else {
                            RouteStatus::Connected
                        }
                    })
                    .unwrap_or(RouteStatus::Unknown);

                routes.push(RouteRow {
                    hostname: route.hostname.clone(),
                    port: *port,
                    tunnel_name: route.tunnel_name.clone(),
                    status,
                });
            }
        }
        routes.sort_by(|a, b| a.hostname.cmp(&b.hostname));
        self.route_rows = routes;

        if self.route_selected >= self.route_rows.len() && !self.route_rows.is_empty() {
            self.route_selected = self.route_rows.len() - 1;
        }
    }

    /// Kick off a background CF sync (non-blocking)
    fn start_cf_sync(&mut self) {
        if self.cf_syncing {
            return;
        }
        self.cf_syncing = true;
        let inputs: Vec<cloudflare::TunnelSyncInput> = self
            .config
            .tunnels
            .iter()
            .map(|t| cloudflare::TunnelSyncInput {
                name: t.name.clone(),
                account_id: t.account_id.clone(),
                tunnel_id: t.tunnel_id.clone(),
                api_token: t.api_token.clone(),
            })
            .collect();
        let tx = self.cf_tx.clone();
        std::thread::spawn(move || {
            let result = cloudflare::sync(&inputs);
            let _ = tx.send(result);
        });
    }

    /// Check if the background sync has completed (called from event loop)
    pub fn poll_cf_sync(&mut self) {
        if let Ok(sync) = self.cf_rx.try_recv() {
            self.cf_syncing = false;
            self.tunnel_info = sync.tunnel_info;
            self.ingress_routes = sync.ingress_routes;
            self.unreached = sync.unreached;
            self.status_msg = Some(sync.status);
            self.refresh();
        }
    }

    pub fn refresh_cf(&mut self) {
        self.status_msg = Some("Syncing from Cloudflare...".into());
        self.start_cf_sync();
    }

    pub fn selected_tunnel(&self) -> Option<&Tunnel> {
        self.config.tunnels.get(self.selected)
    }

    pub fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Tunnels => Tab::Services,
            Tab::Services => Tab::Routes,
            Tab::Routes => Tab::Tunnels,
        };
    }

    pub fn prev_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Tunnels => Tab::Routes,
            Tab::Services => Tab::Tunnels,
            Tab::Routes => Tab::Services,
        };
    }

    pub fn move_up(&mut self) {
        match self.tab {
            Tab::Tunnels => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            Tab::Services => {
                if self.service_selected > 0 {
                    self.service_selected -= 1;
                }
            }
            Tab::Routes => {
                if self.route_selected > 0 {
                    self.route_selected -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.tab {
            Tab::Tunnels => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
            }
            Tab::Services => {
                if self.service_selected + 1 < self.service_rows.len() {
                    self.service_selected += 1;
                }
            }
            Tab::Routes => {
                if self.route_selected + 1 < self.route_rows.len() {
                    self.route_selected += 1;
                }
            }
        }
    }

    /// Run a tunnel lifecycle operation on the selected tunnel
    fn with_selected_tunnel(
        &mut self,
        action: &str,
        op: impl FnOnce(&Tunnel) -> anyhow::Result<()>,
    ) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match op(&t) {
                Ok(()) => self.status_msg = Some(format!("{} '{}'", action, t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    pub fn start_selected(&mut self) {
        self.with_selected_tunnel("Started", |t| launchd::start(&t.name, &t.token));
    }

    pub fn stop_selected(&mut self) {
        self.with_selected_tunnel("Stopped", |t| launchd::stop(&t.name));
    }

    pub fn restart_selected(&mut self) {
        self.with_selected_tunnel("Restarted", |t| launchd::restart(&t.name, &t.token));
    }

    pub fn delete_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            if let Err(e) = launchd::stop(&t.name) {
                self.status_msg = Some(format!("Warning stopping '{}': {}", t.name, e));
            }
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
        let was_running = matches!(launchd::status(&old_name), launchd::Status::Running { .. });
        if was_running {
            let _ = launchd::stop(&old_name);
        }
        match self.config.rename(&old_name, new_name.clone()) {
            Ok(()) => {
                self.status_msg = Some(format!("Renamed '{}' -> '{}'", old_name, new_name));
                if was_running
                    && let Some(t) = self.config.tunnels.iter().find(|t| t.name == new_name)
                {
                    let _ = launchd::start(&t.name, &t.token);
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
            self.mode = Mode::ConfirmingDelete {
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
                if matches!(launchd::status(&name), launchd::Status::Running { .. })
                    && let Some(t) = self.config.tunnels.iter().find(|t| t.name == name)
                {
                    let _ = launchd::restart(&t.name, &t.token);
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
                count,
                daemon_plists.len()
            ));
            self.mode = Mode::Migrating { daemon_plists };
            self.refresh();
            return;
        }
        self.refresh();
    }

    // --- Context menu ---

    pub fn open_context_menu(&mut self) {
        let items: Vec<(char, String)> = match self.tab {
            Tab::Tunnels => {
                if self.rows.is_empty() {
                    return;
                }
                let has_api = self.rows[self.selected].has_api_token;
                let mut items = vec![
                    ('s', "Start".into()),
                    ('x', "Stop".into()),
                    ('r', "Restart".into()),
                    ('e', "Edit tunnel token".into()),
                    ('n', "Rename".into()),
                    ('l', "View logs".into()),
                ];
                if has_api {
                    items.push(('T', "Change API token".into()));
                    items.push(('X', "Remove API token".into()));
                } else {
                    items.push(('T', "Add API token".into()));
                }
                items.push(('d', "Delete".into()));
                items
            }
            Tab::Services => {
                if self.service_rows.is_empty() {
                    return;
                }
                vec![('e', "Edit".into()), ('d', "Untrack".into())]
            }
            Tab::Routes => {
                return;
            }
        };
        self.mode = Mode::ContextMenu { items, selected: 0 };
    }

    pub fn execute_context_action(&mut self, key: char) {
        match self.tab {
            Tab::Tunnels => match key {
                's' => self.start_selected(),
                'x' => self.stop_selected(),
                'r' => self.restart_selected(),
                'e' => {
                    self.begin_edit();
                    return;
                }
                'n' => {
                    self.begin_rename();
                    return;
                }
                'l' => {
                    self.show_logs();
                    return;
                }
                'T' => {
                    self.begin_add_api_token_for_selected();
                    return;
                }
                'X' => {
                    self.remove_api_token_for_selected();
                }
                'd' => {
                    self.confirm_delete();
                    return;
                }
                _ => {}
            },
            Tab::Services => match key {
                'e' => {
                    self.begin_edit_service();
                    return;
                }
                'd' => {
                    self.confirm_delete_service();
                    return;
                }
                _ => {}
            },
            Tab::Routes => {}
        }
        self.mode = Mode::Normal;
    }

    // --- API token for tunnel ---

    pub fn begin_add_api_token_for_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            self.mode = Mode::AddingApiToken {
                tunnel_name: t.name,
                input: String::new(),
            };
        }
    }

    pub fn finish_add_api_token(&mut self, tunnel_name: String, api_token: String) {
        // Verify it works for this tunnel's account
        if let Some(t) = self.config.tunnels.iter().find(|t| t.name == tunnel_name) {
            if t.account_id.is_empty() {
                self.status_msg = Some(format!(
                    "Error: tunnel '{}' has a corrupt token — cannot verify API token",
                    tunnel_name
                ));
                self.mode = Mode::Normal;
                return;
            }
            if !cloudflare::verify_token(&api_token, &t.account_id, &t.tunnel_id) {
                self.status_msg = Some("Token rejected — check permissions".into());
                return;
            }
        }

        match self.config.set_api_token(&tunnel_name, api_token) {
            Ok(()) => self.status_msg = Some(format!("API token set for '{}'", tunnel_name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh_cf();
    }

    pub fn remove_api_token_for_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match self.config.clear_api_token(&t.name) {
                Ok(()) => self.status_msg = Some(format!("API token removed for '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.refresh();
        }
    }

    // --- Service tab methods ---

    pub fn begin_add_service(&mut self) {
        let machine = hostname();
        self.mode = Mode::AddingService {
            field: ServiceField::Name,
            name: String::new(),
            port: String::new(),
            machine,
        };
    }

    pub fn finish_add_service(&mut self, name: String, port: String, machine: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        match self.config.add_service(name.clone(), port, machine) {
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
            };
        }
    }

    pub fn finish_edit_service(&mut self, idx: usize, name: String, port: String, machine: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        match self.config.update_service(idx, name.clone(), port, machine) {
            Ok(()) => self.status_msg = Some(format!("Updated service '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn scan_services(&mut self) {
        let found = scan::scan_services();
        let machine = hostname();
        let found_ports: HashSet<u16> = found.iter().map(|s| s.port).collect();

        self.listening_ports = found_ports.clone();

        // Remove stale services (on this machine, port no longer listening)
        let before = self.config.services.len();
        self.config
            .services
            .retain(|s| s.machine != machine || found_ports.contains(&s.port));
        let removed = before - self.config.services.len();

        // Add new services
        let mut added = 0;
        for s in &found {
            if self
                .config
                .services
                .iter()
                .any(|existing| existing.port == s.port && existing.machine == machine)
            {
                continue;
            }
            self.config.services.push(crate::config::Service {
                name: s.name.clone(),
                port: s.port,
                machine: machine.clone(),
            });
            added += 1;
        }

        // Single save for all changes
        if added > 0 || removed > 0 {
            let _ = self.config.save();
        }

        let msg = match (added, removed) {
            (0, 0) => "Scanned — no changes".into(),
            (a, 0) => format!("Scanned — +{} new", a),
            (0, r) => format!("Scanned — -{} stale", r),
            (a, r) => format!("Scanned — +{} new, -{} stale", a, r),
        };
        self.status_msg = Some(msg);
        self.refresh();
    }

    pub fn confirm_delete_service(&mut self) {
        if let Some(s) = self.config.services.get(self.service_selected) {
            self.mode = Mode::ConfirmingServiceDelete {
                name: s.name.clone(),
                port: s.port,
                machine: s.machine.clone(),
            };
        }
    }

    pub fn delete_service(&mut self, name: &str, port: u16, machine: &str) {
        match self.config.remove_service(name, port, machine) {
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

        for t in &self.config.tunnels {
            if !matches!(launchd::status(&t.name), launchd::Status::Running { .. }) {
                let _ = launchd::start(&t.name, &t.token);
            }
        }

        if errors.is_empty() {
            self.status_msg = Some(format!("Migrated {} plist(s) to user-level", migrated));
        } else {
            self.status_msg = Some(format!(
                "Migrated {} — errors: {}",
                migrated,
                errors.join(", ")
            ));
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
