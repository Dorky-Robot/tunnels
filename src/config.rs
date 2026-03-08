use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub port: u16,
    pub machine: String,
    #[serde(default)]
    pub tunnel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub services: Vec<Service>,
    /// Cloudflare API tokens — one per CF account.
    /// Create at https://dash.cloudflare.com/profile/api-tokens with
    /// "Account.Cloudflare Tunnel:Read" permission.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cf_api_tokens: Vec<String>,
    /// Backward compat: single token (deprecated, use cf_api_tokens)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_api_token: Option<String>,
}

impl Config {
    /// All configured CF API tokens (merges cf_api_tokens + legacy cf_api_token)
    pub fn all_cf_api_tokens(&self) -> Vec<&str> {
        let mut tokens: Vec<&str> = self.cf_api_tokens.iter().map(|s| s.as_str()).collect();
        if let Some(ref t) = self.cf_api_token {
            if !t.is_empty() && !tokens.iter().any(|existing| existing == &t.as_str()) {
                tokens.push(t.as_str());
            }
        }
        tokens
    }

    pub fn path() -> PathBuf {
        dirs::home_dir().map(|h| h.join(".config"))
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
        serde_json::from_str(&data).with_context(|| "parsing config")
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add(&mut self, name: String, token: String) -> Result<()> {
        if self.tunnels.iter().any(|t| t.name == name) {
            anyhow::bail!("tunnel '{}' already exists", name);
        }
        self.tunnels.push(Tunnel { name, token });
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
        t.token = token;
        self.save()
    }

    pub fn add_service(&mut self, name: String, port: u16, machine: String, tunnel: Option<String>) -> Result<()> {
        if self.services.iter().any(|s| s.name == name && s.machine == machine) {
            anyhow::bail!("service '{}' on '{}' already exists", name, machine);
        }
        self.services.push(Service { name, port, machine, tunnel });
        self.save()
    }

    pub fn remove_service(&mut self, name: &str, machine: &str) -> Result<()> {
        let len = self.services.len();
        self.services.retain(|s| !(s.name == name && s.machine == machine));
        if self.services.len() == len {
            anyhow::bail!("service not found");
        }
        self.save()
    }

    pub fn update_service(&mut self, idx: usize, name: String, port: u16, machine: String, tunnel: Option<String>) -> Result<()> {
        if let Some(s) = self.services.get_mut(idx) {
            s.name = name;
            s.port = port;
            s.machine = machine;
            s.tunnel = tunnel;
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
    #[allow(dead_code)]
    pub account_id: String,
    #[serde(rename = "t")]
    pub tunnel_id: String,
}

pub fn decode_token(token: &str) -> Result<TokenPayload> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    let bytes = STANDARD.decode(token).with_context(|| "base64 decode")?;
    serde_json::from_slice(&bytes).with_context(|| "json decode token payload")
}
