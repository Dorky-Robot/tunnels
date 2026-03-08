use crate::config::{self, Config, Tunnel};
use crate::launchd;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Adding { field: AddField, name: String, token: String },
    Editing { name: String, token: String },
    Renaming { old_name: String, new_name: String },
    Confirming { action: String, target: String },
    Logs { name: String, content: String },
    Migrating { daemon_plists: Vec<std::path::PathBuf> },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddField {
    Name,
    Token,
}

#[derive(Debug, Clone)]
pub struct TunnelRow {
    pub name: String,
    pub token_preview: String,
    pub status: launchd::Status,
    pub tunnel_id: String,
}

pub struct App {
    pub config: Config,
    pub rows: Vec<TunnelRow>,
    pub selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let mut app = Self {
            config,
            rows: Vec::new(),
            selected: 0,
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
                let preview = if t.token.len() > 16 {
                    format!("{}...", &t.token[..16])
                } else {
                    t.token.clone()
                };
                let tunnel_id = config::decode_token(&t.token)
                    .map(|p| p.tunnel_id)
                    .unwrap_or_else(|_| "-".into());
                TunnelRow {
                    name: t.name.clone(),
                    token_preview: preview,
                    status,
                    tunnel_id,
                }
            })
            .collect();

        if self.selected >= self.rows.len() && !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
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
