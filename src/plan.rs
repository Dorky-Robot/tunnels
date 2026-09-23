//! The difference between the fleet file and what is out there, as a list
//! of actions, each saying where it acts and who is allowed to carry it out.
//!
//! Pure: a fleet and a snapshot in, a plan out. No network, no launchd — so
//! every decision here is tested without either.
//!
//! Three kinds of care, kept apart because they fail differently:
//!
//! - `prune`: removes something the fleet file does not mention. Maybe it is
//!   junk, maybe somebody added it by hand yesterday. Needs `--prune` (or
//!   `policy.prune`).
//! - `guarded`: takes a hostname away from something that is serving it
//!   right now — a live takeover. `route add` used to do this silently, and
//!   staging-admin left mac2024 without a word. Needs `--yes`.
//! - `destroy`: deletes a tunnel in Cloudflare, which kills every connector
//!   token ever issued for it. Needs `--allow-destroy`.
//!
//! Agents only ever carry out actions that are none of these, and only the
//! ones they own. One owner per thing, so two agents never undo each other.

use crate::fleet::Fleet;
use crate::observe::{LocalObs, Snapshot};
use crate::scope::Scope;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Kind {
    AddIngress { tunnel: String, tunnel_id: String, account_id: String, host: String, service: String },
    UpdateIngress { tunnel: String, tunnel_id: String, account_id: String, host: String, from: String, to: String },
    RemoveIngress { tunnel: String, tunnel_id: String, account_id: String, host: String, service: String, why: String },
    CreateDns { zone_id: String, host: String, tunnel: String, tunnel_id: String },
    RepointDns { zone_id: String, record_id: String, host: String, from: String, tunnel: String, tunnel_id: String },
    DeleteDns { zone_id: String, record_id: String, host: String, content: String, why: String },
    Destroy { tunnel: String, tunnel_id: String, account_id: String },
    /// write the plist and load it, fetching the connector token first if this Mac has none
    StartLocal { tunnel: String, tunnel_id: String, account_id: String, local_name: Option<String> },
    /// the plist is there but launchd has forgotten the job (booted out)
    LoadLocal { local_name: String },
    RestartLocal { local_name: String, why: String },
    /// the Cloudflare tunnel is gone; stop running a connector for it here
    ForgetLocal { local_name: String, why: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Action {
    #[serde(flatten)]
    pub kind: Kind,
    pub scope: Scope,
    /// the machine whose agent may carry this out on its own
    pub owner: Option<String>,
    pub prune: bool,
    pub guarded: bool,
    pub destroy: bool,
    pub summary: String,
}

impl Action {
    fn new(kind: Kind, scope: Scope, owner: Option<&str>, summary: String) -> Self {
        Action { kind, scope, owner: owner.map(String::from), prune: false, guarded: false, destroy: false, summary }
    }

    pub fn symbol(&self) -> &'static str {
        match self.kind {
            Kind::AddIngress { .. } | Kind::CreateDns { .. } | Kind::StartLocal { .. } | Kind::LoadLocal { .. } => "+",
            Kind::UpdateIngress { .. } | Kind::RepointDns { .. } | Kind::RestartLocal { .. } => "~",
            Kind::RemoveIngress { .. } | Kind::DeleteDns { .. } | Kind::ForgetLocal { .. } => "-",
            Kind::Destroy { .. } => "!",
        }
    }

    /// May an agent do this without a person?
    pub fn automatic(&self, prune_allowed: bool) -> bool {
        !self.guarded && !self.destroy && (!self.prune || prune_allowed)
    }

    pub fn host(&self) -> Option<&str> {
        match &self.kind {
            Kind::AddIngress { host, .. }
            | Kind::UpdateIngress { host, .. }
            | Kind::RemoveIngress { host, .. }
            | Kind::CreateDns { host, .. }
            | Kind::RepointDns { host, .. }
            | Kind::DeleteDns { host, .. } => Some(host),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub subject: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

fn finding(level: Level, subject: &str, message: String, fix: Option<String>) -> Finding {
    Finding { level, subject: subject.to_string(), message, fix }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub findings: Vec<Finding>,
}

impl Plan {
    pub fn is_converged(&self) -> bool {
        self.actions.is_empty()
    }
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

/// Name a tunnel id for people: its fleet alias, else its Cloudflare name.
fn describe_tunnel(fleet: &Fleet, snap: &Snapshot, id: &str) -> String {
    if let Some(a) = fleet.alias_for_id(id) {
        return a.clone();
    }
    match snap.tunnel(id) {
        Some(t) => format!("{} ({}, not in the fleet)", t.tunnel.name, short(id)),
        None => format!("{} (no such tunnel)", short(id)),
    }
}

pub fn plan(fleet: &Fleet, snap: &Snapshot, local: Option<&LocalObs>) -> Plan {
    let mut p = Plan::default();

    // --- tunnels: do they exist, are they up
    for (alias, t) in &fleet.tunnels {
        let acct_id = fleet.accounts.get(&t.account).map(|a| a.id.as_str()).unwrap_or("");
        match snap.tunnel(&t.id) {
            None if snap.account_reachable(acct_id) => {
                if !t.destroy {
                    p.findings.push(finding(
                        Level::Error,
                        alias,
                        format!("tunnel {} does not exist in Cloudflare (deleted?)", short(&t.id)),
                        Some(format!("remove `{alias}` from the fleet file, or point it at a tunnel that exists")),
                    ));
                }
            }
            None => p.findings.push(finding(
                Level::Warn,
                alias,
                format!("no API token here can see account `{}`, so this tunnel cannot be checked", t.account),
                Some("tunnels token add <token for that account>".into()),
            )),
            Some(obs) => {
                if t.destroy {
                    let mut a = Action::new(
                        Kind::Destroy { tunnel: alias.clone(), tunnel_id: t.id.clone(), account_id: obs.account_id.clone() },
                        Scope::Cloudflare,
                        None,
                        format!(
                            "DESTROY tunnel {alias} ({}) in Cloudflare — every connector token for it stops working{}",
                            short(&t.id),
                            if obs.up() { format!("; it has {} live connector(s)", obs.tunnel.connections.len()) } else { String::new() }
                        ),
                    );
                    a.destroy = true;
                    a.guarded = true;
                    p.actions.push(a);
                } else if !obs.up() {
                    let since = obs.tunnel.conns_inactive_at.clone().unwrap_or_default();
                    p.findings.push(finding(
                        Level::Warn,
                        alias,
                        format!(
                            "down — no connectors{}{}",
                            if since.is_empty() { String::new() } else { format!(" since {since}") },
                            t.machine.as_ref().map(|m| format!("; it should be running on {m}")).unwrap_or_default()
                        ),
                        t.machine.as_ref().map(|m| format!("check `{m}`: tunnels status --local")),
                    ));
                }
            }
        }
    }

    // --- ingress: each fleet tunnel should carry exactly its routes (as
    // primary or standby)
    for (alias, t) in &fleet.tunnels {
        if t.destroy {
            continue;
        }
        let Some(obs) = snap.tunnel(&t.id) else { continue };
        let Some(current) = &obs.ingress else { continue };
        let owner = t.machine.as_deref();
        let want: Vec<(&str, &str)> = fleet
            .routes
            .iter()
            .filter(|r| r.tunnel == *alias || r.standby.as_deref() == Some(alias.as_str()))
            .map(|r| (r.host.as_str(), r.service.as_str()))
            .collect();
        for (host, service) in &want {
            match current.iter().find(|r| r.hostname.as_deref().map(|h| h.eq_ignore_ascii_case(host)).unwrap_or(false)) {
                None => p.actions.push(Action::new(
                    Kind::AddIngress {
                        tunnel: alias.clone(),
                        tunnel_id: t.id.clone(),
                        account_id: obs.account_id.clone(),
                        host: host.to_string(),
                        service: service.to_string(),
                    },
                    Scope::Cloudflare,
                    owner,
                    format!("{host} → {service} on {alias}"),
                )),
                Some(r) if r.service != *service => p.actions.push(Action::new(
                    Kind::UpdateIngress {
                        tunnel: alias.clone(),
                        tunnel_id: t.id.clone(),
                        account_id: obs.account_id.clone(),
                        host: host.to_string(),
                        from: r.service.clone(),
                        to: service.to_string(),
                    },
                    Scope::Cloudflare,
                    owner,
                    format!("{host} on {alias}: {} → {service}", r.service),
                )),
                Some(_) => {}
            }
        }
        for (host, service) in obs.routes() {
            if want.iter().any(|(h, _)| h.eq_ignore_ascii_case(&host)) {
                continue;
            }
            let why = match fleet.find_route(&host) {
                Some(r) => format!("the fleet sends {host} to {}", r.tunnel),
                None => "not in the fleet file".to_string(),
            };
            let moved = fleet.find_route(&host).is_some();
            let mut a = Action::new(
                Kind::RemoveIngress {
                    tunnel: alias.clone(),
                    tunnel_id: t.id.clone(),
                    account_id: obs.account_id.clone(),
                    host: host.clone(),
                    service: service.clone(),
                    why: why.clone(),
                },
                Scope::Cloudflare,
                owner,
                format!("{host} → {service} off {alias} ({why})"),
            );
            // Even a host the file moved elsewhere is only taken off by a
            // person or by `policy.prune`: the leftover is harmless once DNS
            // has moved, and removing it by inference is how a warm standby
            // would quietly lose its routes.
            let _ = moved;
            a.prune = true;
            p.actions.push(a);
        }
    }

    // --- DNS: each declared host should CNAME to its active tunnel
    for r in &fleet.routes {
        let active = r.active_tunnel();
        let Some(t) = fleet.tunnels.get(active) else { continue };
        let owner = t.machine.as_deref();
        let Some(zone) = snap.zone_for_host(&r.host) else {
            if !snap.zones.is_empty() {
                p.findings.push(finding(
                    Level::Warn,
                    &r.host,
                    "no API token here can see the zone this hostname is in, so its DNS cannot be checked".into(),
                    Some("tunnels token add <token with Zone:DNS:Edit for that zone>".into()),
                ));
            }
            continue;
        };
        if !snap.dns_known_for(&r.host) {
            continue;
        }
        let records = snap.dns_for(&r.host);
        match records.first() {
            None => p.actions.push(Action::new(
                Kind::CreateDns { zone_id: zone.id.clone(), host: r.host.clone(), tunnel: active.to_string(), tunnel_id: t.id.clone() },
                Scope::Cloudflare,
                owner,
                format!("DNS {} → {active}", r.host),
            )),
            Some(rec) if rec.tunnel_target().as_deref() == Some(t.id.to_ascii_lowercase().as_str()) => {}
            Some(rec) => {
                let from_id = rec.tunnel_target();
                let from_live = from_id
                    .as_deref()
                    .and_then(|id| snap.tunnel(id))
                    .map(|o| o.up() && (o.ingress.is_none() || o.has_host(&r.host)))
                    .unwrap_or(false);
                let from_desc = from_id
                    .as_deref()
                    .map(|id| describe_tunnel(fleet, snap, id))
                    .unwrap_or_else(|| rec.content.clone());
                let mut a = Action::new(
                    Kind::RepointDns {
                        zone_id: zone.id.clone(),
                        record_id: rec.id.clone(),
                        host: r.host.clone(),
                        from: rec.content.clone(),
                        tunnel: active.to_string(),
                        tunnel_id: t.id.clone(),
                    },
                    Scope::Cloudflare,
                    owner,
                    format!(
                        "DNS {}: {from_desc} → {active}{}",
                        r.host,
                        if from_live { " — TAKES OVER a hostname that tunnel is serving now" } else { "" }
                    ),
                );
                a.guarded = from_live;
                p.actions.push(a);
            }
        }
    }

    // --- DNS pointing at fleet tunnels for hosts the fleet does not route,
    // and DNS pointing at tunnels that no longer exist
    let all_known_ids: Vec<String> = snap.tunnels.iter().map(|t| t.tunnel.id.to_ascii_lowercase()).collect();
    for rec in &snap.dns {
        let Some(target) = rec.tunnel_target() else { continue };
        if fleet.find_route(&rec.name).is_some() {
            continue;
        }
        let fleet_tunnel = fleet.alias_for_id(&target);
        let zone_acct = snap.zones.iter().find(|z| z.id == rec.zone_id).map(|z| z.account_id.clone());
        let dangling = !all_known_ids.contains(&target)
            && zone_acct.as_deref().map(|a| snap.account_reachable(a)).unwrap_or(false)
            && snap.accounts.iter().all(|a| a.reachable);
        if let Some(alias) = fleet_tunnel {
            let mut a = Action::new(
                Kind::DeleteDns {
                    zone_id: rec.zone_id.clone(),
                    record_id: rec.id.clone(),
                    host: rec.name.clone(),
                    content: rec.content.clone(),
                    why: "not in the fleet file".into(),
                },
                Scope::Cloudflare,
                fleet.machine_of(alias),
                format!("DNS {} → {alias} (not in the fleet file)", rec.name),
            );
            a.prune = true;
            p.actions.push(a);
        } else if dangling {
            let mut a = Action::new(
                Kind::DeleteDns {
                    zone_id: rec.zone_id.clone(),
                    record_id: rec.id.clone(),
                    host: rec.name.clone(),
                    content: rec.content.clone(),
                    why: "points at a tunnel that no longer exists".into(),
                },
                Scope::Cloudflare,
                None,
                format!("DNS {} → {} (that tunnel no longer exists)", rec.name, short(&target)),
            );
            a.prune = true;
            p.actions.push(a);
        }
    }

    // --- orphans: tunnels in Cloudflare that the fleet does not know
    for obs in &snap.tunnels {
        if fleet.alias_for_id(&obs.tunnel.id).is_some() {
            continue;
        }
        let acct = fleet
            .account_alias_for_id(&obs.account_id)
            .cloned()
            .unwrap_or_else(|| short(&obs.account_id).to_string());
        let serving: Vec<String> = snap
            .dns
            .iter()
            .filter(|d| d.tunnel_target().as_deref() == Some(obs.tunnel.id.to_ascii_lowercase().as_str()))
            .map(|d| d.name.clone())
            .collect();
        p.findings.push(finding(
            Level::Warn,
            &obs.tunnel.name,
            format!(
                "tunnel {} ({}, account {acct}) is not in the fleet file. It exists in Cloudflare and its connector tokens still work{}{}",
                obs.tunnel.name,
                short(&obs.tunnel.id),
                if obs.up() { format!("; {} connector(s) are running it right now", obs.tunnel.connections.len()) } else { "; nothing is running it".into() },
                if serving.is_empty() { String::new() } else { format!("; DNS sends {} to it", serving.join(", ")) },
            ),
            Some(format!(
                "adopt it: tunnels tunnel adopt {} --as <alias> [--machine <m>] · or kill it: tunnels tunnel destroy {}",
                obs.tunnel.id, obs.tunnel.id
            )),
        ));
    }

    // --- failover state worth saying out loud
    for r in &fleet.routes {
        let Some(sb) = &r.standby else { continue };
        let prim = fleet.tunnels.get(&r.tunnel).and_then(|t| snap.tunnel(&t.id));
        let stby = fleet.tunnels.get(sb).and_then(|t| snap.tunnel(&t.id));
        if r.on_standby() {
            if prim.map(|o| o.up()).unwrap_or(false) {
                p.findings.push(finding(
                    Level::Info,
                    &r.host,
                    format!("serving from standby {sb}; the primary {} is up again", r.tunnel),
                    Some(format!("tunnels failback {}", r.host)),
                ));
            }
        } else if prim.map(|o| !o.up()).unwrap_or(false) && stby.map(|o| o.up()).unwrap_or(false) {
            p.findings.push(finding(
                Level::Warn,
                &r.host,
                format!("the primary {} is down and standby {sb} is up", r.tunnel),
                Some(format!("tunnels promote {}", r.host)),
            ));
        }
    }

    // --- this Mac
    if let Some(local) = local {
        let me = local.machine.as_str();
        for (alias, t) in &fleet.tunnels {
            if t.machine.as_deref() != Some(me) || t.destroy {
                continue;
            }
            let acct_id = fleet.accounts.get(&t.account).map(|a| a.id.clone()).unwrap_or_default();
            let here = local.tunnels.iter().find(|l| l.tunnel_id.as_deref().map(|i| i.eq_ignore_ascii_case(&t.id)).unwrap_or(false));
            match here {
                None => p.actions.push(Action::new(
                    Kind::StartLocal { tunnel: alias.clone(), tunnel_id: t.id.clone(), account_id: acct_id, local_name: None },
                    Scope::Local,
                    Some(me),
                    format!("run {alias} here (fetch its connector token, write its LaunchAgent)"),
                )),
                Some(l) if l.state == "no plist" => p.actions.push(Action::new(
                    Kind::StartLocal {
                        tunnel: alias.clone(),
                        tunnel_id: t.id.clone(),
                        account_id: acct_id,
                        local_name: Some(l.name.clone()),
                    },
                    Scope::Local,
                    Some(me),
                    format!("start {alias} (as {})", l.name),
                )),
                Some(l) if l.state == "not loaded" => p.actions.push(Action::new(
                    Kind::LoadLocal { local_name: l.name.clone() },
                    Scope::Local,
                    Some(me),
                    format!("load {alias} — its LaunchAgent is on disk but launchd has forgotten it"),
                )),
                Some(l) if l.pid.is_none() => p.actions.push(Action::new(
                    Kind::RestartLocal { local_name: l.name.clone(), why: "loaded, but no process".into() },
                    Scope::Local,
                    Some(me),
                    format!("restart {alias} — loaded, but no process"),
                )),
                Some(l) => {
                    if let Some(obs) = snap.tunnel(&t.id) {
                        if !obs.up() {
                            p.actions.push(Action::new(
                                Kind::RestartLocal { local_name: l.name.clone(), why: "running, but Cloudflare sees no connectors".into() },
                                Scope::Local,
                                Some(me),
                                format!("restart {alias} — running, but Cloudflare sees no connectors"),
                            ));
                        }
                    }
                }
            }
        }
        for l in &local.tunnels {
            let Some(id) = &l.tunnel_id else { continue };
            match fleet.alias_for_id(id) {
                Some(alias) => {
                    let decl = &fleet.tunnels[alias];
                    if decl.machine.as_deref() != Some(me) {
                        p.findings.push(finding(
                            Level::Warn,
                            &l.name,
                            format!(
                                "this Mac holds a connector token for {alias}, which the fleet runs on {}{}",
                                decl.machine.as_deref().unwrap_or("no fleet machine"),
                                if l.state == "loaded" { " — and it is running here too" } else { "" }
                            ),
                            Some(format!("tunnels tunnel forget {} (this Mac only; the tunnel keeps running where it belongs)", l.name)),
                        ));
                    }
                }
                None => {
                    let acct_ok = l.account_id.as_deref().map(|a| snap.account_reachable(a)).unwrap_or(false);
                    if acct_ok && snap.tunnel(id).is_none() {
                        p.actions.push(Action::new(
                            Kind::ForgetLocal {
                                local_name: l.name.clone(),
                                why: format!("tunnel {} no longer exists in Cloudflare", short(id)),
                            },
                            Scope::Local,
                            Some(me),
                            format!("forget {} — its tunnel no longer exists in Cloudflare", l.name),
                        ));
                    } else {
                        p.findings.push(finding(
                            Level::Info,
                            &l.name,
                            format!("runs here ({}) but is not in the fleet file", short(id)),
                            Some(format!("tunnels tunnel adopt {id} --as <alias> --machine {me}")),
                        ));
                    }
                }
            }
        }
        for s in &local.stray_plists {
            p.findings.push(finding(
                Level::Info,
                s,
                format!("a cloudflared LaunchAgent ({}) that this Mac's config does not know", crate::launchd::label_for(s)),
                Some("tunnels tunnel import-plists".into()),
            ));
        }
        if !local.agent_loaded {
            p.findings.push(finding(
                Level::Info,
                me,
                "the agent is not running here, so nothing keeps this Mac in line with the fleet".into(),
                Some("tunnels agent install".into()),
            ));
        }
    }

    p.findings.sort_by(|a, b| b.level.cmp(&a.level));
    p
}

/// Is anything answering on the local ports of the routes this Mac serves?
/// A route to a port with nothing behind it is the 502 nobody notices.
pub fn probe_origins(fleet: &Fleet, machine: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in &fleet.routes {
        let mine = [Some(r.tunnel.as_str()), r.standby.as_deref()]
            .into_iter()
            .flatten()
            .any(|t| fleet.machine_of(t) == Some(machine));
        if !mine {
            continue;
        }
        let Some(port) = crate::fleet::local_port(&r.service) else { continue };
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let ok = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
            || std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
                std::time::Duration::from_millis(300),
            )
            .is_ok();
        if !ok {
            out.push(finding(
                Level::Warn,
                &r.host,
                format!("nothing is listening on port {port} here, so {} answers 502", r.host),
                None,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cf::{Connection, DnsRecord, Ingress, Tunnel, Zone};
    use crate::fleet::tests::sample;
    use crate::observe::{AccountObs, LocalTunnel, TunnelObs};

    const PROD: &str = "6da56f03-7687-46a4-b023-7557decbc04b";
    const STBY: &str = "51e97a99-f3d8-4da9-9a7f-2fd2a0567776";
    const HOME: &str = "bc598bf7-c3a7-4b4c-aa94-4ac056aa87bc";
    const STRAY: &str = "56b5b4ef-68ce-4ed2-9eb0-46ef8e23d179";

    fn tun(account: &str, id: &str, name: &str, up: bool, hosts: &[(&str, &str)]) -> TunnelObs {
        TunnelObs {
            account_id: account.into(),
            tunnel: Tunnel {
                id: id.into(),
                name: name.into(),
                status: if up { "healthy".into() } else { "down".into() },
                connections: if up { vec![Connection::default()] } else { vec![] },
                ..Default::default()
            },
            ingress: Some(
                hosts.iter().map(|(h, s)| Ingress::new(h, s)).chain([Ingress::catch_all("http_status:404")]).collect(),
            ),
        }
    }

    fn cname(zone: &str, name: &str, tunnel: &str) -> DnsRecord {
        DnsRecord {
            id: format!("rec-{name}"),
            name: name.into(),
            rtype: "CNAME".into(),
            content: crate::cf::target_for(tunnel),
            proxied: true,
            zone_id: zone.into(),
        }
    }

    /// Everything exactly as the sample fleet says.
    fn converged() -> Snapshot {
        let mut s = Snapshot::default();
        s.accounts = vec![
            AccountObs { id: "acct-vet".into(), name: "vet".into(), reachable: true, zones: vec!["everyday.vet".into()] },
            AccountObs { id: "acct-home".into(), name: "home".into(), reachable: true, zones: vec!["felixflor.es".into()] },
        ];
        s.zones = vec![
            Zone { id: "z-vet".into(), name: "everyday.vet".into(), account_id: "acct-vet".into(), account_name: "vet".into() },
            Zone { id: "z-home".into(), name: "felixflor.es".into(), account_id: "acct-home".into(), account_name: "home".into() },
        ];
        s.dns_zones_seen = ["everyday.vet".to_string(), "felixflor.es".to_string()].into();
        s.tunnels = vec![
            tun("acct-vet", PROD, "dorkyrobot1", true, &[("admin.everyday.vet", "http://localhost:3300")]),
            tun("acct-vet", STBY, "DorkyRobot2", true, &[("admin.everyday.vet", "http://localhost:3300")]),
            tun("acct-home", HOME, "DorkyRobot2", true, &[("media.felixflor.es", "http://localhost:2283")]),
        ];
        s.dns = vec![cname("z-vet", "admin.everyday.vet", PROD), cname("z-home", "media.felixflor.es", HOME)];
        s
    }

    #[test]
    fn nothing_to_do_when_everything_matches() {
        let p = plan(&sample(), &converged(), None);
        assert!(p.actions.is_empty(), "{:#?}", p.actions);
        assert!(p.findings.is_empty(), "{:#?}", p.findings);
    }

    #[test]
    fn a_new_route_adds_ingress_and_dns_owned_by_its_machine() {
        let mut f = sample();
        f.routes.push(crate::fleet::Route {
            host: "id.felixflor.es".into(),
            tunnel: "dr2-home".into(),
            service: "http://localhost:1411".into(),
            ..Default::default()
        });
        let p = plan(&f, &converged(), None);
        assert_eq!(p.actions.len(), 2, "{:#?}", p.actions);
        assert!(matches!(p.actions[0].kind, Kind::AddIngress { .. }));
        assert!(matches!(p.actions[1].kind, Kind::CreateDns { .. }));
        assert!(p.actions.iter().all(|a| a.owner.as_deref() == Some("dr2") && a.automatic(false)));
    }

    #[test]
    fn taking_a_hostname_from_a_live_tunnel_is_guarded() {
        // staging-admin: DNS pointed at mac2024's live tunnel, and route add
        // moved it without a word
        let mut s = converged();
        s.tunnels.push(tun("acct-vet", STRAY, "mac-2024", true, &[("admin.everyday.vet", "http://localhost:3000")]));
        s.dns = vec![cname("z-vet", "admin.everyday.vet", STRAY), cname("z-home", "media.felixflor.es", HOME)];
        let p = plan(&sample(), &s, None);
        let repoint = p.actions.iter().find(|a| matches!(a.kind, Kind::RepointDns { .. })).expect("repoint");
        assert!(repoint.guarded, "a live takeover must be guarded");
        assert!(!repoint.automatic(true), "no agent does a takeover on its own");
        assert!(repoint.summary.contains("TAKES OVER"), "{}", repoint.summary);
        // and the tunnel it comes from is an orphan worth naming
        assert!(p.findings.iter().any(|f| f.message.contains("mac-2024") && f.message.contains("tokens still work")));
    }

    #[test]
    fn repointing_away_from_a_dead_tunnel_is_not_guarded() {
        let mut s = converged();
        s.tunnels.push(tun("acct-vet", STRAY, "mac-2024", false, &[("admin.everyday.vet", "http://localhost:3000")]));
        s.dns[0] = cname("z-vet", "admin.everyday.vet", STRAY);
        let p = plan(&sample(), &s, None);
        let repoint = p.actions.iter().find(|a| matches!(a.kind, Kind::RepointDns { .. })).unwrap();
        assert!(!repoint.guarded);
    }

    #[test]
    fn undeclared_ingress_is_only_removed_when_pruning() {
        let mut s = converged();
        // an extra host nobody declared, and media also on the standby tunnel
        s.tunnels[1] = tun(
            "acct-vet",
            STBY,
            "DorkyRobot2",
            true,
            &[("admin.everyday.vet", "http://localhost:3300"), ("old.everyday.vet", "http://localhost:9")],
        );
        let p = plan(&sample(), &s, None);
        let rm = p.actions.iter().find(|a| matches!(&a.kind, Kind::RemoveIngress { host, .. } if host == "old.everyday.vet")).unwrap();
        assert!(rm.prune && !rm.automatic(false) && rm.automatic(true));
    }

    #[test]
    fn a_changed_service_updates_in_place() {
        let mut f = sample();
        f.find_route_mut("media.felixflor.es").unwrap().service = "http://localhost:2284".into();
        let p = plan(&f, &converged(), None);
        assert_eq!(p.actions.len(), 1);
        assert!(matches!(&p.actions[0].kind, Kind::UpdateIngress { from, to, .. } if from.ends_with("2283") && to.ends_with("2284")));
    }

    #[test]
    fn promotion_moves_dns_to_the_standby_and_its_owner() {
        let mut f = sample();
        f.find_route_mut("admin.everyday.vet").unwrap().active = Some("standby".into());
        let mut s = converged();
        s.tunnels[0] = tun("acct-vet", PROD, "dorkyrobot1", false, &[("admin.everyday.vet", "http://localhost:3300")]);
        let p = plan(&f, &s, None);
        let repoint = p.actions.iter().find(|a| matches!(a.kind, Kind::RepointDns { .. })).unwrap();
        assert_eq!(repoint.owner.as_deref(), Some("dr2"), "the standby's machine owns DNS now");
        assert!(!repoint.guarded, "the primary is down, nothing is taken from anyone");
    }

    #[test]
    fn a_down_primary_with_a_live_standby_says_promote() {
        let mut s = converged();
        s.tunnels[0].tunnel.connections.clear();
        let p = plan(&sample(), &s, None);
        assert!(p.findings.iter().any(|f| f.fix.as_deref() == Some("tunnels promote admin.everyday.vet")));
    }

    #[test]
    fn a_tunnel_marked_for_destruction_is_a_destroy_action() {
        let mut f = sample();
        f.routes.retain(|r| r.tunnel != "dr2-home");
        f.tunnels.get_mut("dr2-home").unwrap().destroy = true;
        let p = plan(&f, &converged(), None);
        let d = p.actions.iter().find(|a| matches!(a.kind, Kind::Destroy { .. })).unwrap();
        assert!(d.destroy && d.guarded && !d.automatic(true));
        assert!(d.summary.contains("connector token"), "{}", d.summary);
    }

    #[test]
    fn unknown_is_not_missing() {
        // ingress not fetched and DNS not listed: nothing to add or remove
        let mut s = converged();
        for t in &mut s.tunnels {
            t.ingress = None;
        }
        s.dns.clear();
        s.dns_zones_seen.clear();
        let p = plan(&sample(), &s, None);
        assert!(p.actions.is_empty(), "{:#?}", p.actions);
    }

    #[test]
    fn a_token_for_a_tunnel_that_runs_elsewhere_is_named() {
        // mac2024 holds production's connector token under the name dorkyrobot1
        let local = LocalObs {
            machine: "dr2".into(),
            tunnels: vec![
                LocalTunnel {
                    name: "DorkyRobot".into(),
                    tunnel_id: Some(STBY.into()),
                    account_id: Some("acct-vet".into()),
                    state: "loaded".into(),
                    pid: Some(1),
                    label: String::new(),
                    inline_token: false,
                },
                LocalTunnel {
                    name: "DorkyRobot2".into(),
                    tunnel_id: Some(HOME.into()),
                    account_id: Some("acct-home".into()),
                    state: "loaded".into(),
                    pid: Some(2),
                    label: String::new(),
                    inline_token: false,
                },
                LocalTunnel {
                    name: "dorkyrobot1".into(),
                    tunnel_id: Some(PROD.into()),
                    account_id: Some("acct-vet".into()),
                    state: "no plist".into(),
                    pid: None,
                    label: String::new(),
                    inline_token: false,
                },
            ],
            stray_plists: vec![],
            agent_loaded: true,
        };
        let p = plan(&sample(), &converged(), Some(&local));
        assert!(p.actions.is_empty(), "{:#?}", p.actions);
        let f = p.findings.iter().find(|f| f.subject == "dorkyrobot1").unwrap();
        assert!(f.message.contains("runs on dr1"), "{}", f.message);
        assert!(f.fix.as_deref().unwrap().contains("forget"));
    }

    #[test]
    fn a_booted_out_tunnel_is_loaded_again() {
        let local = LocalObs {
            machine: "dr2".into(),
            tunnels: vec![LocalTunnel {
                name: "DorkyRobot2".into(),
                tunnel_id: Some(HOME.into()),
                account_id: Some("acct-home".into()),
                state: "not loaded".into(),
                pid: None,
                label: String::new(),
                inline_token: false,
            }],
            stray_plists: vec![],
            agent_loaded: true,
        };
        let p = plan(&sample(), &converged(), Some(&local));
        assert!(p.actions.iter().any(|a| matches!(&a.kind, Kind::LoadLocal { local_name } if local_name == "DorkyRobot2")));
        // and the standby, which this Mac should run but does not, is started
        assert!(p.actions.iter().any(|a| matches!(&a.kind, Kind::StartLocal { tunnel, .. } if tunnel == "vet-standby")));
    }
}
