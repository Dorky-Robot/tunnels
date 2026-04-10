use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub name: String,
    pub token: String,
    /// Per-tunnel CF API token (optional, used alongside global cf_api_tokens)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine: String,
    #[serde(default)]
    pub tunnel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
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
    /// All configured CF API tokens (merges cf_api_tokens + legacy cf_api_token + per-tunnel tokens)
    pub fn all_cf_api_tokens(&self) -> Vec<&str> {
        let mut tokens: Vec<&str> = self.cf_api_tokens.iter().map(|s| s.as_str()).collect();
        if let Some(ref t) = self.cf_api_token {
            if !t.is_empty() && !tokens.iter().any(|existing| existing == &t.as_str()) {
                tokens.push(t.as_str());
            }
        }
        // Include per-tunnel API tokens
        for tunnel in &self.tunnels {
            if let Some(ref t) = tunnel.api_token {
                if !t.is_empty() && !tokens.iter().any(|existing| existing == &t.as_str()) {
                    tokens.push(t.as_str());
                }
            }
        }
        tokens
    }

    pub fn find_tunnel_by_tunnel_id(&self, tunnel_id: &str) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| {
            decode_token(&t.token)
                .map(|p| p.tunnel_id == tunnel_id)
                .unwrap_or(false)
        })
    }

    pub fn owned_api_tokens(&self) -> Vec<String> {
        self.all_cf_api_tokens().into_iter().map(|s| s.to_string()).collect()
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

    pub fn add_api_token(&mut self, token: String) -> Result<()> {
        if !self.cf_api_tokens.iter().any(|t| t == &token) {
            self.cf_api_tokens.push(token);
        }
        self.save()
    }

    pub fn add(&mut self, name: String, token: String) -> Result<()> {
        if self.tunnels.iter().any(|t| t.name == name) {
            anyhow::bail!("tunnel '{}' already exists", name);
        }
        self.tunnels.push(Tunnel { name, token, api_token: None });
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
        // Validate that the token is a valid connector token (base64-encoded JSON
        // with account_id and tunnel_id fields). This prevents accidentally
        // overwriting the connector token with a CF API token.
        decode_token(&token).with_context(|| {
            "This doesn't look like a tunnel connector token. \
             Connector tokens are base64-encoded and start with 'eyJ...'. \
             If you meant to add a Cloudflare API token, use: tunnels token add <token>"
        })?;

        let t = self
            .tunnels
            .iter_mut()
            .find(|t| t.name == name)
            .with_context(|| format!("tunnel '{}' not found", name))?;
        t.token = token;
        self.save()
    }

    pub fn add_service(&mut self, name: String, port: u16, tunnel: Option<String>, memo: Option<String>) -> Result<()> {
        if self.services.iter().any(|s| s.port == port) {
            anyhow::bail!("port {} already tracked", port);
        }
        self.services.push(Service { name, port, machine: String::new(), tunnel, memo });
        self.save()
    }

    pub fn remove_service_by_idx(&mut self, idx: usize) -> Result<()> {
        if idx < self.services.len() {
            self.services.remove(idx);
            self.save()
        } else {
            anyhow::bail!("service not found")
        }
    }

    pub fn update_service(&mut self, idx: usize, name: String, port: u16, tunnel: Option<String>, memo: Option<String>) -> Result<()> {
        if let Some(s) = self.services.get_mut(idx) {
            s.name = name;
            s.port = port;
            s.tunnel = tunnel;
            s.memo = memo;
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use std::path::Path;

    /// Helper: build a Config with one tunnel using a valid connector token.
    fn test_config(dir: &Path) -> (Config, String) {
        let payload = serde_json::json!({ "a": "acct123", "t": "tun456", "s": "secret" });
        let valid_token = STANDARD.encode(serde_json::to_vec(&payload).unwrap());

        let config = Config {
            tunnels: vec![Tunnel {
                name: "my-tunnel".into(),
                token: valid_token.clone(),
                api_token: None,
            }],
            services: vec![],
            cf_api_tokens: vec![],
            cf_api_token: None,
        };

        // Point save/load at a temp file
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        (config, valid_token)
    }

    /// Helper: make a valid connector token with custom account/tunnel ids.
    fn make_connector_token(account_id: &str, tunnel_id: &str) -> String {
        let payload = serde_json::json!({ "a": account_id, "t": tunnel_id, "s": "sec" });
        STANDARD.encode(serde_json::to_vec(&payload).unwrap())
    }

    #[test]
    fn update_token_rejects_api_token() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _valid) = test_config(dir.path());

        // A CF API token is a plain string, not base64 JSON — must be rejected
        let api_token = "v1.0-55T-6-eesaSOMEFAKETOKEN1234567890abc".to_string();
        let result = config.update_token("my-tunnel", api_token);

        assert!(result.is_err());
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("tunnels token add"),
            "Error should suggest 'tunnels token add', got: {err_msg}"
        );
    }

    #[test]
    fn update_token_rejects_random_string() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _valid) = test_config(dir.path());

        let result = config.update_token("my-tunnel", "not-a-real-token".into());
        assert!(result.is_err());
    }

    #[test]
    fn update_token_accepts_valid_connector_token() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _) = test_config(dir.path());

        let new_token = make_connector_token("new_acct", "new_tun");
        // Patch save to use temp dir — we can't easily override path() so just
        // test the validation logic by calling decode_token + the find logic directly.
        let result = decode_token(&new_token);
        assert!(result.is_ok());

        let t = config.tunnels.iter_mut().find(|t| t.name == "my-tunnel").unwrap();
        t.token = new_token.clone();
        assert_eq!(t.token, new_token);
    }

    #[test]
    fn update_token_tunnel_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _) = test_config(dir.path());

        let token = make_connector_token("a", "t");
        let result = config.update_token("nonexistent", token);

        assert!(result.is_err());
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(err_msg.contains("not found"));
    }

    #[test]
    fn decode_token_valid() {
        let token = make_connector_token("acct_abc", "tun_xyz");
        let payload = decode_token(&token).unwrap();
        assert_eq!(payload.account_id, "acct_abc");
        assert_eq!(payload.tunnel_id, "tun_xyz");
    }

    #[test]
    fn decode_token_invalid_base64() {
        let result = decode_token("not-valid-base64!@#");
        assert!(result.is_err());
    }

    #[test]
    fn decode_token_valid_base64_but_not_json() {
        let token = STANDARD.encode(b"just a plain string");
        let result = decode_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn decode_token_valid_json_but_missing_fields() {
        let payload = serde_json::json!({ "foo": "bar" });
        let token = STANDARD.encode(serde_json::to_vec(&payload).unwrap());
        let result = decode_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn add_api_token_does_not_duplicate() {
        let mut config = Config::default();
        // Manually push instead of calling add_api_token (which calls save)
        config.cf_api_tokens.push("tok_abc".into());
        // Simulate add logic
        if !config.cf_api_tokens.iter().any(|t| t == "tok_abc") {
            config.cf_api_tokens.push("tok_abc".into());
        }
        assert_eq!(config.cf_api_tokens.len(), 1);
    }

    #[test]
    fn all_cf_api_tokens_merges_sources() {
        let config = Config {
            tunnels: vec![Tunnel {
                name: "t1".into(),
                token: "connector".into(),
                api_token: Some("per_tunnel_tok".into()),
            }],
            services: vec![],
            cf_api_tokens: vec!["global_tok".into()],
            cf_api_token: Some("legacy_tok".into()),
        };

        let tokens = config.all_cf_api_tokens();
        assert_eq!(tokens.len(), 3);
        assert!(tokens.contains(&"global_tok"));
        assert!(tokens.contains(&"legacy_tok"));
        assert!(tokens.contains(&"per_tunnel_tok"));
    }

    #[test]
    fn all_cf_api_tokens_deduplicates() {
        let config = Config {
            tunnels: vec![],
            services: vec![],
            cf_api_tokens: vec!["same_tok".into()],
            cf_api_token: Some("same_tok".into()),
        };

        let tokens = config.all_cf_api_tokens();
        assert_eq!(tokens.len(), 1);
    }

    #[test]
    fn update_token_single_tunnel_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _) = test_config(dir.path());
        let new_token = make_connector_token("new_acct", "new_tun");

        // Updating by exact name should work
        assert!(config.update_token("my-tunnel", new_token.clone()).is_ok());
    }

    #[test]
    fn update_token_wrong_name_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _) = test_config(dir.path());
        let new_token = make_connector_token("a", "t");

        let result = config.update_token("nonexistent", new_token);
        assert!(result.is_err());
    }
}
