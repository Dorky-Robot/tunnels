//! `tunnels cf`: the whole Cloudflare API, with guardrails.
//!
//! `tunnels` models tunnels, routes and DNS. Everything else Cloudflare does
//! (Access, SSL, WAF, zone settings) an agent reaches through here:
//!
//! - **names instead of ids**: `{account:felixflor}`, `{zone:everyday.vet}`,
//!   `{tunnel:vet-prod}`, `{record:admin.everyday.vet}`;
//! - **no secrets in view**: the token is picked here and never printed;
//!   secrets in responses are hidden;
//! - **reads are free, writes are previews** until `--yes`;
//! - **every write is logged with its before and after**, on this machine,
//!   with the request that would undo it;
//! - **what `tunnels` owns is refused**: tunnel config and tunnel CNAMEs go
//!   through the fleet file, or the agents would undo the change.
//!
//! Plan: docs/agent-operations.md.

use crate::cf::Client;
use crate::config::Config;
use crate::fleet::Fleet;
use crate::observe::{self, Snapshot, Want};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// "This machine cannot see that" — no token here reaches the account or zone.
/// The one kind of failure worth asking a peer about; any other error (an
/// ambiguous name, a typo) is the caller's to fix and is reported as is.
#[derive(Debug)]
pub struct NotHere(pub String);

impl std::fmt::Display for NotHere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotHere {}

fn not_here(msg: String) -> anyhow::Error {
    anyhow::Error::new(NotHere(msg))
}

