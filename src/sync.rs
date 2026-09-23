//! Keeping every machine's copy of the fleet file current, over the tailnet.
//!
//! Each agent serves its copy at `/api/fleet`. Anyone holding an older copy
//! takes the newest one it can find, so an edit made on any machine reaches
//! the others within one agent pass — and a machine that was asleep for a
//! week catches up from whichever peer is awake. No central server, no
//! GitHub, nothing outside the mesh that has to be up.
//!
//! Trust boundary: the tailnet. Agents only answer tailnet and loopback
//! addresses, and a copy is only adopted if it parses and validates.

use crate::fleet::Fleet;
use anyhow::{Context, Result, anyhow};
use std::time::Duration;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(true).build().into()
}

/// Fetch the fleet file a peer's agent holds.
pub fn fetch(host: &str, port: u16, timeout: Duration) -> Result<Fleet> {
    let url = format!("http://{host}:{port}/api/fleet");
    let text = agent(timeout)
        .get(&url)
        .call()
        .with_context(|| format!("asking {host} for its fleet file"))?
        .body_mut()
        .read_to_string()?;
    let f = Fleet::parse(&text)?;
    let problems = f.validate();
    if !problems.is_empty() {
        return Err(anyhow!("{host}'s fleet file is not valid: {}", problems.join("; ")));
    }
    Ok(f)
}

/// The newest copy among the peers (every machine in `fleet` but `me`, plus
/// any `extra` hosts), if one is newer than `fleet`.
pub fn newest_from_peers(fleet: &Fleet, me: &str, extra: &[String], timeout: Duration) -> Option<(String, Fleet)> {
    let port = fleet.policy.web_port;
    let mut hosts: Vec<String> = fleet
        .machines
        .iter()
        .filter(|(name, _)| name.as_str() != me)
        .map(|(_, m)| m.host.clone())
        .filter(|h| !h.is_empty())
        .collect();
    for h in extra {
        if !hosts.contains(h) {
            hosts.push(h.clone());
        }
    }
    let handles: Vec<_> = hosts
        .into_iter()
        .map(|h| std::thread::spawn(move || (h.clone(), fetch(&h, port, timeout))))
        .collect();
    let mut best: Option<(String, Fleet)> = None;
    for h in handles {
        let Ok((host, Ok(f))) = h.join() else { continue };
        let beats_current = f.newer_than(fleet);
        let beats_best = best.as_ref().map(|(_, b)| f.newer_than(b)).unwrap_or(true);
        if beats_current && beats_best {
            best = Some((host, f));
        }
    }
    best
}

/// Bring this machine's copy up to date. Returns where a newer copy came from.
pub fn pull(me: &str, extra: &[String], timeout: Duration) -> Result<Option<(String, u64)>> {
    let Some(current) = Fleet::load()? else { return Ok(None) };
    match newest_from_peers(&current, me, extra, timeout) {
        Some((host, f)) => {
            f.save()?;
            Ok(Some((host, f.serial)))
        }
        None => Ok(None),
    }
}

/// Tell the other agents there is a new copy here, so they pull now rather
/// than at their next pass. Best effort: a peer that is down catches up later.
pub fn notify(fleet: &Fleet, me: &str) {
    let port = fleet.policy.web_port;
    let my_host = fleet.machines.get(me).map(|m| m.host.clone()).unwrap_or_else(crate::util::short_hostname);
    let handles: Vec<_> = fleet
        .machines
        .iter()
        .filter(|(n, _)| n.as_str() != me)
        .map(|(_, m)| m.host.clone())
        .map(|h| {
            let my_host = my_host.clone();
            std::thread::spawn(move || {
                let _ = agent(Duration::from_secs(3))
                    .post(&format!("http://{h}:{port}/api/notify"))
                    .header("X-Tunnels", "1")
                    .send_json(serde_json::json!({ "from": my_host }));
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    // and our own agent, which serves the copy the others will fetch
    let _ = agent(Duration::from_secs(2))
        .post(&format!("http://127.0.0.1:{port}/api/notify"))
        .header("X-Tunnels", "1")
        .send_json(serde_json::json!({}));
}
