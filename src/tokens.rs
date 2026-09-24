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

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ApiTokenView {
    /// a fingerprint, for naming this token in a removal
    pub id: String,
    pub hint: String,
    pub covers: String,
    pub reach: Vec<Reach>,
    /// does Cloudflare accept it right now
    pub valid: Option<bool>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ConnectorView {
    /// the name in this machine's config and LaunchAgent label
    pub name: String,
    pub tunnel_id: Option<String>,
    pub alias: Option<String>,
    pub account: Option<String>,
    pub account_id: Option<String>,
    /// loaded, not loaded, no plist
    pub state: String,
    /// kept in a 0600 token file (true) or inline in the plist (false)
    pub token_file: bool,
    /// matches the connector token Cloudflare hands out now; None if no API
    /// token here can ask
    pub current: Option<bool>,
    /// the fleet runs this tunnel on some other machine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs_elsewhere: Option<String>,
}

/// One Cloudflare account on one machine: the API tokens that reach it and
/// the tunnels in it. Tokens and tunnels belong to accounts, so this is how
/// they are read — "can this machine manage its tunnels in that account?"
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AccountGroup {
    pub account_id: String,
    /// the fleet's alias for it, if the fleet knows it
    pub alias: Option<String>,
    pub name: String,
    pub zones: Vec<String>,
    pub api_tokens: Vec<ApiTokenView>,
    pub connectors: Vec<ConnectorView>,
    /// an API token here that Cloudflare accepts reaches this account: its
    /// tunnels can be checked, re-fetched and rotated from this machine
    pub manageable: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MachineTokens {
    pub machine: String,
    pub api_tokens: Vec<ApiTokenView>,
    pub connectors: Vec<ConnectorView>,
    /// the same tokens, by Cloudflare account
    pub accounts: Vec<AccountGroup>,
    /// API tokens that reach no account (rejected, or never labelled)
    pub unplaced: Vec<ApiTokenView>,
}

/// Group a machine's tokens by the Cloudflare account they belong to. An API
/// token that reaches two accounts is listed under both.
pub fn group(api_tokens: &[ApiTokenView], connectors: &[ConnectorView], fleet: &Fleet) -> (Vec<AccountGroup>, Vec<ApiTokenView>) {
    let mut groups: Vec<AccountGroup> = Vec::new();
    let ensure = |id: &str, name: &str, groups: &mut Vec<AccountGroup>| -> usize {
        if let Some(i) = groups.iter().position(|g| g.account_id == id) {
            if groups[i].name.is_empty() && !name.is_empty() {
                groups[i].name = name.to_string();
            }
            return i;
        }
        let alias = fleet.account_alias_for_id(id).cloned();
        let fa = alias.as_ref().and_then(|a| fleet.accounts.get(a));
        groups.push(AccountGroup {
            account_id: id.to_string(),
            name: if name.is_empty() { fa.map(|a| a.name.clone()).unwrap_or_default() } else { name.to_string() },
            zones: fa.map(|a| a.zones.clone()).unwrap_or_default(),
            alias,
            api_tokens: Vec::new(),
            connectors: Vec::new(),
            manageable: false,
        });
        groups.len() - 1
    };
    let mut unplaced = Vec::new();
    for t in api_tokens {
        if t.reach.is_empty() {
            unplaced.push(t.clone());
            continue;
        }
        for r in &t.reach {
            let i = ensure(&r.account_id, &r.account_name, &mut groups);
            if groups[i].zones.is_empty() {
                groups[i].zones = r.zones.clone();
            }
            groups[i].api_tokens.push(t.clone());
            if t.valid == Some(true) {
                groups[i].manageable = true;
            }
        }
    }
    for c in connectors {
        let Some(id) = &c.account_id else { continue };
        let i = ensure(id, "", &mut groups);
        groups[i].connectors.push(c.clone());
    }
    groups.sort_by(|a, b| a.alias.clone().unwrap_or(a.name.clone()).cmp(&b.alias.clone().unwrap_or(b.name.clone())));
    (groups, unplaced)
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
    let api_tokens: Vec<ApiTokenView> = config
        .api_tokens()
        .iter()
        .map(|t| {
            let valid = Client::new(&t.token).verify().is_ok();
            // a token kept before reach was recorded (bare-string configs) is
            // still valid; look up what it reaches rather than calling it
            // placeless and its account unmanaged
            let reach = if t.reach.is_empty() && valid { reach_of(&t.token).map(|(_, r)| r).unwrap_or_default() } else { t.reach.clone() };
            ApiTokenView { id: fingerprint(&t.token), hint: t.hint(), covers: t.covers.clone(), reach, valid: Some(valid) }
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
                account_id: acct.clone(),
                state: state.into(),
                token_file: !launchd::plist_has_inline_token(&t.name),
                current,
                runs_elsewhere: decl.and_then(|(_, d)| d.machine).filter(|m| m != me),
            }
        })
        .collect::<Vec<_>>();
    let api_tokens: Vec<ApiTokenView> = api_tokens;
    let (accounts, unplaced) = group(&api_tokens, &connectors, fleet);
    MachineTokens { machine: me.to_string(), api_tokens, connectors, accounts, unplaced }
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

/// Replace this machine's API token for one account with a new one: add
/// it, check it reaches the account, then retire every other token here
/// that reaches only that account. A token that also reaches another
/// account is kept (retiring it would cut this machine off from the other
/// one) and named, so a person can decide. Returns what happened and the
/// hints of the tokens retired, which still need deleting in Cloudflare.
pub fn replace(config: &mut Config, token: &str, account_id: &str) -> Result<(String, Vec<String>)> {
    let token = token.trim();
    if config::decode_token(token).is_ok() {
        bail!("that is a tunnel (connector) token, not an API token — use \"paste a new tunnel token\"");
    }
    let (covers, reach) = reach_of(token)?;
    if !reach.iter().any(|r| r.account_id == account_id) {
        bail!("the new token does not reach this account — it reaches {covers}");
    }
    let new_id = fingerprint(token);
    config.add_api_token(token.to_string(), covers.clone(), reach)?;
    // the old tokens for this account, decided once; removal is by
    // fingerprint, so the list shifting under it does not matter
    // a token kept before reach was recorded has none on file: ask what it
    // reaches, or an old token for this account would be neither retired
    // nor kept, just silently left behind
    let old: Vec<(String, String, bool)> = config
        .api_tokens()
        .iter()
        .filter(|t| fingerprint(&t.token) != new_id)
        .filter_map(|t| {
            let reach = if t.reach.is_empty() { reach_of(&t.token).map(|(_, r)| r).unwrap_or_default() } else { t.reach.clone() };
            reach.iter().any(|r| r.account_id == account_id).then(|| {
                (fingerprint(&t.token), t.hint(), reach.iter().all(|r| r.account_id == account_id))
            })
        })
        .collect();
    let (mut retired, mut kept) = (Vec::new(), Vec::new());
    for (id, hint, only_this) in old {
        if only_this {
            remove(config, &id)?;
            retired.push(hint);
        } else {
            kept.push(hint);
        }
    }
    let mut msg = format!("new API token in place ({covers})");
    if !retired.is_empty() {
        msg.push_str(&format!("; retired {} old token(s) here: {}", retired.len(), retired.join(", ")));
    }
    if !kept.is_empty() {
        msg.push_str(&format!("; kept {} that also reach another account: {}", kept.len(), kept.join(", ")));
    }
    Ok((msg, retired))
}

/// Install a connector token that was refreshed in the Cloudflare
/// dashboard: find the tunnel it is for and restart it on this machine.
pub fn set_connector(config: &mut Config, fleet: &Fleet, me: &str, token: &str) -> Result<String> {
    let token = token.trim();
    let payload = config::decode_token(token).map_err(|_| anyhow!("that is not a connector (tunnel) token — they start with eyJ"))?;
    let id = payload.tunnel_id;
    let name = match config.tunnel_by_id(&id) {
        Some(t) => t.name.clone(),
        None => {
            let (alias, decl) = fleet.find_tunnel(&id).ok_or_else(|| anyhow!("tunnel {} is not in the fleet and does not run here", &id[..8]))?;
            if decl.machine.as_deref() != Some(me) {
                bail!("{alias} does not run on this machine");
            }
            alias.clone()
        }
    };
    config.upsert_tunnel(&name, token)?;
    launchd::write_token_file(token)?;
    if launchd::is_loaded_name(&name) && launchd::plist_runs_token(&name, token) && !launchd::plist_has_inline_token(&name) {
        launchd::kickstart(&name)?;
    } else {
        launchd::restart(&name, token)?;
    }
    Ok(format!("{name}: took the new connector token and restarted"))
}

/// Where one Cloudflare account's tokens are, across the whole mesh.
#[derive(Debug, Clone, Serialize)]
pub struct AccountSummary {
    pub account_id: String,
    pub alias: Option<String>,
    pub name: String,
    pub zones: Vec<String>,
    /// (machine, token) for every API token reaching this account
    pub holders: Vec<(String, ApiTokenView)>,
    /// machines that answered and hold no working token for it
    pub without: Vec<String>,
    /// (machine, tunnel) for every tunnel of this account a machine holds
    pub tunnels: Vec<(String, ConnectorView)>,
    /// the fleet's tunnels in this account, with the machine that runs each
    pub fleet_tunnels: Vec<(String, Option<String>)>,
    /// machines that did not answer
    pub unknown: Vec<String>,
}

/// A peer's view as it arrives over the wire.
pub type MachineTokensIn = MachineTokens;

/// Fold every machine's view into one summary per account.
pub fn by_account(views: &[(String, Option<MachineTokens>)], fleet: &Fleet) -> Vec<AccountSummary> {
    let mut out: Vec<AccountSummary> = Vec::new();
    let mut ids: Vec<(String, String)> = fleet.accounts.values().map(|a| (a.id.clone(), a.name.clone())).collect();
    for (_, v) in views.iter() {
        for g in v.iter().flat_map(|v| v.accounts.iter()) {
            if !ids.iter().any(|(i, _)| *i == g.account_id) {
                ids.push((g.account_id.clone(), g.name.clone()));
            }
        }
    }
    for (id, name) in ids {
        let alias = fleet.account_alias_for_id(&id).cloned();
        let fa = alias.as_ref().and_then(|a| fleet.accounts.get(a));
        let mut s = AccountSummary {
            account_id: id.clone(),
            name: if name.is_empty() { fa.map(|a| a.name.clone()).unwrap_or_default() } else { name },
            zones: fa.map(|a| a.zones.clone()).unwrap_or_default(),
            alias: alias.clone(),
            holders: Vec::new(),
            without: Vec::new(),
            tunnels: Vec::new(),
            fleet_tunnels: fleet
                .tunnels
                .iter()
                .filter(|(_, t)| alias.as_deref() == Some(t.account.as_str()) && !t.destroy)
                .map(|(a, t)| (a.clone(), t.machine.clone()))
                .collect(),
            unknown: Vec::new(),
        };
        for (m, v) in views {
            let Some(v) = v else {
                s.unknown.push(m.clone());
                continue;
            };
            match v.accounts.iter().find(|g| g.account_id == id) {
                Some(g) => {
                    for t in &g.api_tokens {
                        s.holders.push((m.clone(), t.clone()));
                    }
                    for c in &g.connectors {
                        s.tunnels.push((m.clone(), c.clone()));
                    }
                    if !g.manageable {
                        s.without.push(m.clone());
                    }
                }
                None => s.without.push(m.clone()),
            }
            if s.zones.is_empty() {
                if let Some(g) = v.accounts.iter().find(|g| g.account_id == id) {
                    s.zones = g.zones.clone();
                }
            }
        }
        out.push(s);
    }
    out.sort_by(|a, b| a.alias.clone().unwrap_or(a.name.clone()).cmp(&b.alias.clone().unwrap_or(b.name.clone())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(id: &str, valid: bool, accts: &[(&str, &str)]) -> ApiTokenView {
        ApiTokenView {
            id: id.into(),
            hint: format!("{id}…"),
            covers: String::new(),
            reach: accts.iter().map(|(i, n)| Reach { account_id: i.to_string(), account_name: n.to_string(), zones: vec![] }).collect(),
            valid: Some(valid),
        }
    }

    fn conn(name: &str, acct: &str) -> ConnectorView {
        ConnectorView {
            name: name.into(),
            tunnel_id: None,
            alias: None,
            account: None,
            account_id: Some(acct.into()),
            state: "loaded".into(),
            token_file: true,
            current: Some(true),
            runs_elsewhere: None,
        }
    }

    #[test]
    fn tokens_and_tunnels_are_grouped_by_their_account() {
        let fleet = crate::fleet::tests::sample();
        let (groups, unplaced) = group(
            &[api("a", true, &[("acct-home", "Home")]), api("b", true, &[("acct-vet", "Vet"), ("acct-home", "Home")]), api("c", false, &[])],
            &[conn("t1", "acct-home"), conn("t2", "acct-vet"), conn("t3", "acct-other")],
            &fleet,
        );
        let home = groups.iter().find(|g| g.account_id == "acct-home").unwrap();
        assert_eq!(home.alias.as_deref(), Some("home"));
        assert_eq!(home.api_tokens.len(), 2, "a token reaching two accounts is under both");
        assert_eq!(home.connectors.len(), 1);
        assert!(home.manageable);
        // a tunnel in an account no token here reaches: grouped, and not manageable
        let other = groups.iter().find(|g| g.account_id == "acct-other").unwrap();
        assert!(other.api_tokens.is_empty() && !other.manageable);
        assert_eq!(unplaced.len(), 1, "a token that reaches nothing is set apart");
    }

    #[test]
    fn a_rejected_token_does_not_make_its_account_manageable() {
        let fleet = crate::fleet::tests::sample();
        let (groups, _) = group(&[api("a", false, &[("acct-home", "Home")])], &[conn("t1", "acct-home")], &fleet);
        assert!(!groups[0].manageable);
    }

    #[test]
    fn a_fingerprint_is_short_stable_and_reveals_nothing() {
        let f = fingerprint("cfut_secret_value");
        assert_eq!(f, fingerprint("cfut_secret_value"));
        assert_eq!(f.len(), 12);
        assert!(!"cfut_secret_value".contains(&f));
        assert_ne!(f, fingerprint("cfut_secret_valuf"));
    }
}