/// Accounts, zones and which token reaches each, without listing tunnels,
/// ingress or DNS — the few calls needed to resolve names and pick a token.
pub fn directory(config: &Config) -> Snapshot {
    observe::observe(
        config,
        &Want { ingress_for: Some(BTreeSet::new()), dns_zones: Some(BTreeSet::new()), accounts: Some(BTreeSet::new()) },
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub path: String,
    pub account_id: Option<String>,
    /// what each placeholder became, for showing
    pub notes: Vec<String>,
}

fn is_hex_id(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Replace `{kind:name}` placeholders with ids. An ambiguous or unknown name
/// is an error that lists what was found, never a guess.
pub fn resolve(path: &str, fleet: &Fleet, dir: &Snapshot) -> Result<Resolved> {
    if !path.starts_with('/') {
        bail!("the path starts with /, as in /zones/{{zone:example.com}}/settings/ssl");
    }
    let mut out = String::new();
    let mut notes = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').ok_or_else(|| anyhow!("unclosed {{ in {path}"))? + start;
        let inner = &rest[start + 1..end];
        let (kind, name) = inner.split_once(':').ok_or_else(|| anyhow!("{{{inner}}}: write it as {{kind:name}}"))?;
        let id = match kind {
            "account" => resolve_account(name, fleet, dir)?,
            "zone" => resolve_zone(name, dir)?,
            "tunnel" => resolve_tunnel(name, fleet, dir)?,
            "record" => resolve_record(name, dir)?,
            k => bail!("unknown placeholder kind `{k}` — use account, zone, tunnel or record"),
        };
        notes.push(format!("{{{inner}}} = {id}"));
        out.push_str(&id);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let account_id = account_of_path(&out, dir);
    Ok(Resolved { path: out, account_id, notes })
}

fn resolve_account(name: &str, fleet: &Fleet, dir: &Snapshot) -> Result<String> {
    if let Some(a) = fleet.accounts.get(name) {
        return Ok(a.id.clone());
    }
    if is_hex_id(name, 32) {
        return Ok(name.to_string());
    }
    let hits: Vec<_> = dir.accounts.iter().filter(|a| a.name.eq_ignore_ascii_case(name)).collect();
    match hits.as_slice() {
        [a] => Ok(a.id.clone()),
        [] => bail!(
            "no account `{name}` — the fleet has {}",
            fleet.accounts.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        _ => bail!("`{name}` names {} accounts; use a fleet alias or an id", hits.len()),
    }
}

fn resolve_zone(name: &str, dir: &Snapshot) -> Result<String> {
    if is_hex_id(name, 32) {
        return Ok(name.to_string());
    }
    dir.zones
        .iter()
        .find(|z| z.name.eq_ignore_ascii_case(name))
        .map(|z| z.id.clone())
        .ok_or_else(|| {
            not_here(format!(
                "no zone `{name}` reachable from this machine — it can see {}",
                dir.zones.iter().map(|z| z.name.clone()).collect::<Vec<_>>().join(", ")
            ))
        })
}

fn resolve_tunnel(name: &str, fleet: &Fleet, dir: &Snapshot) -> Result<String> {
    if let Some((_, t)) = fleet.find_tunnel(name) {
        return Ok(t.id.clone());
    }
    let mut hits: Vec<(String, String)> = Vec::new();
    for (acct, tok) in &dir.token_for_account {
        if let Ok(ts) = Client::new(tok).tunnels(acct) {
            for t in ts {
                if t.name.eq_ignore_ascii_case(name) || t.id.eq_ignore_ascii_case(name) {
                    hits.push((t.id, format!("{} in account {}", t.name, &acct[..8])));
                }
            }
        }
    }
    match hits.as_slice() {
        [(id, _)] => Ok(id.clone()),
        [] => Err(not_here(format!("no tunnel `{name}` in the fleet or in any account this machine reaches"))),
        _ => bail!(
            "`{name}` names {} tunnels — use a fleet alias or an id:\n  {}",
            hits.len(),
            hits.iter().map(|(id, d)| format!("{id}  {d}")).collect::<Vec<_>>().join("\n  ")
        ),
    }
}

fn resolve_record(host: &str, dir: &Snapshot) -> Result<String> {
    let zone = dir.zone_for_host(host).ok_or_else(|| not_here(format!("no zone reachable from here contains {host}")))?;
    let client = dir.client_for_zone(&zone.id).ok_or_else(|| not_here(format!("no token here reaches {}", zone.name)))?;
    let recs = client.dns_records_named(&zone.id, host).map_err(|e| anyhow!("looking up {host}: {e}"))?;
    match recs.as_slice() {
        [r] => Ok(r.id.clone()),
        [] => bail!("{host} has no DNS record"),
        _ => bail!(
            "{host} has {} records — use the record id:\n  {}",
            recs.len(),
            recs.iter().map(|r| format!("{}  {} {}", r.id, r.rtype, r.content)).collect::<Vec<_>>().join("\n  ")
        ),
    }
}

/// The account a resolved path acts in, when the path says.
fn account_of_path(path: &str, dir: &Snapshot) -> Option<String> {
    let segs: Vec<&str> = path.split('?').next().unwrap_or("").trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        ["accounts", id, ..] if is_hex_id(id, 32) => Some(id.to_string()),
        ["zones", id, ..] if is_hex_id(id, 32) => dir.zones.iter().find(|z| z.id == *id).map(|z| z.account_id.clone()),
        _ => None,
    }
}

/// The token to use: the account's, from the path or `--account`; or the
/// only token here, when there is just one.
pub fn client_for(r: &Resolved, account_flag: Option<&str>, fleet: &Fleet, dir: &Snapshot, config: &Config) -> Result<Client> {
    let acct = match (&r.account_id, account_flag) {
        (Some(a), _) => Some(a.clone()),
        (None, Some(f)) => Some(resolve_account(f, fleet, dir)?),
        (None, None) => None,
    };
    match acct {
        Some(a) => dir.client_for_account(&a).ok_or_else(|| {
            not_here(format!(
                "no API token on this machine reaches account {a}{} — `tunnels token add` one with the permission this call needs",
                fleet.account_alias_for_id(&a).map(|x| format!(" ({x})")).unwrap_or_default()
            ))
        }),
        None => {
            let toks = config.all_cf_api_tokens();
            match toks.as_slice() {
                [t] => Ok(Client::new(t)),
                [] => Err(not_here("no API token on this machine — `tunnels token add`".into())),
                _ => bail!("this path names no account and there are {} tokens here — say which with --account <alias>", toks.len()),
            }
        }
    }
}

// ------------------------------------------------------------------ guards

/// Why a write is refused outright, if it is.
pub fn refusal(method: &str, path: &str, body: Option<&Value>, dir: &Snapshot) -> Option<String> {
    if method == "GET" {
        return None;
    }
    let p = path.split('?').next().unwrap_or("");
    if p.contains("/cfd_tunnel") {
        return Some(
            "tunnels, their ingress and their tokens are managed by `tunnels` itself — \
             use `tunnels route add|rm|mv` or `tunnels tunnel create|destroy|rotate`, \
             so the fleet file stays the truth and no agent undoes your change"
                .into(),
        );
    }
    if p.contains("/dns_records") {
        let body_to_tunnel = body
            .and_then(|b| b.get("content"))
            .and_then(|c| c.as_str())
            .map(|c| c.to_ascii_lowercase().ends_with(".cfargotunnel.com"))
            .unwrap_or(false);
        let existing_to_tunnel = existing_record(p, dir)
            .map(|r| r.get("content").and_then(|c| c.as_str()).unwrap_or("").to_ascii_lowercase().ends_with(".cfargotunnel.com"))
            .unwrap_or(false);
        if body_to_tunnel || existing_to_tunnel {
            return Some("DNS records that point at a tunnel belong to its route — use `tunnels route add|rm|mv`".into());
        }
    }
    None
}

fn existing_record(path: &str, dir: &Snapshot) -> Option<Value> {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if let ["zones", z, "dns_records", _id] = segs.as_slice() {
        let c = dir.client_for_zone(z)?;
        let (st, v) = c.raw("GET", path, None).ok()?;
        if st < 300 {
            return v.get("result").cloned();
        }
    }
    None
}

pub fn is_token_path(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or("");
    p.starts_with("/user/tokens") || (p.starts_with("/accounts/") && p.contains("/tokens"))
}

const HIDDEN: &str = "[hidden by tunnels]";

/// Hide secrets in anything that is shown or logged. A connector token, a
/// client secret, an API token's value — none of them belong in a transcript.
pub fn redact(v: &mut Value, token_path: bool, connector_token_path: bool) -> bool {
    let mut hid = false;
    if connector_token_path {
        if let Some(r) = v.get_mut("result") {
            if r.is_string() {
                *r = Value::String(format!("{HIDDEN}: connector tokens are fetched by `tunnels` when it runs a tunnel"));
                hid = true;
            }
        }
    }
    fn walk(v: &mut Value, token_path: bool, hid: &mut bool) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    let k = k.to_ascii_lowercase();
                    let secret = matches!(k.as_str(), "client_secret" | "tunnel_secret" | "secret" | "private_key" | "password")
                        || (token_path && k == "value");
                    if secret && !x.is_null() {
                        *x = Value::String(HIDDEN.into());
                        *hid = true;
                    } else {
                        walk(x, token_path, hid);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, token_path, hid)),
            _ => {}
        }
    }
    walk(v, token_path, &mut hid);
    hid
}

