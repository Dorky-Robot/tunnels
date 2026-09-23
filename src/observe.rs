//! What is actually out there: every tunnel, its connectors, its ingress,
//! and every DNS record that points at a tunnel — across every Cloudflare
//! account a token here can reach — plus what this Mac is running.
//!
//! The questions agents kept writing Python to answer ("where did things
//! land", "which tunnel does this hostname really go to") are answered from
//! one of these.

use crate::cf::{self, Client, DnsRecord, Ingress, Tunnel, Zone};
use crate::config::Config;
use crate::launchd;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct AccountObs {
    pub id: String,
    pub name: String,
    /// an API token here can see this account's tunnels
    pub reachable: bool,
    pub zones: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelObs {
    pub account_id: String,
    #[serde(flatten)]
    pub tunnel: Tunnel,
    /// `None`: not fetched, or the fetch failed — unknown, not empty
    #[serde(skip)]
    pub ingress: Option<Vec<Ingress>>,
}

impl TunnelObs {
    pub fn up(&self) -> bool {
        !self.tunnel.connections.is_empty()
    }

    pub fn routes(&self) -> Vec<(String, String)> {
        self.ingress
            .as_ref()
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| r.hostname.as_ref().map(|h| (h.to_ascii_lowercase(), r.service.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_host(&self, host: &str) -> bool {
        self.ingress
            .as_ref()
            .map(|rs| rs.iter().any(|r| r.hostname.as_deref().map(|h| h.eq_ignore_ascii_case(host)).unwrap_or(false)))
            .unwrap_or(false)
    }

    pub fn service_for(&self, host: &str) -> Option<&str> {
        self.ingress.as_ref()?.iter().find_map(|r| {
            r.hostname.as_deref().filter(|h| h.eq_ignore_ascii_case(host)).map(|_| r.service.as_str())
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalTunnel {
    /// the name in this Mac's config and in its LaunchAgent label
    pub name: String,
    pub tunnel_id: Option<String>,
    pub account_id: Option<String>,
    pub state: String,
    pub pid: Option<u32>,
    pub label: String,
    /// the plist still has the token inline, not in a token file
    pub inline_token: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LocalObs {
    pub machine: String,
    pub tunnels: Vec<LocalTunnel>,
    /// cloudflared LaunchAgents here that the config does not know about
    pub stray_plists: Vec<String>,
    pub agent_loaded: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Snapshot {
    pub taken_at: String,
    pub accounts: Vec<AccountObs>,
    pub tunnels: Vec<TunnelObs>,
    /// tunnel CNAMEs in every zone looked at, plus any record at a declared host
    pub dns: Vec<DnsRecord>,
    pub zones: Vec<Zone>,
    /// zones whose DNS was actually listed (a host in any other zone is unknown, not missing)
    pub dns_zones_seen: BTreeSet<String>,
    pub errors: Vec<String>,
    #[serde(skip)]
    pub token_for_account: BTreeMap<String, String>,
}

/// How much to look at. The agent looks only at what it owns, to stay well
/// inside Cloudflare's rate limits; `status` and the web UI look at everything.
#[derive(Debug, Clone, Default)]
pub struct Want {
    /// fetch ingress for these tunnel ids; `None` = every tunnel
    pub ingress_for: Option<BTreeSet<String>>,
    /// list DNS in these zones (by name); `None` = every zone
    pub dns_zones: Option<BTreeSet<String>>,
    /// only these accounts; `None` = every reachable one
    pub accounts: Option<BTreeSet<String>>,
}

impl Snapshot {
    pub fn tunnel(&self, id: &str) -> Option<&TunnelObs> {
        self.tunnels.iter().find(|t| t.tunnel.id.eq_ignore_ascii_case(id))
    }

    pub fn dns_for(&self, host: &str) -> Vec<&DnsRecord> {
        self.dns.iter().filter(|r| r.name.eq_ignore_ascii_case(host)).collect()
    }

    pub fn zone_for_host(&self, host: &str) -> Option<&Zone> {
        let name = cf::zone_for(host, self.zones.iter().map(|z| z.name.as_str()))?;
        self.zones.iter().find(|z| z.name == name)
    }

    pub fn dns_known_for(&self, host: &str) -> bool {
        self.zone_for_host(host).map(|z| self.dns_zones_seen.contains(&z.name)).unwrap_or(false)
    }

    pub fn account_reachable(&self, id: &str) -> bool {
        self.accounts.iter().any(|a| a.id == id && a.reachable)
    }

    pub fn client_for_account(&self, account_id: &str) -> Option<Client> {
        self.token_for_account.get(account_id).map(|t| Client::new(t))
    }

    pub fn client_for_zone(&self, zone_id: &str) -> Option<Client> {
        let z = self.zones.iter().find(|z| z.id == zone_id)?;
        self.client_for_account(&z.account_id)
    }
}

/// Look at Cloudflare through every API token in `config`.
pub fn observe(config: &Config, want: &Want) -> Snapshot {
    let mut snap = Snapshot { taken_at: crate::util::now_rfc3339(), ..Default::default() };
    let tokens: Vec<String> = config.all_cf_api_tokens().into_iter().map(String::from).collect();
    if tokens.is_empty() {
        snap.errors.push("no Cloudflare API token on this machine — add one with `tunnels token add`".into());
    }

    // accounts and zones, and which token reaches each account
    let mut accounts: BTreeMap<String, AccountObs> = BTreeMap::new();
    for tok in &tokens {
        let c = Client::new(tok);
        let mut ids: Vec<(String, String)> = Vec::new();
        match c.zones() {
            Ok(zs) => {
                for z in zs {
                    ids.push((z.account_id.clone(), z.account_name.clone()));
                    let a = accounts.entry(z.account_id.clone()).or_insert_with(|| AccountObs {
                        id: z.account_id.clone(),
                        name: z.account_name.clone(),
                        reachable: false,
                        zones: Vec::new(),
                    });
                    if !a.zones.contains(&z.name) {
                        a.zones.push(z.name.clone());
                    }
                    if !snap.zones.iter().any(|x| x.id == z.id) {
                        snap.zones.push(z.clone());
                    }
                    snap.token_for_account.entry(z.account_id.clone()).or_insert_with(|| tok.clone());
                }
            }
            Err(e) => snap.errors.push(format!("listing zones with token {}: {e}", crate::config::hint(tok))),
        }
        // accounts the token can list even without zones in them
        if let Ok(accts) = c.accounts() {
            for a in accts {
                ids.push((a.id.clone(), a.name.clone()));
                accounts.entry(a.id.clone()).or_insert_with(|| AccountObs {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    reachable: false,
                    zones: Vec::new(),
                });
                snap.token_for_account.entry(a.id.clone()).or_insert_with(|| tok.clone());
            }
        }
    }
    // accounts named by local connector tokens, even if no API token reaches them
    for t in &config.tunnels {
        if let Some(a) = t.account_id() {
            accounts.entry(a.clone()).or_insert_with(|| AccountObs {
                id: a.clone(),
                name: String::new(),
                reachable: false,
                zones: Vec::new(),
            });
        }
    }

    for (id, acct) in accounts.iter_mut() {
        if let Some(only) = &want.accounts {
            if !only.contains(id) {
                continue;
            }
        }
        // try each token until one can list this account's tunnels
        let mut listed = None;
        let preferred = snap.token_for_account.get(id).cloned();
        for tok in preferred.iter().chain(tokens.iter()) {
            match Client::new(tok).tunnels(id) {
                Ok(ts) => {
                    listed = Some(ts);
                    snap.token_for_account.insert(id.clone(), tok.clone());
                    break;
                }
                Err(_) => continue,
            }
        }
        let Some(ts) = listed else {
            snap.token_for_account.remove(id);
            continue;
        };
        acct.reachable = true;
        let client = Client::new(&snap.token_for_account[id]);
        for t in ts {
            let fetch = want.ingress_for.as_ref().map(|s| s.contains(&t.id)).unwrap_or(true);
            let ingress = if fetch {
                match client.ingress(id, &t.id) {
                    Ok(i) => Some(i),
                    Err(e) => {
                        snap.errors.push(format!("ingress for tunnel {} ({}): {e}", t.name, &t.id[..8.min(t.id.len())]));
                        None
                    }
                }
            } else {
                None
            };
            snap.tunnels.push(TunnelObs { account_id: id.clone(), tunnel: t, ingress });
        }
    }
    for a in accounts.values_mut() {
        a.zones.sort();
    }
    snap.accounts = accounts.into_values().collect();

    // DNS
    for z in snap.zones.clone() {
        if let Some(only) = &want.dns_zones {
            if !only.contains(&z.name) {
                continue;
            }
        }
        let Some(tok) = snap.token_for_account.get(&z.account_id).cloned() else { continue };
        match Client::new(&tok).dns_records(&z.id) {
            Ok(rs) => {
                snap.dns.extend(rs);
                snap.dns_zones_seen.insert(z.name.clone());
            }
            Err(e) => snap.errors.push(format!("DNS in {}: {e}", z.name)),
        }
    }
    snap
}

/// What this Mac runs, from its config and launchd.
pub fn observe_local(config: &Config, machine: &str) -> LocalObs {
    let mut out = LocalObs { machine: machine.to_string(), ..Default::default() };
    for t in &config.tunnels {
        let (state, pid) = match launchd::status(&t.name) {
            launchd::Status::Running { pid } => ("loaded", pid),
            launchd::Status::Stopped => ("not loaded", None),
            launchd::Status::Inactive => ("no plist", None),
        };
        out.tunnels.push(LocalTunnel {
            name: t.name.clone(),
            tunnel_id: t.tunnel_id(),
            account_id: t.account_id(),
            state: state.to_string(),
            pid,
            label: launchd::label_for(&t.name),
            inline_token: launchd::plist_has_inline_token(&t.name),
        });
    }
    for l in launchd::local_labels() {
        if !config.tunnels.iter().any(|t| t.name == l) {
            out.stray_plists.push(l);
        }
    }
    out.agent_loaded = launchd::agent_loaded();
    out
}
