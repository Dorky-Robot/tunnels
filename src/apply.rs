//! Carrying out a plan.
//!
//! Order matters and is fixed: ingress first, so a hostname never resolves
//! to a tunnel that does not know it yet; then DNS; then removals; then
//! anything destroyed. A route whose DNS step fails has its ingress change
//! undone — `route add` used to leave the ingress behind with "DNS: FAILED"
//! and a wrong explanation, a half-made route somebody had to clean up.

use crate::cf::{self, Client, Ingress};
use crate::config::Config;
use crate::launchd;
use crate::observe::Snapshot;
use crate::plan::{Action, Kind, Plan};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// also remove things the fleet file does not mention
    pub prune: bool,
    /// also take hostnames from tunnels serving them now
    pub yes: bool,
    /// also destroy tunnels marked `destroy = true`
    pub allow_destroy: bool,
    /// only actions this machine owns (the agent)
    pub only_owner: Option<String>,
    /// only actions about these hostnames
    pub only_hosts: Option<Vec<String>>,
    /// skip actions that touch this Mac
    pub no_local: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub summary: String,
    pub ok: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rolled_back: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Report {
    pub done: Vec<Outcome>,
    /// actions in the plan that these options did not allow, and why
    pub held: Vec<(String, String)>,
}

impl Report {
    pub fn failed(&self) -> usize {
        self.done.iter().filter(|o| !o.ok).count()
    }
}

/// Which actions these options allow, and why the others are held back.
pub fn select<'a>(plan: &'a Plan, opts: &Options) -> (Vec<&'a Action>, Vec<(String, String)>) {
    let mut run = Vec::new();
    let mut held = Vec::new();
    for a in &plan.actions {
        if let Some(me) = &opts.only_owner {
            if a.owner.as_deref() != Some(me.as_str()) {
                continue;
            }
        }
        if let Some(hosts) = &opts.only_hosts {
            let about = match &a.kind {
                Kind::Destroy { .. } => false,
                _ => a.host().map(|h| hosts.iter().any(|x| x.eq_ignore_ascii_case(h))).unwrap_or(false),
            };
            if !about {
                continue;
            }
        }
        if opts.no_local && a.scope == crate::scope::Scope::Local {
            continue;
        }
        let why = if a.destroy && !opts.allow_destroy {
            Some("destroys a tunnel — needs --allow-destroy")
        } else if a.guarded && !a.destroy && !opts.yes {
            Some("takes a hostname from a tunnel serving it now — needs --yes")
        } else if a.prune && !opts.prune {
            Some("removes something the fleet file does not mention — needs --prune")
        } else {
            None
        };
        match why {
            Some(w) => held.push((a.summary.clone(), w.to_string())),
            None => run.push(a),
        }
    }
    (run, held)
}

fn rank(a: &Action) -> u8 {
    match a.kind {
        Kind::ForgetLocal { .. } | Kind::StartLocal { .. } | Kind::LoadLocal { .. } | Kind::RestartLocal { .. } => 0,
        Kind::AddIngress { .. } | Kind::UpdateIngress { .. } => 1,
        Kind::CreateDns { .. } | Kind::RepointDns { .. } => 2,
        Kind::DeleteDns { .. } => 3,
        Kind::RemoveIngress { .. } => 4,
        Kind::Destroy { .. } => 5,
    }
}

/// What an ingress step changed, so it can be put back.
struct Undo {
    account_id: String,
    tunnel_id: String,
    before: Vec<Ingress>,
    host: String,
}

pub fn apply(plan: &Plan, snap: &Snapshot, config: &mut Config, opts: &Options) -> Report {
    let (mut run, held) = select(plan, opts);
    run.sort_by_key(|a| rank(a));
    let mut report = Report { held, ..Default::default() };
    let mut undo: Vec<Undo> = Vec::new();

    for a in run {
        let result = step(a, snap, config, opts, &mut undo);
        let mut outcome = Outcome {
            summary: a.summary.clone(),
            ok: result.is_ok(),
            detail: match &result {
                Ok(d) => d.clone(),
                Err(e) => format!("{e:#}"),
            },
            rolled_back: false,
        };
        // a DNS step that failed takes its route's ingress change back with it
        if result.is_err() {
            if let Some(host) = matches!(a.kind, Kind::CreateDns { .. } | Kind::RepointDns { .. }).then(|| a.host()).flatten() {
                for u in undo.iter().filter(|u| u.host.eq_ignore_ascii_case(host)) {
                    if let Some(c) = snap.client_for_account(&u.account_id) {
                        match c.put_ingress(&u.account_id, &u.tunnel_id, &u.before) {
                            Ok(()) => {
                                outcome.rolled_back = true;
                                outcome.detail.push_str("; the ingress change for this host was undone");
                            }
                            Err(e) => outcome.detail.push_str(&format!("; undoing the ingress change ALSO failed: {e}")),
                        }
                    }
                }
            }
        }
        report.done.push(outcome);
    }
    report
}

