//! The agent: one per machine, run by launchd, keeping this Mac in line
//! with the fleet file.
//!
//! Each pass it
//!   1. takes the newest fleet file any peer has,
//!   2. looks at what this Mac owns — its tunnels, their ingress, the DNS
//!      of the hostnames they carry,
//!   3. brings its cloudflared jobs back if launchd forgot them (what the
//!      watchdog script did), restarts ones Cloudflare sees no connectors
//!      from, and fetches a fresh connector token if its tunnel was rotated,
//!   4. repairs ingress and DNS it owns that drifted from the file,
//!   5. fails a hostname over to a standby it runs, when the file says
//!      `failover = "auto"` and the primary has been down long enough.
//!
//! It never takes a hostname from a live tunnel, never removes what the
//! file does not mention unless `policy.prune` says so, never destroys
//! anything, and never boots out a job it is not about to load again.
//!
//! It also serves the web UI and its copy of the fleet file, on the tailnet
//! only (see `web.rs`).

use crate::apply::{self, Options, Outcome};
use crate::config::Config;
use crate::fleet::{self, Failover, Fleet};
use crate::observe::{self, Want};
use crate::plan::{self, Kind};
use crate::{launchd, sync, util};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub at: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Tick {
    pub at: String,
    pub serial: u64,
    pub took_ms: u128,
    pub done: Vec<Outcome>,
    pub waiting: Vec<(String, String)>,
    pub findings: usize,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct State {
    pub machine: String,
    pub started_at: String,
    pub last_tick: Option<Tick>,
    pub events: VecDeque<Event>,
    /// hosts that told us they have a newer fleet copy
    pub pull_from: Vec<String>,
    pub wake: bool,
    /// per local tunnel: passes in a row it has looked unhealthy, and when it
    /// was last restarted
    unhealthy: BTreeMap<String, (u32, i64)>,
    /// per route: when this agent first saw its primary down
    down_since: BTreeMap<String, i64>,
}

pub type Shared = Arc<(Mutex<State>, Condvar)>;

impl State {
    pub fn event(&mut self, kind: &str, message: String) {
        eprintln!("{} {kind}: {message}", util::now_rfc3339());
        self.events.push_front(Event { at: util::now_rfc3339(), kind: kind.into(), message });
        self.events.truncate(200);
    }
}

pub fn wake(shared: &Shared, from: Option<String>) {
    let (m, cv) = &**shared;
    let mut s = m.lock().unwrap();
    if let Some(h) = from {
        if !s.pull_from.contains(&h) {
            s.pull_from.push(h);
        }
    }
    s.wake = true;
    cv.notify_all();
}

/// Where the running binary really is; when `brew upgrade` swaps it, the
/// agent exits and launchd starts the new one.
fn exe_identity() -> Option<(std::path::PathBuf, std::time::SystemTime)> {
    let argv0 = std::env::args().next()?;
    let p = if argv0.contains('/') { std::path::PathBuf::from(argv0) } else { std::env::current_exe().ok()? };
    let real = std::fs::canonicalize(&p).ok()?;
    let mtime = std::fs::metadata(&real).ok()?.modified().ok()?;
    Some((real, mtime))
}

pub fn run() -> Result<()> {
    crate::scope::enter(crate::scope::Scope::LocalAndCloudflare);
    let config = Config::load().unwrap_or_default();
    let fleet = Fleet::load().ok().flatten();
    let machine = fleet::this_machine(&config, fleet.as_ref());
    let port = fleet.as_ref().map(|f| f.policy.web_port).unwrap_or(fleet::DEFAULT_WEB_PORT);
    let shared: Shared = Arc::new((
        Mutex::new(State { machine: machine.clone(), started_at: util::now_rfc3339(), ..Default::default() }),
        Condvar::new(),
    ));
    eprintln!("{} tunnels agent {} starting as {machine}", util::now_rfc3339(), env!("CARGO_PKG_VERSION"));
    crate::web::spawn(shared.clone(), port);
    // Say we are here. A machine that joins edits the fleet file before its
    // agent is listening, so the peers' fetch of that edit fails and they do
    // not know to look again — doug-mini's import sat unseen until it was
    // announced by hand. Once the web server is up, announce it.
    if let Some(f) = fleet.clone() {
        let me = machine.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            sync::notify(&f, &me);
        });
    }

    let exe = exe_identity();
    loop {
        if let (Some((p0, t0)), Some((p1, t1))) = (&exe, exe_identity()) {
            if *p0 != p1 || *t0 != t1 {
                eprintln!("{} the tunnels binary changed; exiting so launchd starts the new one", util::now_rfc3339());
                std::process::exit(0);
            }
        }
        let started = std::time::Instant::now();
        let mut t = match tick(&shared) {
            Ok(t) => t,
            Err(e) => Tick { at: util::now_rfc3339(), error: Some(format!("{e:#}")), ..Default::default() },
        };
        t.took_ms = started.elapsed().as_millis();
        let interval = Fleet::load().ok().flatten().map(|f| f.policy.interval).unwrap_or(120).max(15);
        let (m, cv) = &*shared;
        let mut s = m.lock().unwrap();
        if let Some(e) = &t.error {
            s.event("error", e.clone());
        }
        s.last_tick = Some(t);
        let deadline = std::time::Instant::now() + Duration::from_secs(interval);
        while !s.wake {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            s = cv.wait_timeout(s, deadline - now).unwrap().0;
        }
        s.wake = false;
    }
}

