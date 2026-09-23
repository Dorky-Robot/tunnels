//! One picture of the whole fleet — every account, tunnel, connector,
//! route and where its DNS really points — shared by `tunnels status` and
//! the web UI, so the two can never tell different stories.

use crate::fleet::Fleet;
use crate::observe::{LocalObs, LocalTunnel, Snapshot};
use crate::plan::Plan;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RouteRow {
    pub host: String,
    pub service: String,
    /// primary, standby, or undeclared (in Cloudflare, not in the fleet file)
    pub role: String,
    /// this tunnel is where the hostname's traffic should go now
    pub active: bool,
    /// ok · missing · elsewhere · unknown
    pub dns: String,
    /// for `elsewhere`: the tunnel DNS actually sends it to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_target: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelRow {
    pub alias: Option<String>,
    pub id: String,
    pub cf_name: String,
    pub account: String,
    pub machine: Option<String>,
    /// up · down · missing · unknown
    pub state: String,
    pub connectors: usize,
    pub colos: Vec<String>,
    pub versions: Vec<String>,
    pub in_fleet: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub here: Option<LocalTunnel>,
    pub routes: Vec<RouteRow>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetStatus {
    pub machine: String,
    pub version: String,
    pub serial: u64,
    pub updated_at: String,
    pub updated_by: String,
    pub taken_at: String,
    pub tunnels: Vec<TunnelRow>,
    pub machines: Vec<(String, String)>,
    pub plan: Plan,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalObs>,
}

fn account_label(fleet: &Fleet, snap: &Snapshot, id: &str) -> String {
    if let Some(a) = fleet.account_alias_for_id(id) {
        return a.clone();
    }
    snap.accounts
        .iter()
        .find(|a| a.id == id)
        .map(|a| if a.name.is_empty() { id[..8.min(id.len())].to_string() } else { a.name.clone() })
        .unwrap_or_else(|| id[..8.min(id.len())].to_string())
}

fn dns_state(fleet: &Fleet, snap: &Snapshot, host: &str, tunnel_id: &str) -> (String, Option<String>) {
    if !snap.dns_known_for(host) {
        return ("unknown".into(), None);
    }
    match snap.dns_for(host).first() {
        None => ("missing".into(), None),
        Some(r) => match r.tunnel_target() {
            Some(t) if t.eq_ignore_ascii_case(tunnel_id) => ("ok".into(), None),
            Some(t) => {
                let name = fleet
                    .alias_for_id(&t)
                    .cloned()
                    .or_else(|| snap.tunnel(&t).map(|o| o.tunnel.name.clone()))
                    .unwrap_or_else(|| t[..8.min(t.len())].to_string());
                ("elsewhere".into(), Some(name))
            }
            None => ("elsewhere".into(), Some(r.content.clone())),
        },
    }
}

pub fn build(fleet: &Fleet, snap: &Snapshot, local: Option<&LocalObs>, plan: Plan, machine: &str) -> FleetStatus {
    let mut rows: Vec<TunnelRow> = Vec::new();
    let here_of = |id: &str| -> Option<LocalTunnel> {
        local?.tunnels.iter().find(|l| l.tunnel_id.as_deref().map(|i| i.eq_ignore_ascii_case(id)).unwrap_or(false)).cloned()
    };

    for (alias, t) in &fleet.tunnels {
        let obs = snap.tunnel(&t.id);
        let acct_id = fleet.accounts.get(&t.account).map(|a| a.id.clone()).unwrap_or_default();
        let state = match obs {
            Some(o) if o.up() => "up",
            Some(_) => "down",
            None if snap.account_reachable(&acct_id) => "missing",
            None => "unknown",
        };
        let mut routes: Vec<RouteRow> = Vec::new();
        for r in &fleet.routes {
            let role = if r.tunnel == *alias {
                "primary"
            } else if r.standby.as_deref() == Some(alias.as_str()) {
                "standby"
            } else {
                continue;
            };
            let active = r.active_tunnel() == alias;
            let (dns, dns_target) = if active { dns_state(fleet, snap, &r.host, &t.id) } else { ("—".into(), None) };
            routes.push(RouteRow {
                host: r.host.clone(),
                service: r.service.clone(),
                role: role.into(),
                active,
                dns,
                dns_target,
                note: r.note.clone(),
                failover: r.failover.map(|f| format!("{f:?}").to_lowercase()),
            });
        }
        if let Some(o) = obs {
            for (host, service) in o.routes() {
                if routes.iter().any(|r| r.host.eq_ignore_ascii_case(&host)) {
                    continue;
                }
                let (dns, dns_target) = dns_state(fleet, snap, &host, &t.id);
                routes.push(RouteRow {
                    host,
                    service,
                    role: "undeclared".into(),
                    active: false,
                    dns,
                    dns_target,
                    note: String::new(),
                    failover: None,
                });
            }
        }
        rows.push(TunnelRow {
            alias: Some(alias.clone()),
            id: t.id.clone(),
            cf_name: obs.map(|o| o.tunnel.name.clone()).unwrap_or_default(),
            account: t.account.clone(),
            machine: t.machine.clone(),
            state: state.into(),
            connectors: obs.map(|o| o.tunnel.connections.len()).unwrap_or(0),
            colos: obs.map(|o| dedup(o.tunnel.connections.iter().map(|c| c.colo_name.clone()))).unwrap_or_default(),
            versions: obs.map(|o| dedup(o.tunnel.connections.iter().map(|c| c.client_version.clone()))).unwrap_or_default(),
            in_fleet: true,
            here: here_of(&t.id),
            routes,
            note: t.note.clone(),
        });
    }

    for o in &snap.tunnels {
        if fleet.alias_for_id(&o.tunnel.id).is_some() {
            continue;
        }
        let routes = o
            .routes()
            .into_iter()
            .map(|(host, service)| {
                let (dns, dns_target) = dns_state(fleet, snap, &host, &o.tunnel.id);
                RouteRow { host, service, role: "undeclared".into(), active: false, dns, dns_target, note: String::new(), failover: None }
            })
            .collect();
        rows.push(TunnelRow {
            alias: None,
            id: o.tunnel.id.clone(),
            cf_name: o.tunnel.name.clone(),
            account: account_label(fleet, snap, &o.account_id),
            machine: None,
            state: if o.up() { "up".into() } else { "down".into() },
            connectors: o.tunnel.connections.len(),
            colos: dedup(o.tunnel.connections.iter().map(|c| c.colo_name.clone())),
            versions: dedup(o.tunnel.connections.iter().map(|c| c.client_version.clone())),
            in_fleet: false,
            here: here_of(&o.tunnel.id),
            routes,
            note: String::new(),
        });
    }

    FleetStatus {
        machine: machine.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        serial: fleet.serial,
        updated_at: fleet.updated_at.clone(),
        updated_by: fleet.updated_by.clone(),
        taken_at: snap.taken_at.clone(),
        tunnels: rows,
        machines: fleet.machines.iter().map(|(n, m)| (n.clone(), m.host.clone())).collect(),
        plan,
        errors: snap.errors.clone(),
        local: local.cloned(),
    }
}

fn dedup(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = it.filter(|s| !s.is_empty()).collect();
    v.sort();
    v.dedup();
    v
}

/// The same picture as text, for a terminal.
pub fn render(s: &FleetStatus) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "fleet serial {} · updated {} by {} · seen from {} at {}",
        s.serial,
        if s.updated_at.is_empty() { "never" } else { &s.updated_at },
        if s.updated_by.is_empty() { "-" } else { &s.updated_by },
        s.machine,
        s.taken_at
    );
    let mut accounts: Vec<&str> = s.tunnels.iter().map(|t| t.account.as_str()).collect();
    accounts.sort();
    accounts.dedup();
    for acct in accounts {
        let _ = writeln!(o, "\n{acct}");
        for t in s.tunnels.iter().filter(|t| t.account == acct) {
            let name = match &t.alias {
                Some(a) => a.clone(),
                None => format!("{} (NOT IN FLEET)", t.cf_name),
            };
            let conns = if t.connectors > 0 { format!("{}× {}", t.connectors, t.colos.join(",")) } else { String::new() };
            let _ = writeln!(
                o,
                "  {:<3} {:<24} {:<8} on {:<14} {}  {}",
                match t.state.as_str() {
                    "up" => "●",
                    "down" => "○",
                    _ => "?",
                },
                name,
                t.state,
                t.machine.as_deref().unwrap_or("-"),
                &t.id[..8],
                conns
            );
            for r in &t.routes {
                let dns = match (r.dns.as_str(), &r.dns_target) {
                    ("ok", _) => "dns ok".to_string(),
                    ("elsewhere", Some(t)) => format!("DNS → {t}"),
                    ("missing", _) => "NO DNS".to_string(),
                    ("—", _) => String::new(),
                    (d, _) => format!("dns {d}"),
                };
                let role = match r.role.as_str() {
                    "primary" => "",
                    "standby" if r.active => " [standby, ACTIVE]",
                    "standby" => " [standby]",
                    _ => " [undeclared]",
                };
                let _ = writeln!(o, "        {:<38} {:<28} {}{}", r.host, r.service, dns, role);
            }
        }
    }
    if !s.errors.is_empty() {
        let _ = writeln!(o, "\ncould not see everything:");
        for e in &s.errors {
            let _ = writeln!(o, "  ! {e}");
        }
    }
    o.push_str(&render_plan(&s.plan));
    o
}