fn client_for_account(snap: &Snapshot, account_id: &str) -> Result<Client> {
    snap.client_for_account(account_id)
        .ok_or_else(|| anyhow!("no API token here can reach account {account_id}"))
}

fn client_for_zone(snap: &Snapshot, zone_id: &str) -> Result<Client> {
    snap.client_for_zone(zone_id).ok_or_else(|| anyhow!("no API token here can reach that zone"))
}

fn step(a: &Action, snap: &Snapshot, config: &mut Config, opts: &Options, undo: &mut Vec<Undo>) -> Result<String> {
    match &a.kind {
        Kind::AddIngress { tunnel_id, account_id, host, service, .. }
        | Kind::UpdateIngress { tunnel_id, account_id, host, to: service, .. } => {
            let c = client_for_account(snap, account_id)?;
            let before = c.ingress(account_id, tunnel_id)?;
            let mut rules = before.clone();
            let catch_all = rules
                .iter()
                .position(|r| r.hostname.is_none() && r.path.is_none())
                .map(|i| rules.remove(i))
                .unwrap_or_else(|| Ingress::catch_all("http_status:404"));
            match rules.iter_mut().find(|r| r.hostname.as_deref().map(|h| h.eq_ignore_ascii_case(host)).unwrap_or(false)) {
                Some(r) if r.service == *service => return Ok("already so".into()),
                Some(r) => *r = r.with_service(service),
                None => rules.push(Ingress::new(host, service)),
            }
            rules.push(catch_all);
            c.put_ingress(account_id, tunnel_id, &rules)?;
            undo.push(Undo { account_id: account_id.clone(), tunnel_id: tunnel_id.clone(), before, host: host.clone() });
            Ok("ingress written".into())
        }
        Kind::RemoveIngress { tunnel_id, account_id, host, .. } => {
            let c = client_for_account(snap, account_id)?;
            let before = c.ingress(account_id, tunnel_id)?;
            let rules: Vec<Ingress> = before
                .iter()
                .filter(|r| !r.hostname.as_deref().map(|h| h.eq_ignore_ascii_case(host)).unwrap_or(false))
                .cloned()
                .collect();
            if rules.len() == before.len() {
                return Ok("already gone".into());
            }
            c.put_ingress(account_id, tunnel_id, &rules)?;
            Ok("ingress removed".into())
        }
        Kind::CreateDns { zone_id, host, tunnel_id, .. } => {
            let c = client_for_zone(snap, zone_id)?;
            let existing = c.dns_records_named(zone_id, host)?;
            if let Some(rec) = existing.iter().find(|r| r.rtype == "CNAME") {
                if rec.tunnel_target().as_deref() == Some(tunnel_id.to_ascii_lowercase().as_str()) {
                    return Ok("already so".into());
                }
                // appeared since the snapshot: only replace it if the plan was allowed to
                if !opts.yes {
                    bail!("{host} now has a CNAME to {} — rerun `tunnels plan` to see it, or --yes to replace it", rec.content);
                }
                c.update_cname(zone_id, &rec.id, host, &cf::target_for(tunnel_id))?;
                return Ok("CNAME replaced".into());
            }
            let others: Vec<_> = existing.iter().filter(|r| r.rtype != "CNAME").collect();
            if !others.is_empty() {
                let list = others.iter().map(|r| format!("{} {}", r.rtype, r.content)).collect::<Vec<_>>().join(", ");
                if !opts.yes {
                    bail!("{host} already has other records ({list}); a CNAME cannot sit beside them — --yes to replace them");
                }
                for r in others {
                    c.delete_record(zone_id, &r.id).with_context(|| format!("deleting {} {}", r.rtype, r.content))?;
                }
            }
            c.create_cname(zone_id, host, tunnel_id).map_err(|e| dns_error(e, host, snap))?;
            Ok("CNAME created".into())
        }
        Kind::RepointDns { zone_id, record_id, host, tunnel_id, .. } => {
            let c = client_for_zone(snap, zone_id)?;
            c.update_cname(zone_id, record_id, host, &cf::target_for(tunnel_id)).map_err(|e| dns_error(e, host, snap))?;
            Ok("CNAME repointed".into())
        }
        Kind::DeleteDns { zone_id, record_id, .. } => {
            let c = client_for_zone(snap, zone_id)?;
            c.delete_record(zone_id, record_id)?;
            Ok("record deleted".into())
        }
        Kind::Destroy { tunnel_id, account_id, .. } => {
            destroy(snap, config, account_id, tunnel_id)
        }
        Kind::StartLocal { tunnel, tunnel_id, account_id, local_name } => {
            let (name, token) = match local_name.as_ref().and_then(|n| config.tunnel_by_name(n)) {
                Some(t) => (t.name.clone(), t.token.clone()),
                None => {
                    let c = client_for_account(snap, account_id).context(
                        "this Mac has no connector token for it, and no API token that could fetch one",
                    )?;
                    let token = c.connector_token(account_id, tunnel_id).context("fetching the connector token")?;
                    config.upsert_tunnel(tunnel, &token)?;
                    (tunnel.clone(), token)
                }
            };
            launchd::start(&name, &token)?;
            Ok(format!("running as {}", launchd::label_for(&name)))
        }
        Kind::LoadLocal { local_name } => {
            launchd::bootstrap_existing(local_name)?;
            Ok("loaded".into())
        }
        Kind::RestartLocal { local_name, .. } => {
            if launchd::is_loaded_name(local_name) {
                launchd::kickstart(local_name)?;
            } else {
                launchd::bootstrap_existing(local_name)?;
            }
            Ok("restarted".into())
        }
        Kind::ForgetLocal { local_name, .. } => {
            forget_local(config, local_name)?;
            Ok("forgotten here".into())
        }
    }
}

