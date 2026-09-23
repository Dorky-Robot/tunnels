//! The fleet file: what every machine should be running, and where every
//! hostname should go.
//!
//! Cloudflare holds the truth about what *is* routed; this file says what
//! *should be*. `tunnels plan` is the difference between the two, and the
//! agents close it. It holds no secrets — tunnel ids, hostnames, which
//! machine runs what — so it can be copied freely between machines, and
//! it is: every agent keeps a copy and serves it to the others over the
//! tailnet, so there is no central server whose absence stops the mesh
//! from healing. The newest copy wins, by `serial`.
//!
//! Tunnels are named by an alias you choose and identified by their
//! Cloudflare id. Local names used to be the only handle, and two machines
//! each had a different tunnel called `DorkyRobot`.

use crate::util;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fleet {
    /// bumped on every change; the highest serial is the current file
    #[serde(default)]
    pub serial: u64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub machines: BTreeMap<String, Machine>,
    #[serde(default)]
    pub accounts: BTreeMap<String, Account>,
    #[serde(default)]
    pub tunnels: BTreeMap<String, TunnelDecl>,
    #[serde(default)]
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    /// seconds between agent passes
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// seconds a primary must have been down before an automatic failover
    #[serde(default = "default_failover_after")]
    pub failover_after: u64,
    /// the port each agent serves the web UI and its fleet copy on (tailnet only)
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    /// let agents remove ingress and DNS on fleet tunnels that the file does
    /// not declare. Off, those show up in `plan` and wait for a person.
    #[serde(default)]
    pub prune: bool,
}

