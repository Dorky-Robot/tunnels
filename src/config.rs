use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub tunnels: Vec<Tunnel>,
}

impl Config {
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