fn tick(shared: &Shared) -> Result<Tick> {
    let (m, _) = &**shared;
    let mut config = Config::load()?;
    let extra: Vec<String> = std::mem::take(&mut m.lock().unwrap().pull_from);
    let existing = Fleet::load()?;
    let me = fleet::this_machine(&config, existing.as_ref());
    m.lock().unwrap().machine = me.clone();

    // 1. the newest fleet file anyone has
    if existing.is_some() {
        match sync::pull(&me, &extra, Duration::from_secs(4)) {
            Ok(Some((host, serial))) => m.lock().unwrap().event("sync", format!("took fleet serial {serial} from {host}")),
            Ok(None) => {}
            Err(e) => m.lock().unwrap().event("error", format!("syncing the fleet file: {e:#}")),
        }
    }
    let Some(mut fleet) = Fleet::load()? else {
        return Ok(Tick {
            at: util::now_rfc3339(),
            error: Some("no fleet file on this machine yet — `tunnels import` or `tunnels fleet join <host>`".into()),
            ..Default::default()
        });
    };

    // 2. look at what this machine is responsible for
    let mine: BTreeSet<String> =
        fleet.tunnels.iter().filter(|(_, t)| t.machine.as_deref() == Some(me.as_str())).map(|(a, _)| a.clone()).collect();
    let want = want_for(&fleet, &mine);
    let snap = observe::observe(&config, &want);
    let local = observe::observe_local(&config, &me);

    // 5. failover first: it changes which tunnel DNS should point at
    if let Some(changed) = failover(&fleet, &snap, &me, shared) {
        fleet = changed;
        sync::notify(&fleet, &me);
    }

    // 3a. a rotated tunnel: fetch the new connector token
    for alias in &mine {
        let t = &fleet.tunnels[alias];
        let Some(local_t) = config.tunnel_by_id(&t.id).cloned() else { continue };
        let Some(obs) = snap.tunnel(&t.id) else { continue };
        let Some(client) = snap.client_for_account(&obs.account_id) else { continue };
        let Ok(fresh) = client.connector_token(&obs.account_id, &t.id) else { continue };
        let old_secret = crate::config::decode_token(&local_t.token).map(|p| p.secret).unwrap_or_default();
        let new_secret = crate::config::decode_token(&fresh).map(|p| p.secret).unwrap_or_default();
        if !new_secret.is_empty() && old_secret != new_secret {
            let r = (|| -> Result<()> {
                config.upsert_tunnel(&local_t.name, &fresh)?;
                launchd::write_token_file(&fresh)?;
                if launchd::plist_has_inline_token(&local_t.name) || !launchd::plist_runs_token(&local_t.name, &fresh) {
                    launchd::restart(&local_t.name, &fresh)?;
                } else {
                    launchd::kickstart(&local_t.name)?;
                }
                Ok(())
            })();
            let mut s = m.lock().unwrap();
            match r {
                Ok(()) => s.event("heal", format!("{alias}: its tunnel was rotated; took the new connector token and restarted it")),
                Err(e) => s.event("error", format!("{alias}: taking the rotated token: {e:#}")),
            }
        }
    }

    // 3b + 4. everything else the plan says this machine owns
    let mut p = plan::plan(&fleet, &snap, Some(&local));
    let findings = p.findings.len();
    let now = util::now_epoch();
    {
        let mut s = m.lock().unwrap();
        let unhealthy: BTreeSet<String> = p
            .actions
            .iter()
            .filter_map(|a| match &a.kind {
                Kind::RestartLocal { local_name, .. } => Some(local_name.clone()),
                _ => None,
            })
            .collect();
        s.unhealthy.retain(|k, _| unhealthy.contains(k));
        for n in &unhealthy {
            s.unhealthy.entry(n.clone()).or_insert((0, 0)).0 += 1;
        }
        // restart only after two passes in a row, and not more than once in
        // five minutes: a connector that is still coming up is left alone
        let due: BTreeSet<String> = s
            .unhealthy
            .iter()
            .filter(|(_, (n, last))| *n >= 2 && now - *last >= 300)
            .map(|(k, _)| k.clone())
            .collect();
        p.actions.retain(|a| match &a.kind {
            Kind::RestartLocal { local_name, .. } => due.contains(local_name),
            _ => true,
        });
        for n in &due {
            if let Some(e) = s.unhealthy.get_mut(n) {
                e.1 = now;
            }
        }
    }
    let opts = Options { only_owner: Some(me.clone()), prune: fleet.policy.prune, ..Default::default() };
    let (_, waiting) = apply::select(&p, &opts);
    let report = apply::apply(&p, &snap, &mut config, &opts);
    {
        let mut s = m.lock().unwrap();
        for o in &report.done {
            s.event(if o.ok { "heal" } else { "error" }, format!("{} — {}", o.summary, o.detail));
        }
    }
    Ok(Tick {
        at: util::now_rfc3339(),
        serial: fleet.serial,
        took_ms: 0,
        done: report.done,
        waiting,
        findings,
        error: None,
    })
}