fn is_connector_token_path(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or("");
    p.contains("/cfd_tunnel/") && p.ends_with("/token")
}

// ------------------------------------------------------------------ calls

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Undo {
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub best_effort: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub at: String,
    pub machine: String,
    pub fleet_serial: u64,
    pub method: String,
    pub path: String,
    pub resolved_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    pub status: u16,
    pub ok: bool,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub undo: Option<Undo>,
    /// this record undid another
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undoes: Option<String>,
    /// the machine that asked for this change, when it was made on its behalf
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    /// the machine that made the call, when this one had no token for it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub method: String,
    pub path: String,
    pub resolved_path: String,
    pub notes: Vec<String>,
    pub status: Option<u16>,
    pub ok: bool,
    pub sent: bool,
    pub response: Option<Value>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub undo: Option<Undo>,
    pub log_id: Option<String>,
    pub refused: Option<String>,
    pub hidden_secrets: bool,
}

fn ok_result(status: u16, v: &Value) -> Option<Value> {
    if status < 300 && v.get("success").and_then(|s| s.as_bool()).unwrap_or(true) { v.get("result").cloned() } else { None }
}

fn contains_hidden(v: &Value) -> bool {
    match v {
        Value::String(s) => s.starts_with(HIDDEN),
        Value::Object(m) => m.values().any(contains_hidden),
        Value::Array(a) => a.iter().any(contains_hidden),
        _ => false,
    }
}

