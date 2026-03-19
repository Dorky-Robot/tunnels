use crate::cloudflare::{self, IngressRoute, TunnelInfo, UnreachedAccount};
use crate::config::{self, Config};
use crate::launchd;
use crate::scan;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// --- Background result types ---

pub enum BgResult {
    CfSync(cloudflare::SyncResult),
    LinkComplete { status_msg: String },
    UnlinkComplete { status_msg: String },
}

// --- View model ---

#[derive(Debug, Clone)]
pub struct PortRow {
    pub port: u16,
    pub name: String,
    pub url: Option<String>,
    pub health: Health,
    pub tunnel_name: Option<String>,
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Healthy,   // ✓  linked + tunnel running + edge connected
    Unhealthy, // ✗  linked but something wrong
    Active,    // ●  in config, not linked
}

// --- Settings ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsItem {
    pub kind: SettingsItemKind,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsItemKind {
    AccountHeader(String),  // account_id
    ApiKey(String),         // account_id
    Tunnel(String),         // tunnel name
    Spacer,
    AddAccount,
    ActionScanPorts,
    ActionImportPlists,
    ActionSyncCf,
}

// --- Mode and field enums ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddField {
    Name,
    Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddPortField {
    Port,
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Linking {
        port: u16,
        hostname: String,
        tunnel_name: String,
        old_hostname: Option<String>,
    },
    ConfirmingUnlink {
        port: u16,
        hostname: String,
    },
    AddingPort {
        field: AddPortField,
        port: String,
        name: String,
    },
    Settings {
        items: Vec<SettingsItem>,
        selected: usize,
    },
    Adding {
        field: AddField,
        name: String,
        token: String,
    },
    Editing {
        name: String,
        token: String,
    },
    AddingApiToken {
        input: String,
    },
    Confirming {
        action: String,
        target: String,
    },
    ConfirmingServiceDelete {
        idx: usize,
        name: String,
        port: u16,
    },
    Migrating {
        daemon_plists: Vec<std::path::PathBuf>,
    },
    Logs {
        name: String,
        content: String,
    },
    Help,
}

pub fn settings_item_selectable(kind: &SettingsItemKind) -> bool {
    !matches!(kind, SettingsItemKind::AccountHeader(_) | SettingsItemKind::Spacer)
}

