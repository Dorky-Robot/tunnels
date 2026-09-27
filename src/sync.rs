//! Keeping every machine's copy of the fleet file current, over the tailnet.
//!
//! Each agent serves its copy at `/api/fleet`. Anyone holding an older copy
//! takes the newest one it can find, so an edit made on any machine reaches
//! the others within one agent pass — and a machine that was asleep for a
//! week catches up from whichever peer is awake. No central server, no
//! GitHub, nothing outside the mesh that has to be up.
//!
//! Trust boundary: the tailnet, and then `policy.remote_from`. Agents only
//! answer tailnet and loopback addresses; a copy is only taken from a
//! machine the copy we already hold trusts, and only if it parses and
//! validates. A machine off the list follows but is never followed.

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

/// The hosts a copy may be taken from: the machines the copy we already
/// hold trusts (`Fleet::trusts`, i.e. `policy.remote_from`). Judging by our
/// own copy means a new copy cannot put its sender on the list and so admit
/// itself. A machine off the list still takes from these; nobody takes from
/// it (a laptop that travels should not re-point the mesh's hostnames).
///
/// A copy's sender is the host we dialed, named in our own fleet — the same
/// host-to-machine identity `/api/cf-forward` checks the other way round.
/// Nothing the sender says about itself counts, and `/api/notify` only
/// hurries a pass: it no longer names hosts to pull from, or any tailnet
/// machine could have its copy taken by pointing us at itself.
pub fn trusted_hosts(fleet: &Fleet, me: &str) -> Vec<String> {
    fleet
        .machines
        .iter()
        .filter(|(name, _)| name.as_str() != me && fleet.trusts(name))
        .map(|(_, m)| m.host.clone())
        .filter(|h| !h.is_empty())
        .collect()
}

/// Of the copies fetched, the newest one from a trusted host that is newer
/// than `current`. Pure, so the rule is tested without a network.
pub fn pick(current: &Fleet, fetched: Vec<(String, Fleet)>) -> Option<(String, Fleet)> {
    let mut best: Option<(String, Fleet)> = None;
    for (host, f) in fetched {
        let trusted = current.machine_at(&host).is_some_and(|m| current.trusts(m));
        let beats_current = f.newer_than(current);
        let beats_best = best.as_ref().map(|(_, b)| f.newer_than(b)).unwrap_or(true);
        if trusted && beats_current && beats_best {
            best = Some((host, f));
        }
    }
    best
}

/// The newest copy among the trusted peers, if one is newer than `fleet`.
pub fn newest_from_peers(fleet: &Fleet, me: &str, timeout: Duration) -> Option<(String, Fleet)> {
    let port = fleet.policy.web_port;
    let handles: Vec<_> = trusted_hosts(fleet, me)
        .into_iter()
        .map(|h| std::thread::spawn(move || (h.clone(), fetch(&h, port, timeout))))
        .collect();
    let fetched = handles.into_iter().filter_map(|h| h.join().ok()).filter_map(|(h, r)| r.ok().map(|f| (h, f))).collect();
    pick(fleet, fetched)
}

/// Bring this machine's copy up to date. Returns where a newer copy came from.
///
/// Two machines that edit the same version at once both write the next
/// serial, and one edit would simply lose — which happened on the first
/// rollout, when mini and mac2024 imported seconds apart and mini vanished
/// from the fleet. So when the copy being taken has the *same* serial as
/// ours, the two are merged instead: everything ours has that theirs lacks
/// is added, and the result is a new serial that every machine then takes.
///
/// Only a machine the fleet trusts merges: a merge is an edit, and a machine
/// off `remote_from` does not edit the fleet (see `Fleet::edit`); it takes
/// theirs.
pub fn pull(me: &str, timeout: Duration) -> Result<Option<(String, u64)>> {
    let Some(current) = Fleet::load()? else { return Ok(None) };
    match newest_from_peers(&current, me, timeout) {
        Some((host, f)) => {
            if f.serial == current.serial && current.trusts(me) {
                let mut merged = f.clone();
                let added = merged.absorb(&current);
                if !added.is_empty() {
                    merged.serial += 1;
                    merged.updated_at = crate::util::now_rfc3339();
                    merged.updated_by = me.to_string();
                    // a merge that does not validate is dropped: theirs is taken, as before
                    if merged.validate().is_empty() {
                        merged.save()?;
                        notify(&merged, me);
                        return Ok(Some((format!("{host}, merged with a concurrent edit here ({})", added.join(", ")), merged.serial)));
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::tests::sample;

    fn later(f: &Fleet, serial: u64, remote_from: Option<Vec<&str>>) -> Fleet {
        let mut g = f.clone();
        g.serial = serial;
        g.policy.remote_from = remote_from.map(|l| l.into_iter().map(String::from).collect());
        g
    }

    #[test]
    fn only_trusted_hosts_are_asked_and_only_their_copies_taken() {
        let mut current = sample();
        current.machines.insert("sara".into(), crate::fleet::Machine { host: "saras-mac".into(), ..Default::default() });
        current.policy.remote_from = Some(vec!["dr1".into(), "dr2".into()]);
        assert_eq!(trusted_hosts(&current, "dr2"), vec!["dorkyrobot1"]);
        assert_eq!(trusted_hosts(&current, "sara"), vec!["dorkyrobot1", "dorkyrobot2"], "off the list, it still follows");

        let trusted = later(&current, 9, Some(vec!["dr1", "dr2"]));
        let hers = later(&current, 20, Some(vec!["dr1", "dr2", "sara"]));
        let unknown = later(&current, 30, None);
        let got = pick(&current, vec![("saras-mac".into(), hers), ("dorkyrobot1".into(), trusted), ("elsewhere".into(), unknown)]);
        assert_eq!(got.map(|(h, f)| (h, f.serial)), Some(("dorkyrobot1".into(), 9)));
    }

    #[test]
    fn with_no_list_every_fleet_machine_is_a_source_as_before() {
        let current = sample();
        assert_eq!(trusted_hosts(&current, "dr2"), vec!["dorkyrobot1"]);
        let got = pick(&current, vec![("dorkyrobot1".into(), later(&current, 4, None)), ("not-in-the-fleet".into(), later(&current, 8, None))]);
        assert_eq!(got.map(|(_, f)| f.serial), Some(4), "a host the fleet does not name is nobody");
    }
}
