use crate::cloudflare::{self, IngressRoute, TunnelInfo, UnreachedAccount};
use crate::config::{self, Config, Tunnel};
use crate::launchd;
use crate::scan;
use std::collections::{HashMap, HashSet};
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

// --- Unified tree view types ---

#[derive(Debug, Clone)]
pub enum UnifiedRow {
    Tunnel {
        name: String,
        status: launchd::Status,
        #[allow(dead_code)]
        cf_name: String,
        cf_conns: String,
        service_count: usize,
    },
    Service {
        name: String,
        port: u16,
        #[allow(dead_code)]
        tunnel_name: Option<String>,
        tunnel_status: String,
        url: String,
        memo: String,
        is_last: bool,
    },
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixKey {
    Add,
    Token,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAction {
    StartTunnel,
    StopTunnel,
    RestartTunnel,
    ManageRoutes,
    ViewLogs,
    EditConnToken,
    RenameTunnel,
    DeleteTunnel,
    EditService,
    RenameUrl,
    UntrackService,
}

// --- Mode and field enums ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prefix(PrefixKey),
    ContextMenu {
        items: Vec<(char, String, ContextAction)>,
        selected: usize,
    },
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
    pub unified_rows: Vec<UnifiedRow>,
    pub tunnel_info: HashMap<String, TunnelInfo>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub unreached: Vec<UnreachedAccount>,
    pub selected: usize,
    pub collapsed: HashSet<String>,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub loading: Option<String>,
    pub spinner_tick: usize,
    /// When true, route bg results will reopen the Routes dialog instead of just syncing
    bg_reopen_routes: bool,
    bg_tx: mpsc::Sender<BgResult>,
    bg_rx: mpsc::Receiver<BgResult>,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            config,
            unified_rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            unreached: Vec::new(),
            selected: 0,
            collapsed: HashSet::new(),
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            loading: Some("Syncing Cloudflare...".into()),
            spinner_tick: 0,
            bg_reopen_routes: false,
            bg_tx: tx,
            bg_rx: rx,
        };
        app.rebuild_unified_rows();
        app.spawn_cf_sync();
        app
    }

    pub fn rebuild_unified_rows(&mut self) {
        self.unified_rows.clear();
        let mut claimed_services: HashSet<String> = HashSet::new();

        for tunnel_cfg in &self.config.tunnels {
            let status = launchd::status(&tunnel_cfg.name);
            let tunnel_id = config::decode_token(&tunnel_cfg.token)
                .map(|p| p.tunnel_id)
                .unwrap_or_default();

            let (cf_name, cf_conns) = self.tunnel_info.get(&tunnel_id)
                .map(|info| (info.cf_name.clone(), info.connections.clone()))
                .unwrap_or_else(|| ("—".into(), "—".into()));

            // Find services linked to this tunnel
            let mut linked_services: Vec<&config::Service> = Vec::new();
            for svc in &self.config.services {
                let linked_by_route = self.ingress_routes.get(&svc.port)
                    .map_or(false, |routes| routes.iter().any(|r| r.tunnel_id == tunnel_id));
                let linked_by_name = svc.tunnel.as_deref() == Some(&tunnel_cfg.name);
                if linked_by_route || linked_by_name {
                    linked_services.push(svc);
                    claimed_services.insert(svc.name.clone());
                }
            }

            let service_count = linked_services.len();
            self.unified_rows.push(UnifiedRow::Tunnel {
                name: tunnel_cfg.name.clone(),
                status: status.clone(),
                cf_name,
                cf_conns,
                service_count,
            });

            if !self.collapsed.contains(&tunnel_cfg.name) {
                let total = linked_services.len();
                for (j, svc) in linked_services.iter().enumerate() {
                    let (tunnel_status, url) = self.resolve_service_display(svc, Some(&status));
                    self.unified_rows.push(UnifiedRow::Service {
                        name: svc.name.clone(),
                        port: svc.port,
                        tunnel_name: Some(tunnel_cfg.name.clone()),
                        tunnel_status,
                        url,
                        memo: svc.memo.clone().unwrap_or_default(),
                        is_last: j == total - 1,
                    });
                }
            }
        }

        // Unlinked services
        let unlinked: Vec<&config::Service> = self.config.services.iter()
            .filter(|s| !claimed_services.contains(&s.name))
            .collect();

        if !unlinked.is_empty() {
            self.unified_rows.push(UnifiedRow::Separator);
            let total = unlinked.len();
            for (j, svc) in unlinked.iter().enumerate() {
                let (tunnel_status, url) = self.resolve_service_display(svc, None);
                self.unified_rows.push(UnifiedRow::Service {
                    name: svc.name.clone(),
                    port: svc.port,
                    tunnel_name: None,
                    tunnel_status,
                    url,
                    memo: svc.memo.clone().unwrap_or_default(),
                    is_last: j == total - 1,
                });
            }
        }

        // Adjust selection
        if self.unified_rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.unified_rows.len() {
            self.selected = self.unified_rows.len() - 1;
        }
        // Skip separator if landed on one
        self.skip_separator();
    }

    /// Compute display status and URL for a service
    fn resolve_service_display(&self, svc: &config::Service, parent_status: Option<&launchd::Status>) -> (String, String) {
        let routes = self.ingress_routes.get(&svc.port);
        if let Some(routes) = routes {
            let best = routes.iter()
                .find(|r| {
                    self.tunnel_info.get(&r.tunnel_id)
                        .map_or(false, |info| info.connection_count > 0)
                })
                .or(routes.first());
            if let Some(route) = best {
                let status = self.tunnel_info.get(&route.tunnel_id)
                    .map(|info| if info.connection_count == 0 { "no edge" } else { "connected" })
                    .unwrap_or("—")
                    .to_string();
                return (status, format!("{}://{}", route.scheme, route.hostname));
            }
        }
        if let Some(name) = &svc.tunnel {
            let st = parent_status.cloned().unwrap_or_else(|| launchd::status(name));
            let status_str = match &st {
                launchd::Status::Running { .. } => "running".to_string(),
                launchd::Status::Stopped => "stopped".to_string(),
                launchd::Status::Inactive => "inactive".to_string(),
            };
            (status_str, "—".to_string())
        } else {
            ("—".to_string(), "—".to_string())
        }
    }

    fn skip_separator(&mut self) {
        if matches!(self.unified_rows.get(self.selected), Some(UnifiedRow::Separator)) {
            // Try moving down first, then up
            if self.selected + 1 < self.unified_rows.len() {
                self.selected += 1;
            } else if self.selected > 0 {
                self.selected -= 1;
            }
        }
    }

    pub fn selected_row(&self) -> Option<&UnifiedRow> {
        self.unified_rows.get(self.selected)
    }

    pub fn is_tunnel_selected(&self) -> bool {
        matches!(self.selected_row(), Some(UnifiedRow::Tunnel { .. }))
    }

    pub fn is_service_selected(&self) -> bool {
        matches!(self.selected_row(), Some(UnifiedRow::Service { .. }))
    }

    pub fn selected_tunnel(&self) -> Option<&Tunnel> {
        match self.selected_row()? {
            UnifiedRow::Tunnel { name, .. } => self.config.tunnels.iter().find(|t| &t.name == name),
            _ => None,
        }
    }

    pub fn selected_service_idx(&self) -> Option<usize> {
        match self.selected_row()? {
            UnifiedRow::Service { name, .. } => self.config.services.iter().position(|s| &s.name == name),
            _ => None,
        }
    }

    pub fn build_context_menu(&self) -> Option<Mode> {
        match self.selected_row()? {
            UnifiedRow::Tunnel { status, .. } => {
                let mut items: Vec<(char, String, ContextAction)> = Vec::new();
                match status {
                    launchd::Status::Running { .. } => {
                        items.push(('x', "Stop".into(), ContextAction::StopTunnel));
                        items.push(('r', "Restart".into(), ContextAction::RestartTunnel));
                    }
                    _ => {
                        items.push(('s', "Start".into(), ContextAction::StartTunnel));
                    }
                }
                items.push(('m', "Manage routes".into(), ContextAction::ManageRoutes));
                items.push(('l', "View logs".into(), ContextAction::ViewLogs));
                items.push(('e', "Edit conn. token".into(), ContextAction::EditConnToken));
                items.push(('n', "Rename".into(), ContextAction::RenameTunnel));
                items.push(('d', "Delete".into(), ContextAction::DeleteTunnel));

                Some(Mode::ContextMenu {
                    items,
                    selected: 0,
                })
            }
            UnifiedRow::Service { .. } => {
                let items = vec![
                    ('e', "Edit service".into(), ContextAction::EditService),
                    ('n', "Rename URL".into(), ContextAction::RenameUrl),
                    ('d', "Untrack".into(), ContextAction::UntrackService),
                ];
                Some(Mode::ContextMenu {
                    items,
                    selected: 0,
                })
            }
            UnifiedRow::Separator => None,
        }
    }

    pub fn execute_context_action(&mut self, action: ContextAction) {
        match action {
            ContextAction::StartTunnel => { self.mode = Mode::Normal; self.start_selected(); }
            ContextAction::StopTunnel => { self.mode = Mode::Normal; self.stop_selected(); }
            ContextAction::RestartTunnel => { self.mode = Mode::Normal; self.restart_selected(); }
            ContextAction::ManageRoutes => { self.mode = Mode::Normal; self.begin_routes(); }
            ContextAction::ViewLogs => { self.mode = Mode::Normal; self.show_logs(); }
            ContextAction::EditConnToken => { self.mode = Mode::Normal; self.begin_edit(); }
            ContextAction::RenameTunnel => { self.mode = Mode::Normal; self.begin_rename(); }
            ContextAction::DeleteTunnel => { self.mode = Mode::Normal; self.confirm_delete(); }
            ContextAction::EditService => { self.mode = Mode::Normal; self.begin_edit_service(); }
            ContextAction::RenameUrl => { self.mode = Mode::Normal; self.begin_rename_service_route(); }
            ContextAction::UntrackService => { self.mode = Mode::Normal; self.confirm_delete_service(); }
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
                    self.rebuild_unified_rows();
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
                    if self.bg_reopen_routes {
                        self.bg_reopen_routes = false;
                        self.loading = Some("Reloading routes...".into());
                        self.spawn_reload_routes(tunnel_name, api_token, account_id, tunnel_id);
                    } else {
                        self.loading = Some("Syncing...".into());
                        self.spawn_cf_sync();
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

    // --- Navigation ---

    pub fn move_up(&mut self) {
        if self.selected == 0 {
            return;
        }
        self.selected -= 1;
        // Skip separator
        if matches!(self.unified_rows.get(self.selected), Some(UnifiedRow::Separator)) {
            if self.selected > 0 {
                self.selected -= 1;
            } else {
                self.selected += 1;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.unified_rows.is_empty() || self.selected >= self.unified_rows.len() - 1 {
            return;
        }
        self.selected += 1;
        // Skip separator
        if matches!(self.unified_rows.get(self.selected), Some(UnifiedRow::Separator)) {
            if self.selected + 1 < self.unified_rows.len() {
                self.selected += 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    pub fn toggle_expand(&mut self) {
        if let Some(UnifiedRow::Tunnel { name, .. }) = self.selected_row() {
            let name = name.clone();
            if self.collapsed.contains(&name) {
                self.collapsed.remove(&name);
            } else {
                self.collapsed.insert(name);
            }
            self.rebuild_unified_rows();
        }
    }

    // --- Tunnel actions ---

    pub fn start_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::start(&t.name, &t.token) {
                Ok(()) => self.status_msg = Some(format!("Started '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.rebuild_unified_rows();
        }
    }

    pub fn stop_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::stop(&t.name) {
                Ok(()) => self.status_msg = Some(format!("Stopped '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.rebuild_unified_rows();
        }
    }

    pub fn restart_selected(&mut self) {
        if let Some(t) = self.selected_tunnel().cloned() {
            match launchd::restart(&t.name, &t.token) {
                Ok(()) => self.status_msg = Some(format!("Restarted '{}'", t.name)),
                Err(e) => self.status_msg = Some(format!("Error: {}", e)),
            }
            self.rebuild_unified_rows();
        }
    }

    pub fn delete_tunnel_by_name(&mut self, name: &str) {
        let _ = launchd::stop(name);
        match self.config.remove(name) {
            Ok(()) => self.status_msg = Some(format!("Deleted '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.rebuild_unified_rows();
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
                if was_running {
                    if let Some(t) = self.config.tunnels.iter().find(|t| t.name == new_name) {
                        let _ = launchd::start(&t.name, &t.token);
                    }
                }
            }
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.rebuild_unified_rows();
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
        self.rebuild_unified_rows();
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
        self.rebuild_unified_rows();
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
            self.rebuild_unified_rows();
            return;
        }
        self.rebuild_unified_rows();
    }

    // --- Service actions ---

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
        self.rebuild_unified_rows();
    }

    pub fn begin_edit_service(&mut self) {
        if let Some(idx) = self.selected_service_idx() {
            if let Some(s) = self.config.services.get(idx) {
                self.mode = Mode::EditingService {
                    idx,
                    field: ServiceField::Name,
                    name: s.name.clone(),
                    port: s.port.to_string(),
                    tunnel: s.tunnel.clone().unwrap_or_default(),
                    memo: s.memo.clone().unwrap_or_default(),
                };
            }
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
        self.rebuild_unified_rows();
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
        self.rebuild_unified_rows();
    }

    pub fn confirm_delete_service(&mut self) {
        if let Some(idx) = self.selected_service_idx() {
            if let Some(s) = self.config.services.get(idx) {
                self.mode = Mode::ConfirmingServiceDelete {
                    idx,
                    name: s.name.clone(),
                    port: s.port,
                };
            }
        }
    }

    pub fn delete_service(&mut self, idx: usize) {
        let name = self.config.services.get(idx).map(|s| s.name.clone());
        match self.config.remove_service_by_idx(idx) {
            Ok(()) => self.status_msg = Some(format!("Untracked '{}'", name.unwrap_or_default())),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.rebuild_unified_rows();
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
            self.status_msg = Some(format!("Migrated {} — errors: {}", migrated, errors.join(", ")));
        }
        self.mode = Mode::Normal;
        self.rebuild_unified_rows();
    }

    /// Look up the route info for a service by name.
    pub fn resolve_service_route_by_name(&self, service_name: &str) -> Result<(String, String, String, String), String> {
        let service = self.config.services.iter()
            .find(|s| s.name == service_name)
            .ok_or_else(|| "No service selected".to_string())?;

        let routes = self.ingress_routes.get(&service.port)
            .filter(|r| !r.is_empty())
            .ok_or_else(|| "No route found for this service".to_string())?;

        let route = &routes[0];
        let service_url = format!("http://localhost:{}", service.port);

        let tunnel = self.config.tunnels.iter()
            .find(|t| {
                config::decode_token(&t.token)
                    .map(|p| p.tunnel_id == route.tunnel_id)
                    .unwrap_or(false)
            })
            .ok_or_else(|| "Tunnel not found for this route".to_string())?;

        Ok((tunnel.name.clone(), tunnel.token.clone(), route.hostname.clone(), service_url))
    }

    /// Look up route info for the currently selected service
    pub fn resolve_service_route(&self) -> Result<(String, String, String, String), String> {
        match self.selected_row() {
            Some(UnifiedRow::Service { name, .. }) => self.resolve_service_route_by_name(name),
            _ => Err("No service selected".to_string()),
        }
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

        self.bg_reopen_routes = false; // route rename from service view → sync after
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
                        "No API token with access — press t then a to add one".into()
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
                        "No API token with access — press t then a to add one".into()
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
            self.bg_reopen_routes = true; // route rename from routes dialog → reopen routes
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
        self.mode = Mode::AddingApiToken {
            input: String::new(),
        };
    }

    pub fn finish_add_api_token(&mut self, token: String) {
        let matched: Option<&UnreachedAccount> = self.unreached.iter().find(|a| {
            cloudflare::verify_token(&token, &a.account_id, &a.tunnel_id)
        });

        let description = if let Some(a) = matched {
            a.tunnel_names.join(", ")
        } else if let Some(zones) = cloudflare::verify_token_has_zones(&token) {
            format!("DNS zones: {}", zones.join(", "))
        } else {
            self.status_msg = Some(
                "Token rejected — doesn't match any tunnel account or DNS zone".into(),
            );
            return;
        };

        match self.config.add_api_token(token) {
            Ok(()) => {
                self.status_msg = Some(format!("Token added for {}", description));
            }
            Err(e) => {
                self.status_msg = Some(format!("Error: {}", e));
            }
        }

        self.mode = Mode::Normal;
        self.loading = Some("Syncing Cloudflare...".into());
        self.spawn_cf_sync();
    }

    #[cfg(test)]
    pub fn test_app(config: config::Config, ingress_routes: HashMap<u16, Vec<IngressRoute>>) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            config,
            unified_rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes,
            unreached: Vec::new(),
            selected: 0,
            collapsed: HashSet::new(),
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            loading: None,
            spinner_tick: 0,
            bg_reopen_routes: false,
            bg_tx: tx,
            bg_rx: rx,
        };
        app.rebuild_unified_rows();
        app
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
        let result = app.resolve_service_route_by_name("katulong");
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let (tunnel_name, _, hostname, service_url) = result.unwrap();
        assert_eq!(tunnel_name, "my-tunnel");
        assert_eq!(hostname, "katulong-mini.felixflor.es");
        assert_eq!(service_url, "http://localhost:3001");
    }

    #[test]
    fn resolve_service_route_no_service_selected() {
        let app = make_app(Vec::new(), Vec::new(), HashMap::new());
        let result = app.resolve_service_route_by_name("nonexistent");
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
        let result = app.resolve_service_route_by_name("postgres");
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
        let result = app.resolve_service_route_by_name("katulong");
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
        let (_, token, _, _) = app.resolve_service_route_by_name("katulong").unwrap();
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

        let app = make_app(services, tunnels, ingress);
        // Use by-name variant to directly test the correct service
        let result = app.resolve_service_route_by_name("katulong");
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
        // Select the postgres service in unified rows (it'll be unlinked: Separator at 0, service at 1)
        app.selected = match app.unified_rows.iter().position(|r| matches!(r, UnifiedRow::Service { name, .. } if name == "postgres")) {
            Some(idx) => idx,
            None => { panic!("postgres not found in unified rows"); }
        };
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
        // Select katulong service
        app.selected = match app.unified_rows.iter().position(|r| matches!(r, UnifiedRow::Service { name, .. } if name == "katulong")) {
            Some(idx) => idx,
            None => { panic!("katulong not found in unified rows"); }
        };
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
        // Select katulong service (tunnel at 0, katulong at 1)
        app.selected = match app.unified_rows.iter().position(|r| matches!(r, UnifiedRow::Service { name, .. } if name == "katulong")) {
            Some(idx) => idx,
            None => { panic!("katulong not found in unified rows"); }
        };
        app.begin_rename_service_route();
        assert!(app.loading.is_some());
        std::thread::sleep(std::time::Duration::from_millis(500));
        app.poll_bg();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status_msg, Some("No API token with access — press t then a to add one".into()));
    }
}