/// The request that would put things back, from what was there before.
pub fn plan_undo(method: &str, path: &str, body: Option<&Value>, before: Option<&Value>, created_id: Option<&str>) -> Option<Undo> {
    let p = path.split('?').next().unwrap_or(path).to_string();
    match method {
        "PATCH" => {
            let before = before?;
            let fields = body?.as_object()?;
            let mut back = serde_json::Map::new();
            for k in fields.keys() {
                back.insert(k.clone(), before.get(k).cloned().unwrap_or(Value::Null));
            }
            let b = Value::Object(back);
            (!contains_hidden(&b)).then(|| Undo { method: "PATCH".into(), path: p, body: Some(b), best_effort: false })
        }
        "PUT" => {
            let before = before?.clone();
            (!contains_hidden(&before)).then(|| Undo { method: "PUT".into(), path: p, body: Some(before), best_effort: false })
        }
        "POST" => created_id.map(|id| Undo {
            method: "DELETE".into(),
            path: format!("{}/{id}", p.trim_end_matches('/')),
            body: None,
            best_effort: false,
        }),
        "DELETE" => {
            let mut b = before?.clone();
            if contains_hidden(&b) {
                return None;
            }
            if let Some(m) = b.as_object_mut() {
                for k in ["id", "created_on", "modified_on", "created_at", "updated_at"] {
                    m.remove(k);
                }
            }
            let parent = p.rsplit_once('/').map(|(a, _)| a.to_string())?;
            Some(Undo { method: "POST".into(), path: parent, body: Some(b), best_effort: true })
        }
        _ => None,
    }
}

#[derive(Serialize, Deserialize)]
pub struct Call<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub body: Option<Value>,
    pub yes: bool,
    pub account: Option<&'a str>,
    pub i_mean_tokens: bool,
    pub not_undoable: bool,
    pub undoes: Option<String>,
    /// may this call go to a peer that holds the token, if this machine has none
    #[serde(default)]
    pub forward: bool,
    /// set on the peer: the machine this call is made for
    #[serde(default)]
    pub requested_by: Option<String>,
}

