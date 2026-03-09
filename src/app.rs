use crate::cloudflare::{self, IngressRoute, TunnelInfo, UnreachedAccount};
use crate::config::{self, Config, Tunnel};
use crate::launchd;
use crate::scan;
use std::collections::HashMap;
use std::sync::mpsc;

pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub enum BgResult {
    CfSync(cloudflare::SyncResult),
    Routes {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        routes: Vec<RouteRow>,
        status_msg: Option<String>,
    },
    RouteRenamed {
        status_msg: String,
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
    },
    RouteAdded {
        status_msg: String,
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
    },
    RouteDeleted {
        status_msg: String,
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
    },
    VerifyToken {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        hostname: String,
        service_url: String,
    },
    VerifyTokenFailed(String),
}

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
    AddingService { field: ServiceField, name: String, port: String, tunnel: String, memo: String },
    EditingService { idx: usize, field: ServiceField, name: String, port: String, tunnel: String, memo: String },
    ConfirmingServiceDelete { idx: usize, name: String, port: u16 },
    AddingApiToken {
        input: String,
    },
    Routes {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        routes: Vec<RouteRow>,
        selected: usize,
    },
    AddingRoute {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        field: RouteField,
        hostname: String,
        service: String,
    },
    RenamingRoute {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        old_hostname: String,
        service: String,
        new_subdomain: String,
        domain_suffix: String,
    },
    ConfirmingRouteDelete {
        tunnel_name: String,
        api_token: String,
        account_id: String,
        tunnel_id: String,
        hostname: String,
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
    Tunnel,
    Memo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteField {
    Hostname,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsStatus {
    Ok,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    pub hostname: String,
    pub service: String,
    pub dns: DnsStatus,
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
    #[allow(dead_code)]
    pub tunnel: String,
    pub tunnel_status: String,
    pub url: String,
    pub memo: String,
}

/// Split a hostname into (subdomain, domain_suffix).
/// e.g. "katulong-mini.felixflor.es" → ("katulong-mini", ".felixflor.es")
/// "simple.com" → ("simple", ".com")
/// "bare" → ("bare", "")
fn split_hostname(hostname: &str) -> (String, String) {
    if let Some(dot_pos) = hostname.find('.') {
        (hostname[..dot_pos].to_string(), hostname[dot_pos..].to_string())
    } else {
        (hostname.to_string(), String::new())
    }
}

pub struct App {
    pub config: Config,
    pub tab: Tab,
    pub rows: Vec<TunnelRow>,
    pub service_rows: Vec<ServiceRow>,
    pub tunnel_info: HashMap<String, TunnelInfo>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub unreached: Vec<UnreachedAccount>,
    pub selected: usize,
    pub service_selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub submenu: bool,
    pub loading: Option<String>,
    pub spinner_tick: usize,
    bg_tx: mpsc::Sender<BgResult>,
    bg_rx: mpsc::Receiver<BgResult>,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            config,
            tab: Tab::Services,
            rows: Vec::new(),
            service_rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            unreached: Vec::new(),
            selected: 0,
            service_selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            submenu: false,
            loading: Some("Syncing Cloudflare...".into()),
            spinner_tick: 0,
            bg_tx: tx,
            bg_rx: rx,
        };
        app.refresh();
        app.spawn_cf_sync();
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

                let (cf_name, cf_conns) = self.tunnel_info.get(&tunnel_id)
                    .map(|info| (info.cf_name.clone(), info.connections.clone()))
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
                            self.tunnel_info.get(&r.tunnel_id)
                                .map_or(false, |info| !info.connections.starts_with("no "))
                        })
                        .or(routes.first());

                    if let Some(route) = best {
                        let status = self.tunnel_info.get(&route.tunnel_id)
                            .map(|info| {
                                if info.connections.starts_with("no ") { "no edge" } else { "connected" }
                            })
                            .unwrap_or("—")
                            .to_string();
                        (
                            route.tunnel_name.clone(),
                            status,
                            format!("{}://{}", route.scheme, route.hostname),
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
                    memo: s.memo.clone().unwrap_or_default(),
                }
            })
            .collect();

        if self.service_selected >= self.service_rows.len() && !self.service_rows.is_empty() {
            self.service_selected = self.service_rows.len() - 1;
        }
    }

    pub fn refresh_cf(&mut self) {
        self.loading = Some("Syncing Cloudflare...".into());
        self.spawn_cf_sync();
    }

    fn spawn_cf_sync(&self) {
        let tx = self.bg_tx.clone();
        let tunnel_tokens: Vec<(String, String)> = self.config.tunnels.iter()
            .map(|t| (t.name.clone(), t.token.clone()))
            .collect();
        let cf_tokens: Vec<String> = self.config.all_cf_api_tokens()
            .into_iter().map(|s| s.to_string()).collect();

        std::thread::spawn(move || {
            let refs: Vec<&str> = cf_tokens.iter().map(|s| s.as_str()).collect();
            let sync = cloudflare::sync(&refs, &tunnel_tokens);
            let _ = tx.send(BgResult::CfSync(sync));
        });
    }

    pub fn poll_bg(&mut self) {
        while let Ok(result) = self.bg_rx.try_recv() {
            self.loading = None;
            match result {
                BgResult::CfSync(sync) => {
                    self.tunnel_info = sync.tunnel_info;
                    self.ingress_routes = sync.ingress_routes;
                    self.unreached = sync.unreached;
                    self.status_msg = Some(sync.status);
                    self.refresh();
                    if !self.unreached.is_empty() {
                        self.begin_add_api_token();
                    }
                }
                BgResult::Routes { tunnel_name, api_token, account_id, tunnel_id, routes, status_msg } => {
                    if let Some(msg) = status_msg {
                        self.status_msg = Some(msg);
                    }
                    self.mode = Mode::Routes {
                        tunnel_name,
                        api_token,
                        account_id,
                        tunnel_id,
                        routes,
                        selected: 0,
                    };
                }
                BgResult::RouteRenamed { status_msg, tunnel_name, api_token, account_id, tunnel_id } => {
                    self.status_msg = Some(status_msg);
                    if self.tab == Tab::Services {
                        self.loading = Some("Syncing...".into());
                        self.spawn_cf_sync();
                    } else {
                        self.loading = Some("Reloading routes...".into());
                        self.spawn_reload_routes(tunnel_name, api_token, account_id, tunnel_id);
                    }
                }
                BgResult::RouteAdded { status_msg, tunnel_name, api_token, account_id, tunnel_id } => {
                    self.status_msg = Some(status_msg);
                    self.loading = Some("Reloading routes...".into());
                    self.spawn_reload_routes(tunnel_name, api_token, account_id, tunnel_id);
                }
                BgResult::RouteDeleted { status_msg, tunnel_name, api_token, account_id, tunnel_id } => {
                    self.status_msg = Some(status_msg);
                    self.loading = Some("Reloading routes...".into());
                    self.spawn_reload_routes(tunnel_name, api_token, account_id, tunnel_id);
                }
                BgResult::VerifyToken { tunnel_name, api_token, account_id, tunnel_id, hostname, service_url } => {
                    let (subdomain, domain_suffix) = split_hostname(&hostname);
                    self.mode = Mode::RenamingRoute {
                        tunnel_name,
                        api_token,
                        account_id,
                        tunnel_id,
                        old_hostname: hostname,
                        service: service_url,
                        new_subdomain: subdomain,
                        domain_suffix,
                    };
                }
                BgResult::VerifyTokenFailed(msg) => {
                    self.status_msg = Some(msg);
                }
            }
        }
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
        self.mode = Mode::AddingService {
            field: ServiceField::Name,
            name: String::new(),
            port: String::new(),
            tunnel: String::new(),
            memo: String::new(),
        };
    }

    pub fn finish_add_service(&mut self, name: String, port: String, tunnel: String, memo: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        let tunnel = if tunnel.is_empty() { None } else { Some(tunnel) };
        let memo = if memo.is_empty() { None } else { Some(memo) };
        match self.config.add_service(name.clone(), port, tunnel, memo) {
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
                tunnel: s.tunnel.clone().unwrap_or_default(),
                memo: s.memo.clone().unwrap_or_default(),
            };
        }
    }

    pub fn finish_edit_service(&mut self, idx: usize, name: String, port: String, tunnel: String, memo: String) {
        let port: u16 = match port.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        let tunnel = if tunnel.is_empty() { None } else { Some(tunnel) };
        let memo = if memo.is_empty() { None } else { Some(memo) };
        match self.config.update_service(idx, name.clone(), port, tunnel, memo) {
            Ok(()) => self.status_msg = Some(format!("Updated service '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn scan_services(&mut self) {
        let found = scan::scan_services();
        let mut count = 0;

        for s in &found {
            if self.config.services.iter().any(|existing| existing.port == s.port) {
                continue;
            }

            let tunnel = self.config.tunnels.iter()
                .find(|t| t.name.contains(&s.name) || s.name.contains(&t.name))
                .map(|t| t.name.clone());

            if self.config.add_service(s.name.clone(), s.port, tunnel, None).is_ok() {
                count += 1;
            }
        }

        self.status_msg = Some(format!("Scanned — found {} new service(s)", count));
        self.refresh();
    }

    pub fn confirm_delete_service(&mut self) {
        if let Some(s) = self.config.services.get(self.service_selected) {
            self.mode = Mode::ConfirmingServiceDelete {
                idx: self.service_selected,
                name: s.name.clone(),
                port: s.port,
            };
        }
    }

    pub fn delete_service(&mut self, idx: usize) {
        let name = self.config.services.get(idx).map(|s| s.name.clone());
        match self.config.remove_service_by_idx(idx) {
            Ok(()) => self.status_msg = Some(format!("Untracked '{}'", name.unwrap_or_default())),
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

    /// Look up the route info for the currently selected service.
    /// Returns (tunnel_name, tunnel_token, hostname, service_url) or an error message.
    pub fn resolve_service_route(&self) -> Result<(String, String, String, String), String> {
        let service = self.config.services.get(self.service_selected)
            .ok_or_else(|| "No service selected".to_string())?;

        let routes = self.ingress_routes.get(&service.port)
            .filter(|r| !r.is_empty())
            .ok_or_else(|| "No route found for this service".to_string())?;

        let route = &routes[0];
        let service_url = format!("http://localhost:{}", service.port);

        // Find the tunnel config that owns this route
        let tunnel = self.config.tunnels.iter()
            .find(|t| {
                config::decode_token(&t.token)
                    .map(|p| p.tunnel_id == route.tunnel_id)
                    .unwrap_or(false)
            })
            .ok_or_else(|| "Tunnel not found for this route".to_string())?;

        Ok((tunnel.name.clone(), tunnel.token.clone(), route.hostname.clone(), service_url))
    }

    pub fn begin_rename_service_route(&mut self) {
        let (tunnel_name, tunnel_token, hostname, service_url) = match self.resolve_service_route() {
            Ok(v) => v,
            Err(msg) => {
                self.status_msg = Some(msg);
                return;
            }
        };

        let payload = match config::decode_token(&tunnel_token) {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Could not decode tunnel token".into());
                return;
            }
        };

        self.loading = Some("Verifying API token...".into());
        let tx = self.bg_tx.clone();
        let api_tokens: Vec<String> = self.config.all_cf_api_tokens()
            .into_iter().map(|s| s.to_string()).collect();
        let account_id = payload.account_id.clone();
        let tunnel_id = payload.tunnel_id.clone();

        std::thread::spawn(move || {
            let api_token = api_tokens.iter().find(|t| {
                cloudflare::verify_token(t, &account_id, &tunnel_id)
            });
            match api_token {
                Some(t) => {
                    let _ = tx.send(BgResult::VerifyToken {
                        tunnel_name,
                        api_token: t.to_string(),
                        account_id,
                        tunnel_id,
                        hostname,
                        service_url,
                    });
                }
                None => {
                    let _ = tx.send(BgResult::VerifyTokenFailed(
                        "No API token with access — press T to add one".into()
                    ));
                }
            }
        });
    }

    // --- Route management methods ---

    pub fn begin_routes(&mut self) {
        let tunnel = match self.selected_tunnel().cloned() {
            Some(t) => t,
            None => return,
        };

        let payload = match config::decode_token(&tunnel.token) {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Could not decode tunnel token".into());
                return;
            }
        };

        self.loading = Some("Loading routes...".into());
        let tx = self.bg_tx.clone();
        let api_tokens: Vec<String> = self.config.all_cf_api_tokens()
            .into_iter().map(|s| s.to_string()).collect();
        let tunnel_name = tunnel.name.clone();
        let account_id = payload.account_id.clone();
        let tunnel_id = payload.tunnel_id.clone();

        std::thread::spawn(move || {
            let api_token = match api_tokens.iter().find(|t| {
                cloudflare::verify_token(t, &account_id, &tunnel_id)
            }) {
                Some(t) => t.to_string(),
                None => {
                    let _ = tx.send(BgResult::VerifyTokenFailed(
                        "No API token with access — press T to add one".into()
                    ));
                    return;
                }
            };

            let cf_routes = cloudflare::list_routes(&api_token, &account_id, &tunnel_id);
            let mut fixed = 0;
            let mut fix_failed = 0;
            let routes: Vec<RouteRow> = cf_routes.iter()
                .map(|r| {
                    let hostname = r.hostname.clone().unwrap_or_else(|| "(catch-all)".into());
                    let dns = if r.hostname.is_none() {
                        DnsStatus::Ok
                    } else {
                        match cloudflare::check_dns(&api_token, &hostname) {
                            Ok(true) => DnsStatus::Ok,
                            Ok(false) => {
                                match cloudflare::ensure_dns(&api_token, &hostname, &tunnel_id) {
                                    Ok(cloudflare::RouteResult::Ok) => {
                                        fixed += 1;
                                        DnsStatus::Ok
                                    }
                                    _ => {
                                        fix_failed += 1;
                                        DnsStatus::Missing
                                    }
                                }
                            }
                            Err(_) => DnsStatus::Unknown,
                        }
                    };
                    RouteRow { hostname, service: r.service.clone(), dns }
                })
                .collect();

            let status_msg = if fixed > 0 && fix_failed == 0 {
                Some(format!("✓ Fixed DNS for {} route(s)", fixed))
            } else if fixed > 0 {
                Some(format!("✓ Fixed {} route(s), ⚠ {} still need DNS (token needs Zone>DNS>Edit)", fixed, fix_failed))
            } else if fix_failed > 0 {
                Some(format!("⚠ {} route(s) missing DNS — token needs Zone>Zone>Read + Zone>DNS>Edit", fix_failed))
            } else {
                None
            };

            let _ = tx.send(BgResult::Routes {
                tunnel_name, api_token, account_id, tunnel_id, routes, status_msg,
            });
        });
    }

    pub fn begin_add_route(&mut self) {
        let Mode::Routes { tunnel_name, api_token, account_id, tunnel_id, .. } = &self.mode else {
            return;
        };
        self.mode = Mode::AddingRoute {
            tunnel_name: tunnel_name.clone(),
            api_token: api_token.clone(),
            account_id: account_id.clone(),
            tunnel_id: tunnel_id.clone(),
            field: RouteField::Hostname,
            hostname: String::new(),
            service: "http://localhost:".into(),
        };
    }

    pub fn begin_rename_route(&mut self) {
        let Mode::Routes { tunnel_name, api_token, account_id, tunnel_id, routes, selected } = &self.mode else {
            return;
        };
        if let Some(route) = routes.get(*selected) {
            if route.hostname == "(catch-all)" {
                self.status_msg = Some("Cannot rename catch-all route".into());
                return;
            }
            let (subdomain, domain_suffix) = split_hostname(&route.hostname);
            self.mode = Mode::RenamingRoute {
                tunnel_name: tunnel_name.clone(),
                api_token: api_token.clone(),
                account_id: account_id.clone(),
                tunnel_id: tunnel_id.clone(),
                old_hostname: route.hostname.clone(),
                service: route.service.clone(),
                new_subdomain: subdomain,
                domain_suffix,
            };
        }
    }

    pub fn finish_rename_route(&mut self, tunnel_name: String, api_token: String, account_id: String, tunnel_id: String, old_hostname: String, service: String, new_hostname: String) {
        if old_hostname == new_hostname {
            self.status_msg = Some("Name unchanged".into());
            self.mode = Mode::Normal;
            return;
        }

        self.loading = Some("Renaming route...".into());
        self.mode = Mode::Normal;
        let tx = self.bg_tx.clone();

        std::thread::spawn(move || {
            // Add new route first
            match cloudflare::add_route(&api_token, &account_id, &tunnel_id, &new_hostname, &service) {
                Ok(cloudflare::RouteResult::Ok | cloudflare::RouteResult::AlreadyExists) => {}
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
                    let _ = tx.send(BgResult::RouteRenamed {
                        status_msg: format!("⚠ New route ok, DNS failed: {} — re-run m to fix", e),
                        tunnel_name, api_token, account_id, tunnel_id,
                    });
                    return;
                }
                Err(e) => {
                    let msg = if e.contains("10000") || e.contains("Authentication") {
                        format!("✗ API token needs Cloudflare Tunnel:Edit permission — {}", e)
                    } else {
                        format!("✗ Failed to create {}: {}", new_hostname, e)
                    };
                    let _ = tx.send(BgResult::RouteRenamed {
                        status_msg: msg,
                        tunnel_name, api_token, account_id, tunnel_id,
                    });
                    return;
                }
            }

            // Remove old route
            let status_msg = match cloudflare::remove_route(&api_token, &account_id, &tunnel_id, &old_hostname) {
                Ok(cloudflare::RouteResult::Ok) => {
                    format!("✓ Renamed {} → {}", old_hostname, new_hostname)
                }
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
                    format!("⚠ Renamed, old DNS cleanup failed: {}", e)
                }
                Ok(cloudflare::RouteResult::AlreadyExists) => unreachable!(),
                Err(e) => {
                    format!("⚠ New route ok, old removal failed: {}", e)
                }
            };

            let _ = tx.send(BgResult::RouteRenamed {
                status_msg, tunnel_name, api_token, account_id, tunnel_id,
            });
        });
    }

    pub fn confirm_delete_route(&mut self) {
        let Mode::Routes { tunnel_name, api_token, account_id, tunnel_id, routes, selected } = &self.mode else {
            return;
        };
        if let Some(route) = routes.get(*selected) {
            if route.hostname == "(catch-all)" {
                self.status_msg = Some("Cannot delete catch-all route".into());
                return;
            }
            self.mode = Mode::ConfirmingRouteDelete {
                tunnel_name: tunnel_name.clone(),
                api_token: api_token.clone(),
                account_id: account_id.clone(),
                tunnel_id: tunnel_id.clone(),
                hostname: route.hostname.clone(),
            };
        }
    }

    pub fn finish_add_route(&mut self, tunnel_name: String, api_token: String, account_id: String, tunnel_id: String, hostname: String, service: String) {
        // Normalize port shorthand
        let service = if service.parse::<u16>().is_ok() {
            format!("http://localhost:{}", service)
        } else {
            service
        };

        self.loading = Some("Adding route...".into());
        self.mode = Mode::Normal;
        let tx = self.bg_tx.clone();

        std::thread::spawn(move || {
            let status_msg = match cloudflare::add_route(&api_token, &account_id, &tunnel_id, &hostname, &service) {
                Ok(cloudflare::RouteResult::Ok) => {
                    format!("✓ {} → {} (route + DNS)", hostname, service)
                }
                Ok(cloudflare::RouteResult::AlreadyExists) => {
                    format!("✓ {} — route exists, DNS ok", hostname)
                }
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
                    format!("⚠ Route ok, DNS failed: {} — re-run or add CNAME: {} → {}.cfargotunnel.com", e, hostname, tunnel_id)
                }
                Err(e) => {
                    format!("✗ {}", e)
                }
            };
            let _ = tx.send(BgResult::RouteAdded {
                status_msg, tunnel_name, api_token, account_id, tunnel_id,
            });
        });
    }

    pub fn finish_delete_route(&mut self, tunnel_name: String, api_token: String, account_id: String, tunnel_id: String, hostname: String) {
        self.loading = Some("Removing route...".into());
        self.mode = Mode::Normal;
        let tx = self.bg_tx.clone();

        std::thread::spawn(move || {
            let status_msg = match cloudflare::remove_route(&api_token, &account_id, &tunnel_id, &hostname) {
                Ok(cloudflare::RouteResult::Ok) => {
                    format!("✓ Removed {} (route + DNS)", hostname)
                }
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
                    format!("⚠ Route removed, DNS cleanup failed: {} — manually delete CNAME for {}", e, hostname)
                }
                Ok(cloudflare::RouteResult::AlreadyExists) => unreachable!(),
                Err(e) => {
                    format!("✗ {}", e)
                }
            };
            let _ = tx.send(BgResult::RouteDeleted {
                status_msg, tunnel_name, api_token, account_id, tunnel_id,
            });
        });
    }

    fn spawn_reload_routes(&self, tunnel_name: String, api_token: String, account_id: String, tunnel_id: String) {
        let tx = self.bg_tx.clone();

        std::thread::spawn(move || {
            let cf_routes = cloudflare::list_routes(&api_token, &account_id, &tunnel_id);
            let routes: Vec<RouteRow> = cf_routes.iter()
                .map(|r| {
                    let hostname = r.hostname.clone().unwrap_or_else(|| "(catch-all)".into());
                    let dns = if r.hostname.is_none() {
                        DnsStatus::Ok
                    } else {
                        match cloudflare::check_dns(&api_token, &hostname) {
                            Ok(true) => DnsStatus::Ok,
                            Ok(false) => {
                                match cloudflare::ensure_dns(&api_token, &hostname, &tunnel_id) {
                                    Ok(cloudflare::RouteResult::Ok) => DnsStatus::Ok,
                                    _ => DnsStatus::Missing,
                                }
                            }
                            Err(_) => DnsStatus::Unknown,
                        }
                    };
                    RouteRow { hostname, service: r.service.clone(), dns }
                })
                .collect();

            let _ = tx.send(BgResult::Routes {
                tunnel_name, api_token, account_id, tunnel_id, routes, status_msg: None,
            });
        });
    }

    // --- CF API Token methods ---

    pub fn begin_add_api_token(&mut self) {
        if self.unreached.is_empty() {
            self.status_msg = Some("All accounts have tokens".into());
            return;
        }
        self.mode = Mode::AddingApiToken {
            input: String::new(),
        };
    }

    #[cfg(test)]
    pub fn test_app(config: config::Config, ingress_routes: HashMap<u16, Vec<IngressRoute>>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            config,
            tab: Tab::Services,
            rows: Vec::new(),
            service_rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes,
            unreached: Vec::new(),
            selected: 0,
            service_selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            submenu: false,
            loading: None,
            spinner_tick: 0,
            bg_tx: tx,
            bg_rx: rx,
        }
    }

    pub fn finish_add_api_token(&mut self, token: String) {
        // verify_token is blocking but it's a single HTTP call per unreached account — keep sync for now
        let matched: Option<&UnreachedAccount> = self.unreached.iter().find(|a| {
            cloudflare::verify_token(&token, &a.account_id, &a.tunnel_id)
        });

        let matched_names = match matched {
            Some(a) => a.tunnel_names.join(", "),
            None => {
                self.status_msg = Some("Token rejected — doesn't match any unreached account".into());
                return;
            }
        };

        // Save it
        match self.config.add_api_token(token) {
            Ok(()) => {
                self.status_msg = Some(format!("Token added for {}", matched_names));
            }
            Err(e) => {
                self.status_msg = Some(format!("Error: {}", e));
            }
        }

        self.mode = Mode::Normal;
        self.loading = Some("Syncing Cloudflare...".into());
        self.spawn_cf_sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudflare::IngressRoute;
    use crate::config::{Config, Service, Tunnel};

    // Token payload: {"a":"acct123","t":"tun456"}
    const TEST_TOKEN: &str = "eyJhIjoiYWNjdDEyMyIsInQiOiJ0dW40NTYifQ==";

    fn make_app(services: Vec<Service>, tunnels: Vec<Tunnel>, ingress: HashMap<u16, Vec<IngressRoute>>) -> App {
        let config = Config {
            tunnels,
            services,
            cf_api_tokens: Vec::new(),
            cf_api_token: None,
        };
        App::test_app(config, ingress)
    }

    #[test]
    fn split_hostname_splits_correctly() {
        let (sub, domain) = super::split_hostname("katulong-mini.felixflor.es");
        assert_eq!(sub, "katulong-mini");
        assert_eq!(domain, ".felixflor.es");
    }

    #[test]
    fn split_hostname_single_dot() {
        let (sub, domain) = super::split_hostname("simple.com");
        assert_eq!(sub, "simple");
        assert_eq!(domain, ".com");
    }

    #[test]
    fn split_hostname_no_dot() {
        let (sub, domain) = super::split_hostname("bare");
        assert_eq!(sub, "bare");
        assert_eq!(domain, "");
    }

    #[test]
    fn resolve_service_route_finds_route_by_port() {
        let services = vec![Service {
            name: "katulong".into(),
            port: 3001,
            machine: String::new(),
            tunnel: None,
            memo: None,
        }];
        let tunnels = vec![Tunnel {
            name: "my-tunnel".into(),
            token: TEST_TOKEN.into(),
            api_token: None,
        }];
        let mut ingress = HashMap::new();
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let app = make_app(services, tunnels, ingress);
        let result = app.resolve_service_route();
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let (tunnel_name, _, hostname, service_url) = result.unwrap();
        assert_eq!(tunnel_name, "my-tunnel");
        assert_eq!(hostname, "katulong-mini.felixflor.es");
        assert_eq!(service_url, "http://localhost:3001");
    }

    #[test]
    fn resolve_service_route_no_service_selected() {
        let app = make_app(Vec::new(), Vec::new(), HashMap::new());
        let result = app.resolve_service_route();
        assert_eq!(result, Err("No service selected".into()));
    }

    #[test]
    fn resolve_service_route_no_route_for_port() {
        let services = vec![Service {
            name: "postgres".into(),
            port: 5432,
            machine: String::new(),
            tunnel: None,
            memo: Some("levee db".into()),
        }];

        let app = make_app(services, Vec::new(), HashMap::new());
        let result = app.resolve_service_route();
        assert_eq!(result, Err("No route found for this service".into()));
    }

    #[test]
    fn resolve_service_route_no_matching_tunnel() {
        let services = vec![Service {
            name: "katulong".into(),
            port: 3001,
            machine: String::new(),
            tunnel: None,
            memo: None,
        }];
        // No tunnels configured
        let mut ingress = HashMap::new();
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let app = make_app(services, Vec::new(), ingress);
        let result = app.resolve_service_route();
        assert_eq!(result, Err("Tunnel not found for this route".into()));
    }

    #[test]
    fn resolve_service_route_returns_decoded_token_data() {
        let services = vec![Service {
            name: "katulong".into(),
            port: 3001,
            machine: String::new(),
            tunnel: None,
            memo: None,
        }];
        let tunnels = vec![Tunnel {
            name: "my-tunnel".into(),
            token: TEST_TOKEN.into(),
            api_token: None,
        }];
        let mut ingress = HashMap::new();
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let app = make_app(services, tunnels, ingress);
        let (_, token, _, _) = app.resolve_service_route().unwrap();
        // Verify the token can be decoded to get account_id
        let payload = config::decode_token(&token).unwrap();
        assert_eq!(payload.account_id, "acct123");
        assert_eq!(payload.tunnel_id, "tun456");
    }

    #[test]
    fn resolve_service_route_selects_correct_service() {
        let services = vec![
            Service {
                name: "dogtopia".into(),
                port: 3000,
                machine: String::new(),
                tunnel: None,
                memo: None,
            },
            Service {
                name: "katulong".into(),
                port: 3001,
                machine: String::new(),
                tunnel: None,
                memo: None,
            },
        ];
        let tunnels = vec![Tunnel {
            name: "my-tunnel".into(),
            token: TEST_TOKEN.into(),
            api_token: None,
        }];
        let mut ingress = HashMap::new();
        ingress.insert(3000, vec![IngressRoute {
            hostname: "dogtopia.everyday.vet".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let mut app = make_app(services, tunnels, ingress);
        app.service_selected = 1; // katulong
        let result = app.resolve_service_route();
        assert!(result.is_ok());
        let (_, _, hostname, _) = result.unwrap();
        assert_eq!(hostname, "katulong-mini.felixflor.es");
    }

    #[test]
    fn begin_rename_service_route_sets_error_when_no_route() {
        let services = vec![Service {
            name: "postgres".into(),
            port: 5432,
            machine: String::new(),
            tunnel: None,
            memo: Some("levee db".into()),
        }];
        let mut app = make_app(services, Vec::new(), HashMap::new());
        app.begin_rename_service_route();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status_msg, Some("No route found for this service".into()));
    }

    #[test]
    fn begin_rename_service_route_sets_error_when_no_tunnel() {
        let services = vec![Service {
            name: "katulong".into(),
            port: 3001,
            machine: String::new(),
            tunnel: None,
            memo: None,
        }];
        let mut ingress = HashMap::new();
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let mut app = make_app(services, Vec::new(), ingress);
        app.begin_rename_service_route();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status_msg, Some("Tunnel not found for this route".into()));
    }

    #[test]
    fn begin_rename_service_route_sets_error_when_no_api_token() {
        let services = vec![Service {
            name: "katulong".into(),
            port: 3001,
            machine: String::new(),
            tunnel: None,
            memo: None,
        }];
        let tunnels = vec![Tunnel {
            name: "my-tunnel".into(),
            token: TEST_TOKEN.into(),
            api_token: None,
        }];
        let mut ingress = HashMap::new();
        ingress.insert(3001, vec![IngressRoute {
            hostname: "katulong-mini.felixflor.es".into(),
            tunnel_name: "my-tunnel".into(),
            tunnel_id: "tun456".into(),
            scheme: "https".into(),
        }]);

        let mut app = make_app(services, tunnels, ingress);
        // No cf_api_tokens configured, so verify_token can't find a match
        app.begin_rename_service_route();
        // This now runs in a background thread — wait for it
        assert!(app.loading.is_some());
        std::thread::sleep(std::time::Duration::from_millis(500));
        app.poll_bg();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status_msg, Some("No API token with access — press T to add one".into()));
    }
}

