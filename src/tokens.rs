//! The tokens on one machine, for looking at and managing from the web UI.
//!
//! Two kinds live in a machine's config.json:
//!
//! - **API tokens**: what this machine may do in Cloudflare (routes, DNS,
//!   `tunnels cf`). Shown by a hint and what they reach; identified by a
//!   fingerprint, so a removal names exactly one token even if the list
//!   changed since the page loaded.
//! - **Connector tokens**: what lets cloudflared run a tunnel here. Shown by
//!   tunnel, with whether it still matches what Cloudflare holds (a rotation
//!   elsewhere makes it stale), and re-fetched from Cloudflare on request.
//!
//! No view ever contains a token. A token added through the UI travels once,
//! to the machine that keeps it, and is never shown again.

use crate::cf::Client;
use crate::config::{self, Config, Reach};
use crate::fleet::Fleet;
use crate::launchd;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ApiTokenView {
    /// a fingerprint, for naming this token in a removal
    pub id: String,
    pub hint: String,
    pub covers: String,
    pub reach: Vec<Reach>,
    /// does Cloudflare accept it right now
    pub valid: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorView {
    /// the name in this machine's config and LaunchAgent label
    pub name: String,
    pub tunnel_id: Option<String>,
    pub alias: Option<String>,
    pub account: Option<String>,
    /// loaded, not loaded, no plist
    pub state: String,
    /// kept in a 0600 token file (true) or inline in the plist (false)
    pub token_file: bool,
    /// matches the connector token Cloudflare hands out now; None if no API
    /// token here can ask
    pub current: Option<bool>,
    /// the fleet runs this tunnel on some other machine
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs_elsewhere: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineTokens {
    pub machine: String,
    pub api_tokens: Vec<ApiTokenView>,
    pub connectors: Vec<ConnectorView>,
}

/// A short, stable name for a token that reveals nothing about it.
pub fn fingerprint(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// What a token reaches: its accounts and the zones in each.
pub fn reach_of(token: &str) -> Result<(String, Vec<Reach>)> {
    let client = Client::new(token);
    client.verify().context("Cloudflare does not accept this token")?;
    let zones = client.zones().unwrap_or_default();
    let mut reach: Vec<Reach> = Vec::new();
    for z in &zones {
        match reach.iter_mut().find(|r| r.account_id == z.account_id) {
            Some(r) => r.zones.push(z.name.clone()),
            None => reach.push(Reach { account_id: z.account_id.clone(), account_name: z.account_name.clone(), zones: vec![z.name.clone()] }),
        }
    }
    for a in client.accounts().unwrap_or_default() {
        if !reach.iter().any(|r| r.account_id == a.id) {
            reach.push(Reach { account_id: a.id, account_name: a.name, zones: vec![] });
        }
    }
    if reach.is_empty() {
        bail!("this token reaches no account and no zone — wrong Cloudflare account, or missing permissions");
    }
    for r in &mut reach {
        r.zones.sort();
    }
    let covers = reach.iter().map(|r| format!("{} ({})", r.account_name, r.zones.join(", "))).collect::<Vec<_>>().join(" · ");
    Ok((covers, reach))
}

/// Everything about this machine's tokens, with no token in it.
pub fn view(config: &Config, fleet: &Fleet, me: &str) -> MachineTokens {
    let api_tokens = config
        .api_tokens()
        .iter()
        .map(|t| ApiTokenView {
            id: fingerprint(&t.token),
            hint: t.hint(),
            covers: t.covers.clone(),
            reach: t.reach.clone(),
            valid: Some(Client::new(&t.token).verify().is_ok()),
        })
        .collect();
    let dir = crate::api::directory(config);
    let connectors = config
        .tunnels
        .iter()
        .map(|t| {
            let id = t.tunnel_id();
            let acct = t.account_id();
            let decl = id.as_deref().and_then(|i| fleet.alias_for_id(i).map(|a| (a.clone(), fleet.tunnels[a].clone())));
            let current = match (&id, &acct) {
                (Some(i), Some(a)) => dir.client_for_account(a).and_then(|c| c.connector_token(a, i).ok()).map(|fresh| {
                    config::decode_token(&fresh).map(|p| p.secret).ok() == config::decode_token(&t.token).map(|p| p.secret).ok()
                }),
                _ => None,
            };
            let state = match launchd::status(&t.name) {
                launchd::Status::Running { .. } => "loaded",
                launchd::Status::Stopped => "not loaded",
                launchd::Status::Inactive => "no plist",
            };
            ConnectorView {
                name: t.name.clone(),
                tunnel_id: id,
                alias: decl.as_ref().map(|(a, _)| a.clone()),
                account: acct.as_deref().and_then(|a| fleet.account_alias_for_id(a).cloned()).or(acct.clone()),
                state: state.into(),
                token_file: !launchd::plist_has_inline_token(&t.name),
                current,
                runs_elsewhere: decl.and_then(|(_, d)| d.machine).filter(|m| m != me),
            }
        })
        .collect();
    MachineTokens { machine: me.to_string(), api_tokens, connectors }
}

/// Keep an API token on this machine, after finding out what it reaches.
pub fn add(config: &mut Config, token: &str) -> Result<String> {
    let token = token.trim();
    if token.is_empty() {
        bail!("no token given");
    }
    if config::decode_token(token).is_ok() {
        bail!("that is a connector (tunnel) token, not an API token — connector tokens are fetched for you");
    }
    let (covers, reach) = reach_of(token)?;
    config.add_api_token(token.to_string(), covers.clone(), reach)?;
    Ok(covers)
}

/// Forget the API token with this fingerprint, on this machine only. It
/// still works in Cloudflare until it is revoked there.
pub fn remove(config: &mut Config, id: &str) -> Result<String> {
    let idx = config
        .api_tokens()
        .iter()
        .position(|t| fingerprint(&t.token) == id)
        .ok_or_else(|| anyhow!("no API token {id} on this machine (the list may have changed; reload)"))?;
    config.remove_api_token(idx)
}

/// Re-check every API token and relabel it with what it reaches now.
pub fn refresh(config: &mut Config) -> Vec<(String, std::result::Result<String, String>)> {
    let tokens: Vec<String> = config.api_tokens().iter().map(|t| t.token.clone()).collect();
    tokens
        .iter()
        .map(|t| {
            let r = reach_of(t).map_err(|e| format!("{e:#}")).and_then(|(covers, reach)| {
                config.add_api_token(t.clone(), covers.clone(), reach).map_err(|e| format!("{e:#}"))?;
                Ok(covers)
            });
            (fingerprint(t), r)
        })
        .collect()
}

/// Take the connector token Cloudflare hands out now for a tunnel on this
/// machine, and restart the tunnel on it if it changed. What a machine does
/// after its tunnel was rotated somewhere else.
pub fn refetch_connector(config: &mut Config, fleet: &Fleet, key: &str) -> Result<String> {
    let t = config
        .tunnel_by_name(key)
        .cloned()
        .or_else(|| fleet.find_tunnel(key).and_then(|(_, d)| config.tunnel_by_id(&d.id).cloned()))
        .ok_or_else(|| anyhow!("no tunnel `{key}` on this machine"))?;
    let (id, acct) = (t.tunnel_id().ok_or_else(|| anyhow!("its token does not decode"))?, t.account_id().unwrap_or_default());
    let dir = crate::api::directory(config);
    let client = dir.client_for_account(&acct).ok_or_else(|| anyhow!("no API token on this machine reaches that tunnel's account"))?;
    let fresh = client.connector_token(&acct, &id).map_err(|e| anyhow!("{e}"))?;
    if config::decode_token(&fresh).map(|p| p.secret).ok() == config::decode_token(&t.token).map(|p| p.secret).ok() {
        return Ok(format!("{}: already current", t.name));
    }
    config.upsert_tunnel(&t.name, &fresh)?;
    launchd::write_token_file(&fresh)?;
    if launchd::is_loaded_name(&t.name) && launchd::plist_runs_token(&t.name, &fresh) && !launchd::plist_has_inline_token(&t.name) {
        launchd::kickstart(&t.name)?;
    } else {
        launchd::restart(&t.name, &fresh)?;
    }
    Ok(format!("{}: took the current connector token and restarted it", t.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_short_stable_and_reveals_nothing() {
        let f = fingerprint("cfut_secret_value");
        assert_eq!(f, fingerprint("cfut_secret_value"));
        assert_eq!(f.len(), 12);
        assert!(!"cfut_secret_value".contains(&f));
        assert_ne!(f, fingerprint("cfut_secret_valuf"));
    }
}