/// Only what this machine owns, so six agents stay far inside Cloudflare's
/// rate limits: its tunnels' ingress, the DNS of the zones its hostnames are
/// in, and the tunnel lists of the accounts involved (which carry every
/// tunnel's connectors, primaries included).
pub fn want_for(fleet: &Fleet, mine: &BTreeSet<String>) -> Want {
    let mut ingress = BTreeSet::new();
    let mut zones = BTreeSet::new();
    let mut accounts = BTreeSet::new();
    for alias in mine {
        let t = &fleet.tunnels[alias];
        ingress.insert(t.id.clone());
        if let Some(a) = fleet.accounts.get(&t.account) {
            accounts.insert(a.id.clone());
            if fleet.policy.prune {
                zones.extend(a.zones.iter().cloned());
            }
        }
    }
    for r in &fleet.routes {
        let involved = mine.contains(&r.tunnel) || r.standby.as_ref().map(|s| mine.contains(s)).unwrap_or(false);
        if !involved {
            continue;
        }
        if let Some((_, a)) = fleet.account_for_host(&r.host) {
            accounts.insert(a.id.clone());
            if let Some(z) = crate::cf::zone_for(&r.host, a.zones.iter().map(|s| s.as_str())) {
                zones.insert(z.to_string());
            }
        }
    }
    Want { ingress_for: Some(ingress), dns_zones: Some(zones), accounts: Some(accounts) }
}

/// Move hostnames to a standby this machine runs, when the file allows it
/// and the primary has been down long enough. The decision is written to the
/// fleet file (`active = "standby"`), so every machine — including the
/// primary when it comes back — agrees where traffic goes, and nothing
/// flaps back on its own. `tunnels failback` returns it.
fn failover(fleet: &Fleet, snap: &observe::Snapshot, me: &str, shared: &Shared) -> Option<Fleet> {
    let (m, _) = &**shared;
    let now = util::now_epoch();
    let mut to_promote = Vec::new();
    for r in &fleet.routes {
        let Some(sb) = &r.standby else { continue };
        if fleet.machine_of(sb) != Some(me) || r.on_standby() {
            m.lock().unwrap().down_since.remove(&r.host);
            continue;
        }
        let prim = fleet.tunnels.get(&r.tunnel).and_then(|t| snap.tunnel(&t.id));
        let stby = fleet.tunnels.get(sb).and_then(|t| snap.tunnel(&t.id));
        let (Some(prim), Some(stby)) = (prim, stby) else { continue };
        if prim.up() || !stby.up() {
            m.lock().unwrap().down_since.remove(&r.host);
            continue;
        }
        let first_seen = *m.lock().unwrap().down_since.entry(r.host.clone()).or_insert(now);
        let since = prim.tunnel.conns_inactive_at.as_deref().and_then(util::parse_rfc3339).map(|t| t.min(first_seen)).unwrap_or(first_seen);
        let down_for = now - since;
        if r.failover == Some(Failover::Auto) && down_for >= fleet.policy.failover_after as i64 {
            to_promote.push((r.host.clone(), down_for));
        }
    }
    if to_promote.is_empty() {
        return None;
    }
    let hosts: Vec<String> = to_promote.iter().map(|(h, _)| h.clone()).collect();
    match Fleet::edit(me, |f| {
        for h in &hosts {
            if let Some(r) = f.find_route_mut(h) {
                r.active = Some("standby".into());
            }
        }
        Ok(())
    }) {
        Ok(f) => {
            let mut s = m.lock().unwrap();
            for (h, d) in &to_promote {
                s.event("failover", format!("{h}: primary down {}s — traffic moved to the standby here", d));
            }
            Some(f)
        }
        Err(e) => {
            m.lock().unwrap().event("error", format!("failover: {e:#}"));
            None
        }
    }
}