/// Can this machine make this call itself? `Ok(true)` yes; `Ok(false)` only
/// because no token here reaches it (a peer might); `Err` for anything a peer
/// could not fix either.
pub fn local_check(path: &str, account: Option<&str>, fleet: &Fleet, dir: &Snapshot, config: &Config) -> Result<bool> {
    let r = match resolve(path, fleet, dir) {
        Ok(r) => r,
        Err(e) if e.downcast_ref::<NotHere>().is_some() => return Ok(false),
        Err(e) => return Err(e),
    };
    match client_for(&r, account, fleet, dir, config) {
        Ok(_) => Ok(true),
        Err(e) if e.downcast_ref::<NotHere>().is_some() => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn can_handle(path: &str, account: Option<&str>, fleet: &Fleet, dir: &Snapshot, config: &Config) -> bool {
    matches!(local_check(path, account, fleet, dir, config), Ok(true))
}

/// Do one call: resolve, guard, record the before state, send (if allowed),
/// record the after state, log it.
pub fn call(c: Call, config: &Config, fleet: &Fleet, me: &str) -> Result<Outcome> {
    let method = c.method.to_ascii_uppercase();
    let dir = directory(config);
    // a machine without the token asks a peer that has it; the token never moves
    if c.forward && !local_check(c.path, c.account, fleet, &dir, config)? {
        return forward(&c, fleet, me);
    }
    let r = resolve(c.path, fleet, &dir)?;
    let token_path = is_token_path(&r.path);
    let conn_path = is_connector_token_path(&r.path);
    let mut out = Outcome {
        via: None,
        method: method.clone(),
        path: c.path.to_string(),
        resolved_path: r.path.clone(),
        notes: r.notes.clone(),
        status: None,
        ok: false,
        sent: false,
        response: None,
        before: None,
        after: None,
        undo: None,
        log_id: None,
        refused: None,
        hidden_secrets: false,
    };
    if let Some(why) = refusal(&method, &r.path, c.body.as_ref(), &dir) {
        out.refused = Some(why);
        return Ok(out);
    }
    if token_path && method != "GET" && !c.i_mean_tokens {
        out.refused = Some("this changes API tokens — add --i-mean-tokens as well as --yes if that is really what you want".into());
        return Ok(out);
    }
    let client = client_for(&r, c.account, fleet, &dir, config)?;

    if method == "GET" {
        let (st, mut v) = client.raw("GET", &r.path, None).map_err(|e| anyhow!("{e}"))?;
        out.hidden_secrets = redact(&mut v, token_path, conn_path);
        out.status = Some(st);
        out.ok = ok_result(st, &v).is_some();
        out.response = Some(v);
        out.sent = true;
        return Ok(out);
    }

    // the before state
    let before = if method == "POST" {
        None
    } else {
        client.raw("GET", &r.path, None).ok().and_then(|(st, mut v)| {
            redact(&mut v, token_path, conn_path);
            ok_result(st, &v)
        })
    };
    out.before = before.clone();
    // what undo would be, as far as can be known before sending
    out.undo = if method == "POST" {
        Some(Undo { method: "DELETE".into(), path: format!("{}/<the id it creates>", r.path), body: None, best_effort: false })
    } else {
        plan_undo(&method, &r.path, c.body.as_ref(), before.as_ref(), None)
    };
    if out.undo.is_none() && !c.not_undoable {
        out.refused = Some(
            "there is no way to undo this (no readable before state) — add --not-undoable as well as --yes if you accept that".into(),
        );
        return Ok(out);
    }
    if !c.yes {
        return Ok(out);
    }

    let (st, mut v) = client.raw(&method, &r.path, c.body.clone()).map_err(|e| anyhow!("{e}"))?;
    out.hidden_secrets = redact(&mut v, token_path, conn_path);
    out.sent = true;
    out.status = Some(st);
    let result = ok_result(st, &v);
    out.ok = result.is_some();
    let created = if method == "POST" { result.as_ref().and_then(|x| x.get("id")).and_then(|i| i.as_str()).map(String::from) } else { None };
    out.undo = plan_undo(&method, &r.path, c.body.as_ref(), before.as_ref(), created.as_deref());
    let after_path = match (&method[..], &created) {
        ("POST", Some(id)) => Some(format!("{}/{id}", r.path.split('?').next().unwrap_or("").trim_end_matches('/'))),
        ("POST", None) => None,
        _ => Some(r.path.clone()),
    };
    out.after = after_path.and_then(|p| {
        client.raw("GET", &p, None).ok().and_then(|(st, mut v)| {
            redact(&mut v, token_path, conn_path);
            ok_result(st, &v)
        })
    });
    out.response = Some(v);

    let rec = Record {
        id: new_id(),
        at: crate::util::now_rfc3339(),
        machine: me.to_string(),
        fleet_serial: fleet.serial,
        method: method.clone(),
        path: c.path.to_string(),
        resolved_path: r.path.clone(),
        account_id: r.account_id.clone(),
        body: c.body.clone().map(|mut b| {
            redact(&mut b, token_path, false);
            b
        }),
        status: st,
        ok: out.ok,
        before: out.before.clone(),
        after: out.after.clone(),
        undo: out.undo.clone(),
        undoes: c.undoes.clone(),
        requested_by: c.requested_by.clone(),
    };
    save(&rec)?;
    out.log_id = Some(rec.id);
    Ok(out)
}

/// What a 403 most likely means, in words: which permission the token for
/// this account is missing. The tokens `tunnels` needs for routes (Tunnel,
/// DNS) do not cover zone settings or Access, and a bare 403 says none of that.
pub fn permission_hint(path: &str) -> String {
    let p = path.split('?').next().unwrap_or("");
    let perm = if p.contains("/settings") {
        "Zone › Zone Settings (Read, or Edit to change)"
    } else if p.contains("/access/identity_providers") {
        "Account › Access: Identity Providers"
    } else if p.contains("/access/organizations") {
        "Account › Access: Organizations, Identity Providers, and Groups"
    } else if p.contains("/access/") {
        "Account › Access: Apps and Policies"
    } else if p.contains("/dns_records") {
        "Zone › DNS"
    } else if p.contains("/firewall") || p.contains("/rulesets") {
        "Zone › Firewall Services / WAF"
    } else if p.contains("/tokens") {
        "User › API Tokens"
    } else {
        "the permission for this endpoint"
    };
    format!(
        "the API token this machine uses for that account lacks {perm}. Add it at dash.cloudflare.com/profile/api-tokens (edit the token, or add a second one with `tunnels token add`)"
    )
}

/// Send the call to the first peer that can make it. Each peer checks that
/// the caller is who it says (by tailnet address) and on the fleet's
/// `policy.remote_from` list, makes the call with its own token, and logs it.
fn forward(c: &Call, fleet: &Fleet, me: &str) -> Result<Outcome> {
    let port = fleet.policy.web_port;
    let mut body = serde_json::to_value(c)?;
    body["forward"] = Value::Bool(false);
    body["requested_by"] = Value::String(me.to_string());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(40)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut refusals = Vec::new();
    for (name, m) in fleet.machines.iter().filter(|(n, _)| n.as_str() != me) {
        let Ok(mut resp) = agent
            .post(&format!("http://{}:{port}/api/cf-forward", m.host))
            .header("X-Tunnels", "1")
            .send_json(&body)
        else {
            refusals.push(format!("{name}: not answering"));
            continue;
        };
        let status = resp.status().as_u16();
        let v: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
        match status {
            200 => {
                let mut out: Outcome = serde_json::from_value(v)?;
                out.via = Some(name.clone());
                return Ok(out);
            }
            409 => refusals.push(format!("{name}: no token for it")),
            403 => refusals.push(format!("{name}: {}", v["error"].as_str().unwrap_or("refused"))),
            _ => bail!("{name} could not make the call: {}", v["error"].as_str().unwrap_or("unknown error")),
        }
    }
    bail!(
        "no token on this machine reaches that account, and no peer could make the call for it:\n  {}",
        refusals.join("\n  ")
    )
}

// ------------------------------------------------------------------ the log

pub fn log_dir() -> PathBuf {
    Config::dir().join("cf-log")
}

fn new_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 4];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn save(r: &Record) -> Result<()> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!("{}-{}.json", r.at.replace(':', ""), r.id);
    crate::util::write_atomic(&dir.join(name), serde_json::to_string_pretty(r)?.as_bytes(), 0o600)
}

