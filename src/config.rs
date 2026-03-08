use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub name: String,
    pub token: String,
    /// CF API token for this tunnel's account (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    /// Cached account_id decoded from the cloudflared token
    #[serde(skip)]
    pub account_id: String,
    /// Cached tunnel_id decoded from the cloudflared token
    #[serde(skip)]
    pub tunnel_id: String,
}

impl Tunnel {
    /// Create a new tunnel, decoding the token to cache account_id and tunnel_id
    pub fn new(name: String, token: String, api_token: Option<String>) -> Self {
        let (account_id, tunnel_id) = decode_token(&token)
            .map(|p| (p.account_id, p.tunnel_id))
            .unwrap_or_default();
        Self {
            name,
            token,
            api_token,
            account_id,
            tunnel_id,
        }
    }

    /// Re-derive cached fields from the token (called after deserialization)
    fn hydrate(&mut self) {
        if let Ok(payload) = decode_token(&self.token) {
            self.account_id = payload.account_id;
            self.tunnel_id = payload.tunnel_id;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub port: u16,
    pub machine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub services: Vec<Service>,
    /// Legacy: global CF API tokens (migrated to per-tunnel on load)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cf_api_tokens: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_api_token: Option<String>,
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".config"))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tunnels")
            .join("config.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut config: Self = serde_json::from_str(&data).with_context(|| "parsing config")?;
        // Hydrate cached fields from tokens after deserialization
        for t in &mut config.tunnels {
            t.hydrate();
        }
        config.migrate_legacy_tokens();
        Ok(config)
    }

    /// Migrate global cf_api_tokens / cf_api_token onto tunnels by account_id
    fn migrate_legacy_tokens(&mut self) {
        let mut legacy: Vec<String> = std::mem::take(&mut self.cf_api_tokens);
        if let Some(single) = self.cf_api_token.take()
            && !single.is_empty()
            && !legacy.contains(&single)
        {
            legacy.push(single);
        }

        if legacy.is_empty() {
            return;
        }

        // Group tunnels by account_id (using cached fields)
        let mut account_tunnels: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, t) in self.tunnels.iter().enumerate() {
            if !t.account_id.is_empty() {
                account_tunnels
                    .entry(t.account_id.clone())
                    .or_default()
                    .push(i);
            }
        }

        // Try each legacy token against each account
        for api_token in &legacy {
            for (account_id, indices) in &account_tunnels {
                // Skip if tunnels in this account already have an api_token
                if indices.iter().any(|&i| self.tunnels[i].api_token.is_some()) {
                    continue;
                }
                // Probe with the first tunnel's cached tunnel_id
                let probe_tunnel_id = indices
                    .first()
                    .map(|&i| self.tunnels[i].tunnel_id.clone())
                    .filter(|id| !id.is_empty());
                if let Some(tid) = probe_tunnel_id
                    && crate::cloudflare::verify_token(api_token, account_id, &tid)
                {
                    for &i in indices {
                        self.tunnels[i].api_token = Some(api_token.clone());
                    }
                }
            }
        }

        // Save to persist migration (clears legacy fields)
        let _ = self.save();
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;

        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &data)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    pub fn add(&mut self, name: String, token: String) -> Result<()> {
        if self.tunnels.iter().any(|t| t.name == name) {
            anyhow::bail!("tunnel '{}' already exists", name);
        }
        self.tunnels.push(Tunnel::new(name, token, None));
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let len = self.tunnels.len();
        self.tunnels.retain(|t| t.name != name);
        if self.tunnels.len() == len {
            anyhow::bail!("tunnel '{}' not found", name);
        }
        self.save()
    }

    pub fn rename(&mut self, old_name: &str, new_name: String) -> Result<()> {
        if self.tunnels.iter().any(|t| t.name == new_name) {
            anyhow::bail!("tunnel '{}' already exists", new_name);
        }
        let t = self
            .tunnels
            .iter_mut()
            .find(|t| t.name == old_name)
            .with_context(|| format!("tunnel '{}' not found", old_name))?;
        t.name = new_name;
        self.save()
    }

    pub fn update_token(&mut self, name: &str, token: String) -> Result<()> {
        let t = self
            .tunnels
            .iter_mut()
            .find(|t| t.name == name)
            .with_context(|| format!("tunnel '{}' not found", name))?;
        t.token = token.clone();
        // Re-derive cached fields from new token
        if let Ok(payload) = decode_token(&token) {
            t.account_id = payload.account_id;
            t.tunnel_id = payload.tunnel_id;
        }
        self.save()
    }

    pub fn set_api_token(&mut self, name: &str, api_token: String) -> Result<()> {
        let account_id = self
            .tunnels
            .iter()
            .find(|t| t.name == name)
            .with_context(|| format!("tunnel '{}' not found", name))?
            .account_id
            .clone();

        for t in &mut self.tunnels {
            if t.account_id == account_id {
                t.api_token = Some(api_token.clone());
            }
        }
        self.save()
    }

    pub fn clear_api_token(&mut self, name: &str) -> Result<()> {
        let account_id = self
            .tunnels
            .iter()
            .find(|t| t.name == name)
            .with_context(|| format!("tunnel '{}' not found", name))?
            .account_id
            .clone();

        for t in &mut self.tunnels {
            if t.account_id == account_id {
                t.api_token = None;
            }
        }
        self.save()
    }

    pub fn add_service(&mut self, name: String, port: u16, machine: String) -> Result<()> {
        if self
            .services
            .iter()
            .any(|s| s.port == port && s.machine == machine)
        {
            anyhow::bail!("port {} on '{}' already tracked", port, machine);
        }
        self.services.push(Service {
            name,
            port,
            machine,
        });
        self.save()
    }

    pub fn remove_service(&mut self, name: &str, port: u16, machine: &str) -> Result<()> {
        let len = self.services.len();
        self.services
            .retain(|s| !(s.name == name && s.port == port && s.machine == machine));
        if self.services.len() == len {
            anyhow::bail!("service not found");
        }
        self.save()
    }

    pub fn update_service(
        &mut self,
        idx: usize,
        name: String,
        port: u16,
        machine: String,
    ) -> Result<()> {
        if let Some(s) = self.services.get_mut(idx) {
            s.name = name;
            s.port = port;
            s.machine = machine;
            self.save()
        } else {
            anyhow::bail!("service index out of range")
        }
    }
}

/// Decode the JWT-like token to extract account_id and tunnel_id
#[derive(Debug, Deserialize)]
pub struct TokenPayload {
    #[serde(rename = "a")]
    pub account_id: String,
    #[serde(rename = "t")]
    pub tunnel_id: String,
}

pub fn decode_token(token: &str) -> Result<TokenPayload> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    let bytes = STANDARD.decode(token).with_context(|| "base64 decode")?;
    serde_json::from_slice(&bytes).with_context(|| "json decode token payload")
}