/// Delete a tunnel in Cloudflare: the DNS that points at it, its
/// connections, then the tunnel. After this every connector token issued
/// for it is dead. If this Mac was running it, it is forgotten here too.
pub fn destroy(snap: &Snapshot, config: &mut Config, account_id: &str, tunnel_id: &str) -> Result<String> {
    let c = client_for_account(snap, account_id)?;
    let target = tunnel_id.to_ascii_lowercase();
    let mut notes = Vec::new();
    for rec in snap.dns.iter().filter(|r| r.tunnel_target().as_deref() == Some(target.as_str())) {
        match client_for_zone(snap, &rec.zone_id).and_then(|zc| zc.delete_record(&rec.zone_id, &rec.id).map_err(Into::into)) {
            Ok(()) => notes.push(format!("DNS {} deleted", rec.name)),
            Err(e) => notes.push(format!("DNS {} NOT deleted: {e}", rec.name)),
        }
    }
    // a tunnel with live connections cannot be deleted; clear them first
    let _ = c.delete_connections(account_id, tunnel_id);
    c.delete_tunnel(account_id, tunnel_id).context("deleting the tunnel")?;
    notes.push("tunnel deleted — its connector tokens no longer work".into());
    let here: Vec<String> = config
        .tunnels
        .iter()
        .filter(|t| t.tunnel_id().as_deref().map(|i| i.eq_ignore_ascii_case(tunnel_id)).unwrap_or(false))
        .map(|t| t.name.clone())
        .collect();
    for name in here {
        forget_local(config, &name)?;
        notes.push(format!("forgotten on this Mac ({name})"));
    }
    Ok(notes.join("; "))
}

/// Stop a tunnel here and remove every trace of it on this Mac: the
/// LaunchAgent, the config entry, the token file. Nothing in Cloudflare.
pub fn forget_local(config: &mut Config, name: &str) -> Result<()> {
    let id = config.tunnel_by_name(name).and_then(|t| t.tunnel_id());
    launchd::stop(name)?;
    if config.tunnel_by_name(name).is_some() {
        let actual = config.tunnel_by_name(name).unwrap().name.clone();
        config.remove(&actual)?;
    }
    if let Some(id) = id {
        let _ = std::fs::remove_file(launchd::token_file(&id));
    }
    Ok(())
}

/// Say what actually went wrong with DNS. The old message blamed token
/// permissions for everything, including the real cause the one time it
/// mattered: a tunnel in a different account from the hostname's zone.
fn dns_error(e: cf::CfError, host: &str, snap: &Snapshot) -> anyhow::Error {
    let zone = snap.zone_for_host(host);
    let hint = if e.status == 403 || e.message.contains("[10000]") || e.message.contains("uthentication") {
        "the API token for this zone lacks Zone › DNS › Edit"
    } else if zone.is_none() {
        "no API token here can see this hostname's zone"
    } else {
        ""
    };
    if hint.is_empty() { anyhow!("{e}") } else { anyhow!("{e} ({hint})") }
}