/// This machine's records, newest first.
pub fn load_log(limit: usize) -> Vec<Record> {
    let Ok(rd) = std::fs::read_dir(log_dir()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "json").unwrap_or(false)).collect();
    files.sort();
    files
        .iter()
        .rev()
        .take(limit)
        .filter_map(|p| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()))
        .collect()
}

pub fn find(id: &str) -> Option<Record> {
    load_log(10_000).into_iter().find(|r| r.id == id)
}

/// The top-level fields that differ, as `field: before → after`.
pub fn diff(before: Option<&Value>, after: Option<&Value>) -> Vec<String> {
    let (Some(Value::Object(b)), Some(Value::Object(a))) = (before, after) else {
        return match (before, after) {
            (None, Some(_)) => vec!["(created)".into()],
            (Some(_), None) => vec!["(gone)".into()],
            (Some(b), Some(a)) if b != a => vec![format!("{b} → {a}")],
            _ => vec![],
        };
    };
    let keys: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
    keys.into_iter()
        .filter(|k| !matches!(k.as_str(), "modified_on" | "updated_at"))
        .filter_map(|k| {
            let (x, y) = (b.get(k), a.get(k));
            (x != y).then(|| {
                format!(
                    "{k}: {} → {}",
                    x.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                    y.map(|v| v.to_string()).unwrap_or_else(|| "—".into())
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_patch_is_undone_with_just_the_fields_it_changed() {
        let before = json!({ "id": "ssl", "value": "full", "editable": true });
        let u = plan_undo("PATCH", "/zones/z/settings/ssl", Some(&json!({ "value": "strict" })), Some(&before), None).unwrap();
        assert_eq!(u.method, "PATCH");
        assert_eq!(u.body, Some(json!({ "value": "full" })));
    }

    #[test]
    fn a_post_is_undone_by_deleting_what_it_made() {
        let u = plan_undo("POST", "/accounts/a/access/apps", Some(&json!({})), None, Some("app1")).unwrap();
        assert_eq!((u.method.as_str(), u.path.as_str()), ("DELETE", "/accounts/a/access/apps/app1"));
    }

    #[test]
    fn a_delete_is_undone_by_posting_it_back_best_effort() {
        let before = json!({ "id": "x", "name": "n", "created_on": "t" });
        let u = plan_undo("DELETE", "/accounts/a/access/apps/x", None, Some(&before), None).unwrap();
        assert_eq!(u.path, "/accounts/a/access/apps");
        assert_eq!(u.body, Some(json!({ "name": "n" })));
        assert!(u.best_effort);
    }

    #[test]
    fn nothing_readable_before_means_no_undo() {
        assert!(plan_undo("PATCH", "/x", Some(&json!({ "a": 1 })), None, None).is_none());
        // and a before state with secrets hidden cannot be put back
        let hidden = json!({ "client_secret": HIDDEN });
        assert!(plan_undo("PUT", "/x", Some(&json!({})), Some(&hidden), None).is_none());
    }

    #[test]
    fn secrets_are_hidden() {
        let mut v = json!({ "result": { "config": { "client_id": "c", "client_secret": "s3cret" }, "value": "keep" } });
        assert!(redact(&mut v, false, false));
        assert_eq!(v["result"]["config"]["client_secret"], HIDDEN);
        assert_eq!(v["result"]["value"], "keep", "value is only secret on token paths");
        let mut t = json!({ "result": { "id": "t", "value": "tok-value" } });
        assert!(redact(&mut t, true, false));
        assert_eq!(t["result"]["value"], HIDDEN);
        let mut conn = json!({ "success": true, "result": "eyJhIjoi..." });
        assert!(redact(&mut conn, false, true));
        assert!(conn["result"].as_str().unwrap().starts_with(HIDDEN));
    }

    #[test]
    fn what_tunnels_owns_is_refused() {
        let dir = Snapshot::default();
        assert!(refusal("PUT", "/accounts/a/cfd_tunnel/t/configurations", None, &dir).is_some());
        assert!(refusal("DELETE", "/accounts/a/cfd_tunnel/t", None, &dir).is_some());
        assert!(refusal("GET", "/accounts/a/cfd_tunnel/t/configurations", None, &dir).is_none(), "reading is fine");
        let cname = json!({ "type": "CNAME", "name": "x.example.com", "content": "abc.cfargotunnel.com" });
        assert!(refusal("POST", "/zones/z/dns_records", Some(&cname), &dir).is_some());
        let a = json!({ "type": "A", "name": "x.example.com", "content": "1.2.3.4" });
        assert!(refusal("POST", "/zones/z/dns_records", Some(&a), &dir).is_none());
        assert!(refusal("PATCH", "/zones/z/settings/ssl", Some(&json!({"value":"strict"})), &dir).is_none());
    }

    #[test]
    fn diffs_name_the_fields_that_changed() {
        let d = diff(Some(&json!({ "value": "full", "modified_on": "1" })), Some(&json!({ "value": "strict", "modified_on": "2" })));
        assert_eq!(d, vec![r#"value: "full" → "strict""#]);
    }
}