fn default_interval() -> u64 {
    120
}
fn default_failover_after() -> u64 {
    300
}
pub const DEFAULT_WEB_PORT: u16 = 7630;
fn default_web_port() -> u16 {
    DEFAULT_WEB_PORT
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            interval: default_interval(),
            failover_after: default_failover_after(),
            web_port: default_web_port(),
            prune: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Machine {
    /// the name its agent is reached by — a tailnet (MagicDNS) name or address
    pub host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Account {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub zones: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TunnelDecl {
    pub id: String,
    pub account: String,
    /// the machine whose agent runs this tunnel; none for a tunnel run
    /// outside the fleet
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// delete this tunnel in Cloudflare at the next `apply --allow-destroy`
    #[serde(default, skip_serializing_if = "is_false")]
    pub destroy: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Failover {
    /// say so, and wait for `tunnels promote`
    #[default]
    Manual,
    /// the standby's agent moves DNS once the primary has been down long enough
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Route {
    pub host: String,
    pub tunnel: String,
    pub service: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standby: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover: Option<Failover>,
    /// "standby" once promoted; traffic goes to the standby until `failback`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Route {
    pub fn on_standby(&self) -> bool {
        self.active.as_deref() == Some("standby") && self.standby.is_some()
    }

    /// The tunnel traffic for this host should reach right now.
    pub fn active_tunnel(&self) -> &str {
        if self.on_standby() { self.standby.as_deref().unwrap() } else { &self.tunnel }
    }
}

/// `3000` → `http://localhost:3000`; anything else is taken as written.
pub fn normalize_service(input: &str) -> String {
    if input.parse::<u16>().is_ok() { format!("http://localhost:{input}") } else { input.to_string() }
}

/// The local port a service points at, if it is on this machine.
pub fn local_port(service: &str) -> Option<u16> {
    let rest = service.split("://").nth(1)?;
    let hostport = rest.split('/').next()?;
    let (host, port) = hostport.rsplit_once(':')?;
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0") { port.parse().ok() } else { None }
}

/// Which fleet machine this is: `TUNNELS_MACHINE`, else the name this Mac's
/// config was given, else whichever fleet machine has this hostname, else
/// the hostname itself.
pub fn this_machine(config: &crate::config::Config, fleet: Option<&Fleet>) -> String {
    if let Ok(m) = std::env::var("TUNNELS_MACHINE") {
        if !m.is_empty() {
            return m;
        }
    }
    if let Some(m) = &config.machine {
        return m.clone();
    }
    let h = util::short_hostname();
    if let Some(f) = fleet {
        if let Some(n) = f.find_machine(&h) {
            return n.clone();
        }
    }
    h
}

pub fn looks_like_id(s: &str) -> bool {
    s.len() == 36 && s.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit())
}

impl Default for Fleet {
    fn default() -> Self {
        Fleet {
            serial: 0,
            updated_at: String::new(),
            updated_by: String::new(),
            policy: Policy::default(),
            machines: BTreeMap::new(),
            accounts: BTreeMap::new(),
            tunnels: BTreeMap::new(),
            routes: Vec::new(),
        }
    }
}

const HEADER: &str = "\
# The fleet: every tunnel, which machine runs it, and where every hostname goes.
# Written by `tunnels`; safe to edit by hand — run `tunnels fleet validate` after,
# then `tunnels plan` to see what would change. Holds no secrets. Every agent
# keeps a copy and the highest `serial` wins, so bump it if you edit by hand
# (`tunnels fleet edit` does that for you). Use `note = \"…\"` rather than
# comments: comments do not survive the next write.
";

impl Fleet {
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("TUNNELS_FLEET") {
            return PathBuf::from(p);
        }
        crate::config::Config::dir().join("fleet.toml")
    }

    fn history_dir() -> PathBuf {
        let p = Self::path();
        p.with_file_name("fleet.history")
    }

    pub fn parse(text: &str) -> Result<Fleet> {
        let f: Fleet = toml::from_str(text).context("parsing the fleet file")?;
        Ok(f)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!("{HEADER}\n{body}")
    }

    /// The fleet file, or `None` if this machine has never had one.
    pub fn load() -> Result<Option<Fleet>> {
        let path = Self::path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Some(Self::parse(&text).with_context(|| format!("in {}", path.display()))?))
    }

    pub fn load_required() -> Result<Fleet> {
        Self::load()?.ok_or_else(|| {
            anyhow::anyhow!(
                "no fleet file at {} — create one from what exists with `tunnels import`, \
                 or fetch one from a machine that has it with `tunnels fleet join <host>`",
                Self::path().display()
            )
        })
    }

    /// Write this copy to disk, keeping the one it replaces in the history.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Ok(old) = std::fs::read_to_string(&path) {
            if let Ok(prev) = Self::parse(&old) {
                let dir = Self::history_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(format!("{:06}.toml", prev.serial)), old);
                Self::trim_history(&dir, 50);
            }
        }
        util::write_atomic(&path, self.to_toml().as_bytes(), 0o644)
    }

    fn trim_history(dir: &std::path::Path, keep: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        if files.len() > keep {
            for f in &files[..files.len() - keep] {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    /// Change the fleet: read what is on disk now, apply the change, bump the
    /// serial, check it, write it. Reading first means two edits in a row
    /// from different processes compose instead of the second erasing the
    /// first — the lesson of the config file, applied here too.
    pub fn edit(machine: &str, change: impl FnOnce(&mut Fleet) -> Result<()>) -> Result<Fleet> {
        let mut f = Self::load()?.unwrap_or_default();
        change(&mut f)?;
        f.serial += 1;
        f.updated_at = util::now_rfc3339();
        f.updated_by = machine.to_string();
        let problems = f.validate();
        if !problems.is_empty() {
            bail!("the fleet would not be valid:\n  {}", problems.join("\n  "));
        }
        f.save()?;
        Ok(f)
    }

    /// Is `self` a later version than `other`? Serial first; on a tie (two
    /// machines edited the same version at once) the later timestamp, then
    /// the text itself, so every machine picks the same winner.
    pub fn newer_than(&self, other: &Fleet) -> bool {
        (self.serial, &self.updated_at, self.to_toml()) > (other.serial, &other.updated_at, other.to_toml())
    }

    /// Everything wrong with this file, as sentences. Empty means valid.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen_alias: BTreeMap<String, &str> = BTreeMap::new();
        let mut seen_id: BTreeMap<String, &str> = BTreeMap::new();
        for (alias, t) in &self.tunnels {
            if let Some(prev) = seen_alias.insert(alias.to_ascii_lowercase(), alias) {
                out.push(format!("tunnels `{prev}` and `{alias}` differ only in case"));
            }
            if !looks_like_id(&t.id) {
                out.push(format!("tunnel `{alias}`: id `{}` is not a Cloudflare tunnel id", t.id));
            }
            if let Some(prev) = seen_id.insert(t.id.to_ascii_lowercase(), alias) {
                out.push(format!("tunnels `{prev}` and `{alias}` are the same Cloudflare tunnel"));
            }
            if !self.accounts.contains_key(&t.account) {
                out.push(format!("tunnel `{alias}`: no account called `{}`", t.account));
            }
            if let Some(m) = &t.machine {
                if !self.machines.contains_key(m) {
                    out.push(format!("tunnel `{alias}`: no machine called `{m}`"));
                }
            }
        }
        let mut hosts: BTreeMap<String, ()> = BTreeMap::new();
        for r in &self.routes {
            let h = r.host.to_ascii_lowercase();
            if hosts.insert(h.clone(), ()).is_some() {
                out.push(format!("`{}` is routed twice — a hostname goes to one place", r.host));
            }
            if r.host.is_empty() || r.host.contains(char::is_whitespace) {
                out.push(format!("`{}` is not a hostname", r.host));
            }
            match self.tunnels.get(&r.tunnel) {
                None => out.push(format!("`{}`: no tunnel called `{}`", r.host, r.tunnel)),
                Some(t) if t.destroy => {
                    out.push(format!("`{}` is routed to `{}`, which is marked for destruction", r.host, r.tunnel))
                }
                _ => {}
            }
            if let Some(s) = &r.standby {
                if !self.tunnels.contains_key(s) {
                    out.push(format!("`{}`: no standby tunnel called `{s}`", r.host));
                }
                if s == &r.tunnel {
                    out.push(format!("`{}`: its standby is its own tunnel", r.host));
                }
            }
            if r.failover.is_some() && r.standby.is_none() {
                out.push(format!("`{}`: failover is set but there is no standby", r.host));
            }
            if let Some(a) = &r.active {
                if a != "standby" && a != "primary" {
                    out.push(format!("`{}`: active must be \"standby\" or \"primary\", not `{a}`", r.host));
                }
            }
            if !r.service.contains("://") && !r.service.starts_with("http_status:") {
                out.push(format!("`{}`: service `{}` should be a URL like http://localhost:3000", r.host, r.service));
            }
            if local_port(&r.service) == Some(self.policy.web_port) {
                out.push(format!("`{}` would publish the tunnels web UI (port {}) to the internet", r.host, self.policy.web_port));
            }
            if self.account_for_host(&r.host).is_none() && !self.accounts.is_empty() {
                out.push(format!("`{}` is in no zone of any account in this file", r.host));
            }
            // DNS for a host lives in its zone's account, and a tunnel CNAME
            // only works to a tunnel in that same account. Found the hard way:
            // staging-admin was aimed at a tunnel in the other account and
            // the error blamed token permissions.
            if let (Some((za, _)), Some(t)) = (self.account_for_host(&r.host), self.tunnels.get(r.active_tunnel())) {
                if za != &t.account {
                    out.push(format!(
                        "`{}` is in account `{za}` but tunnel `{}` is in `{}` — a hostname can only go to a tunnel in its own zone's account",
                        r.host,
                        r.active_tunnel(),
                        t.account
                    ));
                }
            }
        }
        out
    }

    /// The zone's account for a hostname: the account holding the longest
    /// zone the host ends with.
    pub fn account_for_host(&self, host: &str) -> Option<(&String, &Account)> {
        let mut best: Option<(&String, &Account, usize)> = None;
        for (alias, a) in &self.accounts {
            if let Some(z) = crate::cf::zone_for(host, a.zones.iter().map(|s| s.as_str())) {
                if best.map(|b| z.len() > b.2).unwrap_or(true) {
                    best = Some((alias, a, z.len()));
                }
            }
        }
        best.map(|(a, b, _)| (a, b))
    }

    pub fn account_alias_for_id(&self, id: &str) -> Option<&String> {
        self.accounts.iter().find(|(_, a)| a.id == id).map(|(k, _)| k)
    }

    /// A tunnel by alias (any case), full id, or an id prefix of 8 or more.
    pub fn find_tunnel(&self, key: &str) -> Option<(&String, &TunnelDecl)> {
        if let Some(t) = self.tunnels.get_key_value(key) {
            return Some(t);
        }
        let lower = key.to_ascii_lowercase();
        if let Some(t) = self.tunnels.iter().find(|(a, _)| a.to_ascii_lowercase() == lower) {
            return Some(t);
        }
        if let Some(t) = self.tunnels.iter().find(|(_, t)| t.id.eq_ignore_ascii_case(key)) {
            return Some(t);
        }
        if key.len() >= 8 {
            let hits: Vec<_> = self.tunnels.iter().filter(|(_, t)| t.id.starts_with(&lower)).collect();
            if hits.len() == 1 {
                return Some(hits[0]);
            }
        }
        None
    }

    pub fn alias_for_id(&self, id: &str) -> Option<&String> {
        self.tunnels.iter().find(|(_, t)| t.id.eq_ignore_ascii_case(id)).map(|(a, _)| a)
    }

    pub fn find_route(&self, host: &str) -> Option<&Route> {
        self.routes.iter().find(|r| r.host.eq_ignore_ascii_case(host))
    }

    pub fn find_route_mut(&mut self, host: &str) -> Option<&mut Route> {
        self.routes.iter_mut().find(|r| r.host.eq_ignore_ascii_case(host))
    }

    /// A machine by its fleet name or its host, any case.
    pub fn find_machine(&self, key: &str) -> Option<&String> {
        let k = key.to_ascii_lowercase();
        self.machines
            .iter()
            .find(|(n, m)| n.to_ascii_lowercase() == k || m.host.to_ascii_lowercase() == k)
            .map(|(n, _)| n)
    }

    /// Which machine runs this tunnel, if a fleet machine does.
    pub fn machine_of(&self, tunnel_alias: &str) -> Option<&str> {
        self.tunnels.get(tunnel_alias).and_then(|t| t.machine.as_deref())
    }

    /// The machine responsible for a route's ingress on `tunnel_alias` and,
    /// when that tunnel is the active one, its DNS. One owner per thing, so
    /// two agents never undo each other.
    pub fn owner_of_tunnel(&self, tunnel_alias: &str) -> Option<&str> {
        self.machine_of(tunnel_alias)
    }

    /// Rename a tunnel alias everywhere it is used.
    pub fn rename_tunnel(&mut self, old: &str, new: &str) -> Result<()> {
        if self.tunnels.contains_key(new) {
            bail!("there is already a tunnel called `{new}`");
        }
        let t = self.tunnels.remove(old).ok_or_else(|| anyhow::anyhow!("no tunnel called `{old}`"))?;
        self.tunnels.insert(new.to_string(), t);
        for r in &mut self.routes {
            if r.tunnel == old {
                r.tunnel = new.to_string();
            }
            if r.standby.as_deref() == Some(old) {
                r.standby = Some(new.to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Fleet {
        Fleet::parse(
            r#"
serial = 3
[machines.dr1]
host = "dorkyrobot1"
[machines.dr2]
host = "dorkyrobot2"
[accounts.vet]
id = "acct-vet"
zones = ["everyday.vet"]
[accounts.home]
id = "acct-home"
zones = ["felixflor.es"]
[tunnels.vet-prod]
id = "6da56f03-7687-46a4-b023-7557decbc04b"
account = "vet"
machine = "dr1"
[tunnels.vet-standby]
id = "51e97a99-f3d8-4da9-9a7f-2fd2a0567776"
account = "vet"
machine = "dr2"
[tunnels.dr2-home]
id = "bc598bf7-c3a7-4b4c-aa94-4ac056aa87bc"
account = "home"
machine = "dr2"
[[routes]]
host = "admin.everyday.vet"
tunnel = "vet-prod"
service = "http://localhost:3300"
standby = "vet-standby"
[[routes]]
host = "media.felixflor.es"
tunnel = "dr2-home"
service = "http://localhost:2283"
"#,
        )
        .unwrap()
    }

    #[test]
    fn the_sample_is_valid() {
        assert_eq!(sample().validate(), Vec::<String>::new());
    }

    #[test]
    fn round_trips_through_toml() {
        let f = sample();
        let again = Fleet::parse(&f.to_toml()).unwrap();
        assert_eq!(f, again);
    }

    #[test]
    fn tunnels_are_found_by_alias_any_case_or_id() {
        let f = sample();
        assert_eq!(f.find_tunnel("VET-PROD").unwrap().0, "vet-prod");
        assert_eq!(f.find_tunnel("6da56f03-7687-46a4-b023-7557decbc04b").unwrap().0, "vet-prod");
        assert_eq!(f.find_tunnel("6da56f03").unwrap().0, "vet-prod");
        assert!(f.find_tunnel("6da5").is_none(), "too short a prefix to trust");
        assert!(f.find_tunnel("nope").is_none());
    }

    #[test]
    fn a_hostname_routed_to_a_tunnel_in_the_other_account_is_refused() {
        // the staging-admin case: zone in one account, tunnel in the other
        let mut f = sample();
        f.routes.push(Route {
            host: "staging-admin.everyday.vet".into(),
            tunnel: "dr2-home".into(),
            service: "http://localhost:3312".into(),
            ..Default::default()
        });
        let problems = f.validate();
        assert!(problems.iter().any(|p| p.contains("its own zone's account")), "{problems:?}");
    }

    #[test]
    fn the_web_ui_is_never_a_route() {
        let mut f = sample();
        f.routes.push(Route {
            host: "ui.felixflor.es".into(),
            tunnel: "dr2-home".into(),
            service: format!("http://localhost:{DEFAULT_WEB_PORT}"),
            ..Default::default()
        });
        assert!(f.validate().iter().any(|p| p.contains("publish the tunnels web UI")));
    }

    #[test]
    fn a_hostname_goes_to_one_place() {
        let mut f = sample();
        f.routes.push(Route {
            host: "Media.felixflor.es".into(),
            tunnel: "dr2-home".into(),
            service: "http://localhost:1".into(),
            ..Default::default()
        });
        assert!(f.validate().iter().any(|p| p.contains("routed twice")));
    }

    #[test]
    fn promotion_moves_the_active_tunnel() {
        let mut f = sample();
        assert_eq!(f.find_route("admin.everyday.vet").unwrap().active_tunnel(), "vet-prod");
        f.find_route_mut("admin.everyday.vet").unwrap().active = Some("standby".into());
        assert_eq!(f.find_route("admin.everyday.vet").unwrap().active_tunnel(), "vet-standby");
    }

    #[test]
    fn the_newest_copy_wins_the_same_way_everywhere() {
        let a = sample();
        let mut b = sample();
        b.serial += 1;
        assert!(b.newer_than(&a) && !a.newer_than(&b));
        // same serial, edited on two machines at once: a stable tiebreak
        let mut c = sample();
        c.updated_at = "2026-09-23T20:00:00Z".into();
        let mut d = sample();
        d.updated_at = "2026-09-23T21:00:00Z".into();
        assert!(d.newer_than(&c) && !c.newer_than(&d));
    }

    #[test]
    fn renaming_a_tunnel_follows_its_routes() {
        let mut f = sample();
        f.rename_tunnel("vet-standby", "vet-warm").unwrap();
        assert_eq!(f.find_route("admin.everyday.vet").unwrap().standby.as_deref(), Some("vet-warm"));
        assert!(f.validate().is_empty());
    }

    #[test]
    fn services() {
        assert_eq!(normalize_service("3000"), "http://localhost:3000");
        assert_eq!(normalize_service("ssh://localhost:22"), "ssh://localhost:22");
        assert_eq!(local_port("http://localhost:3000"), Some(3000));
        assert_eq!(local_port("http://127.0.0.1:8080/path"), Some(8080));
        assert_eq!(local_port("ssh://localhost:22"), Some(22));
        assert_eq!(local_port("http://nas.lan:5000"), None);
        assert_eq!(local_port("http_status:404"), None);
    }
}