pub fn render_plan(p: &Plan) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    if !p.findings.is_empty() {
        let _ = writeln!(o, "\nfindings:");
        for f in &p.findings {
            let lvl = match f.level {
                crate::plan::Level::Error => "✗",
                crate::plan::Level::Warn => "⚠",
                crate::plan::Level::Info => "·",
            };
            let _ = writeln!(o, "  {lvl} {}: {}", f.subject, f.message);
            if let Some(fix) = &f.fix {
                let _ = writeln!(o, "      → {fix}");
            }
        }
    }
    if p.actions.is_empty() {
        let _ = writeln!(o, "\nin line with the fleet file — nothing to do");
    } else {
        let _ = writeln!(o, "\nto bring things in line with the fleet file:");
        for a in &p.actions {
            let mut tags = vec![a.scope.tag().to_string()];
            if let Some(m) = &a.owner {
                tags.push(format!("owner {m}"));
            }
            if a.prune {
                tags.push("needs --prune".into());
            }
            if a.guarded && !a.destroy {
                tags.push("needs --yes".into());
            }
            if a.destroy {
                tags.push("needs --allow-destroy".into());
            }
            let _ = writeln!(o, "  {} {}   {}", a.symbol(), a.summary, tags.join(" · "));
        }
    }
    o
}