pub struct App {
    pub config: Config,
    pub rows: Vec<PortRow>,
    pub tunnel_info: HashMap<String, TunnelInfo>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub unreached: Vec<UnreachedAccount>,
    pub selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub loading: Option<String>,
    pub spinner_tick: usize,
    pub last_sync: Option<std::time::Instant>,
    pub return_to_settings: bool,
    bg_tx: mpsc::Sender<BgResult>,
    bg_rx: mpsc::Receiver<BgResult>,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            config,
            rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            unreached: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            loading: Some("Syncing Cloudflare...".into()),
            spinner_tick: 0,
            last_sync: None,
            return_to_settings: false,
            bg_tx: tx,
            bg_rx: rx,
        };
        app.rebuild_rows();
        app.spawn_cf_sync();
        app
    }

    pub fn rebuild_rows(&mut self) {
        self.rows.clear();

        for svc in &self.config.services {
            let routes = self.ingress_routes.get(&svc.port);
            let (url, health, tunnel_name) = if let Some(routes) = routes.filter(|r| !r.is_empty()) {
                let best = routes.iter()
                    .find(|r| {
                        self.tunnel_info.get(&r.tunnel_id)
                            .map_or(false, |info| info.connection_count > 0)
                    })
                    .or(routes.first());

                if let Some(route) = best {
                    let tunnel_healthy = self.tunnel_info.get(&route.tunnel_id)
                        .map_or(false, |info| info.connection_count > 0);
                    let health = if tunnel_healthy { Health::Healthy } else { Health::Unhealthy };
                    (Some(route.hostname.clone()), health, Some(route.tunnel_name.clone()))
                } else {
                    (None, Health::Active, svc.tunnel.clone())
                }
            } else {
                (None, Health::Active, svc.tunnel.clone())
            };

            self.rows.push(PortRow {
                port: svc.port,
                name: svc.name.clone(),
                url,
                health,
                tunnel_name,
                memo: svc.memo.clone(),
            });
        }

        self.rows.sort_by_key(|r| r.port);

        if self.rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
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
                    self.last_sync = Some(std::time::Instant::now());
                    self.rebuild_rows();
                    if !self.unreached.is_empty() {
                        self.begin_add_api_token();
                    }
                }
                BgResult::LinkComplete { status_msg }
                | BgResult::UnlinkComplete { status_msg } => {
                    self.status_msg = Some(status_msg);
                    self.loading = Some("Syncing...".into());
                    self.spawn_cf_sync();
                }
            }
        }
    }

    // --- Navigation ---

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

    pub fn selected_row(&self) -> Option<&PortRow> {
        self.rows.get(self.selected)
    }

    // --- Link / Unlink ---

    pub fn begin_link(&mut self) {
        let row = match self.selected_row().cloned() {
            Some(r) => r,
            None => return,
        };

        let tunnel_name = match self.resolve_tunnel_name_for_port(row.port) {
            Some(name) => name,
            None => {
                if self.config.tunnels.is_empty() {
                    self.status_msg = Some("No tunnels — press . to add one".into());
                } else {
                    self.status_msg = Some("Could not resolve tunnel".into());
                }
                return;
            }
        };

        let old_hostname = row.url.clone();
        let hostname = old_hostname.clone().unwrap_or_default();

        self.mode = Mode::Linking {
            port: row.port,
            hostname,
            tunnel_name,
            old_hostname,
        };
    }

    pub fn finish_link(&mut self, port: u16, new_hostname: String, tunnel_name: String, old_hostname: Option<String>) {
        if new_hostname.is_empty() {
            self.status_msg = Some("Hostname cannot be empty".into());
            self.mode = Mode::Normal;
            return;
        }

        // If hostname unchanged, nothing to do
        if old_hostname.as_deref() == Some(&new_hostname) {
            self.status_msg = Some("Unchanged".into());
            self.mode = Mode::Normal;
            return;
        }

        let tunnel = match self.config.tunnels.iter().find(|t| t.name == tunnel_name) {
            Some(t) => t.clone(),
            None => {
                self.status_msg = Some("Tunnel not found".into());
                self.mode = Mode::Normal;
                return;
            }
        };

        let payload = match config::decode_token(&tunnel.token) {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Could not decode tunnel token".into());
                self.mode = Mode::Normal;
                return;
            }
        };

        // Auto-start tunnel if not running
        if !matches!(launchd::status(&tunnel.name), launchd::Status::Running { .. }) {
            let _ = launchd::start(&tunnel.name, &tunnel.token);
        }

        self.loading = Some("Linking...".into());
        self.mode = Mode::Normal;
        let tx = self.bg_tx.clone();
        let api_tokens = self.config.owned_api_tokens();
        let account_id = payload.account_id;
        let tunnel_id = payload.tunnel_id;
        let service_url = format!("http://localhost:{}", port);

        std::thread::spawn(move || {
            let api_token = match api_tokens.iter().find(|t| {
                cloudflare::verify_token(t, &account_id, &tunnel_id)
            }) {
                Some(t) => t.clone(),
                None => {
                    let _ = tx.send(BgResult::LinkComplete {
                        status_msg: "No API token with access — press . to add one".into(),
                    });
                    return;
                }
            };

            // Add new route
            let add_result = cloudflare::add_route(&api_token, &account_id, &tunnel_id, &new_hostname, &service_url);
            let add_ok = match &add_result {
                Ok(cloudflare::RouteResult::Ok) | Ok(cloudflare::RouteResult::AlreadyExists) => true,
                Ok(cloudflare::RouteResult::DnsFailure(_)) => true, // route added, DNS failed
                Err(_) => false,
            };

            // If we're renaming (old hostname exists), remove old route
            if add_ok {
                if let Some(ref old) = old_hostname {
                    let _ = cloudflare::remove_route(&api_token, &account_id, &tunnel_id, old);
                }
            }

            let status_msg = match add_result {
                Ok(cloudflare::RouteResult::Ok) => {
                    if old_hostname.is_some() {
                        format!("✓ :{} → {}", port, new_hostname)
                    } else {
                        format!("✓ :{} → {} (route + DNS)", port, new_hostname)
                    }
                }
                Ok(cloudflare::RouteResult::AlreadyExists) => {
                    format!("✓ :{} → {} (exists)", port, new_hostname)
                }
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => {
                    format!("⚠ Route ok, DNS failed: {}", e)
                }
                Err(e) => {
                    if e.contains("10000") || e.contains("Authentication") {
                        format!("✗ API token needs Tunnel:Edit permission — {}", e)
                    } else {
                        format!("✗ {}", e)
                    }
                }
            };
            let _ = tx.send(BgResult::LinkComplete { status_msg });
        });
    }

    pub fn handle_delete(&mut self) {
        let row = match self.selected_row().cloned() {
            Some(r) => r,
            None => return,
        };

        if let Some(hostname) = row.url {
            self.mode = Mode::ConfirmingUnlink {
                port: row.port,
                hostname,
            };
        } else if let Some(idx) = self.config.services.iter().position(|s| s.port == row.port) {
            let svc = &self.config.services[idx];
            self.mode = Mode::ConfirmingServiceDelete {
                idx,
                name: svc.name.clone(),
                port: svc.port,
            };
        }
    }

    pub fn finish_unlink(&mut self, port: u16, hostname: String) {
        let tunnel_id = match self.ingress_routes.get(&port).and_then(|r| r.first()) {
            Some(r) => r.tunnel_id.clone(),
            None => {
                self.status_msg = Some("No route found".into());
                self.mode = Mode::Normal;
                return;
            }
        };

        let tunnel = match self.config.find_tunnel_by_tunnel_id(&tunnel_id) {
            Some(t) => t.clone(),
            None => {
                self.status_msg = Some("Tunnel not found for this route".into());
                self.mode = Mode::Normal;
                return;
            }
        };

        let payload = match config::decode_token(&tunnel.token) {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Could not decode tunnel token".into());
                self.mode = Mode::Normal;
                return;
            }
        };

        self.loading = Some("Unlinking...".into());
        self.mode = Mode::Normal;
        let tx = self.bg_tx.clone();
        let api_tokens = self.config.owned_api_tokens();
        let account_id = payload.account_id;
        let tunnel_id_owned = payload.tunnel_id;

        std::thread::spawn(move || {
            let api_token = match api_tokens.iter().find(|t| {
                cloudflare::verify_token(t, &account_id, &tunnel_id_owned)
            }) {
                Some(t) => t.clone(),
                None => {
                    let _ = tx.send(BgResult::UnlinkComplete {
                        status_msg: "No API token with access — press . to add one".into(),
                    });
                    return;
                }
            };

            let status_msg = match cloudflare::remove_route(&api_token, &account_id, &tunnel_id_owned, &hostname) {
                Ok(cloudflare::RouteResult::Ok) => format!("✓ Unlinked {}", hostname),
                Ok(cloudflare::RouteResult::DnsFailure(ref e)) => format!("⚠ Route removed, DNS failed: {}", e),
                Ok(cloudflare::RouteResult::AlreadyExists) => unreachable!(),
                Err(e) => format!("✗ {}", e),
            };
            let _ = tx.send(BgResult::UnlinkComplete { status_msg });
        });
    }

    pub fn delete_service(&mut self, idx: usize) {
        let name = self.config.services.get(idx).map(|s| s.name.clone());
        match self.config.remove_service_by_idx(idx) {
            Ok(()) => self.status_msg = Some(format!("Removed '{}'", name.unwrap_or_default())),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.rebuild_rows();
    }

    // --- Add port ---

    pub fn begin_add_port(&mut self) {
        self.mode = Mode::AddingPort {
            field: AddPortField::Port,
            port: String::new(),
            name: String::new(),
        };
    }

    pub fn finish_add_port(&mut self, port_str: String, name: String) {
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_msg = Some("Invalid port number".into());
                self.mode = Mode::Normal;
                return;
            }
        };
        let name = if name.is_empty() { format!("port-{}", port) } else { name };
        let tunnel = self.config.tunnels.first().map(|t| t.name.clone());
        match self.config.add_service(name.clone(), port, tunnel, None) {
            Ok(()) => self.status_msg = Some(format!("Added :{}", port)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.mode = Mode::Normal;
        self.rebuild_rows();
    }

    // --- Logs ---

    pub fn show_logs_for_port(&mut self) {
        let row = match self.selected_row().cloned() {
            Some(r) => r,
            None => return,
        };

        let tunnel_name = row.tunnel_name.or_else(|| {
            self.resolve_tunnel_name_for_port(row.port)
        });

        match tunnel_name {
            Some(name) => {
                let content = launchd::read_logs(&name, 40).unwrap_or_default();
                self.mode = Mode::Logs { name, content };
            }
            None => {
                self.status_msg = Some("No tunnel associated".into());
            }
        }
    }

    // --- Settings ---

    pub fn open_settings(&mut self) {
        let items = self.build_settings_items();
        let selected = items.iter()
            .position(|i| settings_item_selectable(&i.kind))
            .unwrap_or(0);
        self.mode = Mode::Settings { items, selected };
    }

    pub fn build_settings_items(&self) -> Vec<SettingsItem> {
        use std::collections::BTreeMap;

        // Group tunnels by account_id
        let mut accounts: BTreeMap<String, Vec<&config::Tunnel>> = BTreeMap::new();
        for tunnel in &self.config.tunnels {
            let account_id = config::decode_token(&tunnel.token)
                .map(|p| p.account_id)
                .unwrap_or_else(|_| "unknown".into());
            accounts.entry(account_id).or_default().push(tunnel);
        }

        // Determine which accounts have API tokens based on cached sync data.
        // Unreached accounts have no working token. Reached accounts have at least
        // one token that worked — show the first configured token whose account
        // matches (by checking if tunnel_info has data for any tunnel in that account).
        let unreached_ids: HashSet<String> = self.unreached.iter()
            .map(|u| u.account_id.clone())
            .collect();

        let mut matched_api: HashMap<String, String> = HashMap::new();
        for (account_id, tunnels) in &accounts {
            if unreached_ids.contains(account_id) {
                continue;
            }
            // Check if any tunnel in this account has info (meaning sync reached it)
            let reached = tunnels.iter().any(|t| {
                config::decode_token(&t.token)
                    .map(|p| self.tunnel_info.contains_key(&p.tunnel_id))
                    .unwrap_or(false)
            });
            if reached {
                // Find the per-tunnel API token or first global one
                let tok = tunnels.iter()
                    .find_map(|t| t.api_token.as_ref())
                    .or_else(|| self.config.cf_api_tokens.first());
                if let Some(tok) = tok {
                    matched_api.insert(account_id.clone(), tok.clone());
                }
            }
        }

        let mut items = Vec::new();

        for (account_id, tunnels) in &accounts {
            // Account header — use short account_id or CF name if available
            let account_label = if account_id.len() > 12 {
                format!("{}…", &account_id[..12])
            } else {
                account_id.clone()
            };
            items.push(SettingsItem {
                kind: SettingsItemKind::AccountHeader(account_id.clone()),
                label: account_label,
                detail: String::new(),
            });

            // API key row
            let api_detail = if let Some(tok) = matched_api.get(account_id) {
                let masked = if tok.len() > 4 {
                    format!("••••{}", &tok[tok.len()-4..])
                } else {
                    "••••".into()
                };
                masked
            } else {
                "(none)".into()
            };
            items.push(SettingsItem {
                kind: SettingsItemKind::ApiKey(account_id.clone()),
                label: "api key".into(),
                detail: api_detail,
            });

            // Tunnel rows
            for tunnel in tunnels {
                let status = launchd::status(&tunnel.name);
                let status_str = match status {
                    launchd::Status::Running { .. } => "running",
                    launchd::Status::Stopped => "stopped",
                    launchd::Status::Inactive => "inactive",
                };
                items.push(SettingsItem {
                    kind: SettingsItemKind::Tunnel(tunnel.name.clone()),
                    label: tunnel.name.clone(),
                    detail: status_str.into(),
                });
            }

            // Spacer between accounts
            items.push(SettingsItem {
                kind: SettingsItemKind::Spacer,
                label: String::new(),
                detail: String::new(),
            });
        }

        // Add account
        items.push(SettingsItem {
            kind: SettingsItemKind::AddAccount,
            label: "+ add account".into(),
            detail: String::new(),
        });

        // Spacer
        items.push(SettingsItem {
            kind: SettingsItemKind::Spacer,
            label: String::new(),
            detail: String::new(),
        });

        // Utility actions
        items.push(SettingsItem { kind: SettingsItemKind::ActionScanPorts, label: "Scan ports".into(), detail: String::new() });
        items.push(SettingsItem { kind: SettingsItemKind::ActionImportPlists, label: "Import plists".into(), detail: String::new() });
        items.push(SettingsItem { kind: SettingsItemKind::ActionSyncCf, label: "Sync from Cloudflare".into(), detail: String::new() });

        items
    }

    /// Return to settings if we came from there, otherwise Normal mode.
    pub fn dismiss_or_settings(&mut self) {
        if self.return_to_settings {
            self.return_to_settings = false;
            self.open_settings();
        } else {
            self.mode = Mode::Normal;
        }
    }

    // --- Tunnel CRUD (from settings) ---

    pub fn begin_add(&mut self) {
        self.mode = Mode::Adding {
            field: AddField::Name,
            name: String::new(),
            token: String::new(),
        };
    }

    pub fn finish_add(&mut self, name: String, token: String) {
        match self.config.add(name.clone(), token) {
            Ok(()) => self.status_msg = Some(format!("Added tunnel '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.dismiss_or_settings();
        self.rebuild_rows();
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
        self.dismiss_or_settings();
        self.rebuild_rows();
    }

    pub fn delete_tunnel_by_name(&mut self, name: &str) {
        let _ = launchd::stop(name);
        match self.config.remove(name) {
            Ok(()) => self.status_msg = Some(format!("Deleted '{}'", name)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }
        self.rebuild_rows();
    }

    // --- API token ---

    pub fn begin_add_api_token(&mut self) {
        self.mode = Mode::AddingApiToken {
            input: String::new(),
        };
    }

    pub fn finish_add_api_token(&mut self, token: String) {
        let matched_unreached: Option<&UnreachedAccount> = self.unreached.iter().find(|a| {
            cloudflare::verify_token(&token, &a.account_id, &a.tunnel_id)
        });

        let description = if let Some(a) = matched_unreached {
            a.tunnel_names.join(", ")
        } else {
            let matched_tunnel = self.config.tunnels.iter().find(|t| {
                if let Ok(p) = config::decode_token(&t.token) {
                    cloudflare::verify_token(&token, &p.account_id, &p.tunnel_id)
                } else {
                    false
                }
            });
            if let Some(t) = matched_tunnel {
                t.name.clone()
            } else if let Some(zones) = cloudflare::verify_token_has_zones(&token) {
                format!("DNS zones: {}", zones.join(", "))
            } else {
                self.status_msg = Some(
                    "Token rejected — doesn't match any tunnel account or DNS zone".into(),
                );
                return;
            }
        };

        match self.config.add_api_token(token) {
            Ok(()) => self.status_msg = Some(format!("Token added for {}", description)),
            Err(e) => self.status_msg = Some(format!("Error: {}", e)),
        }

        self.return_to_settings = false; // sync will rebuild everything
        self.mode = Mode::Normal;
        self.loading = Some("Syncing Cloudflare...".into());
        self.spawn_cf_sync();
    }

    // --- CF sync ---

    pub fn refresh_cf(&mut self) {
        self.loading = Some("Syncing Cloudflare...".into());
        self.spawn_cf_sync();
    }

    fn spawn_cf_sync(&self) {
        let tx = self.bg_tx.clone();
        let tunnel_tokens: Vec<(String, String)> = self.config.tunnels.iter()
            .map(|t| (t.name.clone(), t.token.clone()))
            .collect();
        let cf_tokens = self.config.owned_api_tokens();

        std::thread::spawn(move || {
            let refs: Vec<&str> = cf_tokens.iter().map(|s| s.as_str()).collect();
            let sync = cloudflare::sync(&refs, &tunnel_tokens);
            let _ = tx.send(BgResult::CfSync(sync));
        });
    }

    // --- Import / Migrate ---

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
            self.rebuild_rows();
            return;
        }
        self.rebuild_rows();
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
        self.rebuild_rows();
    }

    // --- Scan ---

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
        self.rebuild_rows();
    }

    // --- Internal helpers ---

    fn resolve_tunnel_name_for_port(&self, port: u16) -> Option<String> {
        // 1. Check service.tunnel field
        let service = self.config.services.iter().find(|s| s.port == port);
        if let Some(svc) = service {
            if let Some(ref name) = svc.tunnel {
                if self.config.tunnels.iter().any(|t| t.name == *name) {
                    return Some(name.clone());
                }
            }
        }

        // 2. Check existing routes from sync data
        if let Some(routes) = self.ingress_routes.get(&port) {
            if let Some(route) = routes.first() {
                if let Some(tunnel) = self.config.find_tunnel_by_tunnel_id(&route.tunnel_id) {
                    return Some(tunnel.name.clone());
                }
            }
        }

        // 3. First available tunnel
        self.config.tunnels.first().map(|t| t.name.clone())
    }

    #[cfg(test)]
    pub fn test_app(config: config::Config, ingress_routes: HashMap<u16, Vec<IngressRoute>>) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            config,
            rows: Vec::new(),
            tunnel_info: HashMap::new(),
            ingress_routes,
            unreached: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
            loading: None,
            spinner_tick: 0,
            last_sync: None,
            return_to_settings: false,
            bg_tx: tx,
            bg_rx: rx,
        };
        app.rebuild_rows();
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
    fn find_tunnel_by_tunnel_id_found() {
        let tunnels = vec![Tunnel {
            name: "my-tunnel".into(),
            token: TEST_TOKEN.into(),
            api_token: None,
        }];
        let config = Config {
            tunnels,
            services: Vec::new(),
            cf_api_tokens: Vec::new(),
            cf_api_token: None,
        };
        let result = config.find_tunnel_by_tunnel_id("tun456");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "my-tunnel");
    }

    #[test]
    fn find_tunnel_by_tunnel_id_not_found() {
        let config = Config {
            tunnels: Vec::new(),
            services: Vec::new(),
            cf_api_tokens: Vec::new(),
            cf_api_token: None,
        };
        assert!(config.find_tunnel_by_tunnel_id("nonexistent").is_none());
    }

    #[test]
    fn rebuild_rows_sorts_by_port() {
        let services = vec![
            Service { name: "b".into(), port: 8080, machine: String::new(), tunnel: None, memo: None },
            Service { name: "a".into(), port: 3000, machine: String::new(), tunnel: None, memo: None },
        ];
        let app = make_app(services, Vec::new(), HashMap::new());
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.rows[0].port, 3000);
        assert_eq!(app.rows[1].port, 8080);
    }

    #[test]
    fn rebuild_rows_health_active_when_no_routes() {
        let services = vec![
            Service { name: "test".into(), port: 3000, machine: String::new(), tunnel: None, memo: None },
        ];
        let app = make_app(services, Vec::new(), HashMap::new());
        assert_eq!(app.rows[0].health, Health::Active);
        assert!(app.rows[0].url.is_none());
    }
}
