//! The web UI and the agent's API — on the tailnet, and nowhere else.
//!
//! It binds only to this Mac's tailnet address and to loopback, never to
//! the LAN or the internet, and it refuses any request that arrives from
//! anywhere but those, in case the binding is ever widened by mistake.
//! Requests that change something need an `X-Tunnels: 1` header, which a
//! page on another site cannot send without a CORS preflight this server
//! never answers — so a browser tab elsewhere cannot drive it.

use crate::agent::{self, Shared};
use crate::apply::{self, Options};
use crate::config::Config;
use crate::fleet::{self, Fleet};
use crate::observe::{self, Want};
use crate::plan;
use crate::status::{self, FleetStatus};
use crate::{launchd, sync, util};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::io::Read;
use std::sync::Mutex;
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX: &str = include_str!("web/index.html");

static STATUS_CACHE: Mutex<Option<(i64, FleetStatus)>> = Mutex::new(None);

/// Start serving in the background: loopback now, the tailnet address as
/// soon as there is one (Tailscale may come up after the agent does).
pub fn spawn(shared: Shared, port: u16) {
    let s1 = shared.clone();
    std::thread::spawn(move || serve(s1, format!("127.0.0.1:{port}")));
    std::thread::spawn(move || {
        loop {
            if let Some(ip) = util::tailnet_ipv4() {
                serve(shared.clone(), format!("{ip}:{port}"));
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    });
}

fn serve(shared: Shared, addr: String) {
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{} web: cannot listen on {addr}: {e}", util::now_rfc3339());
            return;
        }
    };
    eprintln!("{} web: listening on http://{addr}", util::now_rfc3339());
    for req in server.incoming_requests() {
        let shared = shared.clone();
        std::thread::spawn(move || handle(shared, req));
    }
}

fn json_header() -> Header {
    Header::from_bytes("Content-Type", "application/json").unwrap()
}

fn reply_json(req: Request, code: u16, v: &impl serde::Serialize) {
    let body = serde_json::to_string(v).unwrap_or_else(|_| "{}".into());
    let _ = req.respond(Response::from_string(body).with_status_code(code).with_header(json_header()));
}

fn reply_err(req: Request, code: u16, msg: &str) {
    reply_json(req, code, &serde_json::json!({ "error": msg }));
}

fn handle(shared: Shared, mut req: Request) {
    let allowed = req.remote_addr().map(|a| util::is_tailnet_or_loopback(&a.ip())).unwrap_or(false);
    // cloudflared connects from localhost, so a route pointed at this port
    // would pass the address check and publish the UI to the internet.
    // Anything that came through a proxy carries these; nothing on the
    // tailnet does.
    let proxied = req.headers().iter().any(|h| {
        ["Cf-Ray", "Cf-Connecting-Ip", "X-Forwarded-For", "Forwarded", "Cf-Warp-Tag-Id"].iter().any(|n| h.field.equiv(*n))
    });
    if !allowed || proxied {
        let _ = req.respond(Response::from_string("tailnet only").with_status_code(403));
        return;
    }
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let method = req.method().clone();
    let is_action = method == Method::Post;
    if is_action && !req.headers().iter().any(|h| h.field.equiv("X-Tunnels") && h.value.as_str() == "1") {
        reply_err(req, 403, "missing X-Tunnels header");
        return;
    }
    let mut body = String::new();
    if is_action {
        let _ = req.as_reader().take(1 << 20).read_to_string(&mut body);
    }
    // everything below runs as this machine, which may touch both
    crate::scope::enter(crate::scope::Scope::LocalAndCloudflare);
    match (method, path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            let _ = req.respond(
                Response::from_string(INDEX)
                    .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
            );
        }
        (Method::Get, "/api/fleet") => match std::fs::read_to_string(Fleet::path()) {
            Ok(t) => {
                let _ = req.respond(
                    Response::from_string(t).with_header(Header::from_bytes("Content-Type", "text/plain; charset=utf-8").unwrap()),
                );
            }
            Err(_) => reply_err(req, 404, "no fleet file here"),
        },
        (Method::Get, "/api/health") => {
            let (m, _) = &*shared;
            let s = m.lock().unwrap();
            let serial = Fleet::load().ok().flatten().map(|f| f.serial);
            let v = serde_json::json!({
                "machine": s.machine, "version": env!("CARGO_PKG_VERSION"), "serial": serial,
                "started_at": s.started_at, "last_tick": s.last_tick,
            });
            drop(s);
            reply_json(req, 200, &v);
        }
        (Method::Get, "/api/agent") => {
            let (m, _) = &*shared;
            let s = m.lock().unwrap();
            let v = serde_json::json!({
                "machine": s.machine, "version": env!("CARGO_PKG_VERSION"), "started_at": s.started_at,
                "last_tick": s.last_tick, "events": s.events,
            });
            drop(s);
            reply_json(req, 200, &v);
        }
        (Method::Get, "/api/status") => {
            let fresh = query.contains("fresh=1");
            match full_status(fresh) {
                Ok(s) => reply_json(req, 200, &s),
                Err(e) => reply_err(req, 500, &format!("{e:#}")),
            }
        }
        (Method::Get, "/api/peers") => {
            let fleet = Fleet::load().ok().flatten().unwrap_or_default();
            let port = fleet.policy.web_port;
            let handles: Vec<_> = fleet
                .machines
                .iter()
                .map(|(n, m)| {
                    let (n, h) = (n.clone(), m.host.clone());
                    std::thread::spawn(move || {
                        let a: ureq::Agent = ureq::Agent::config_builder()
                            .timeout_global(Some(std::time::Duration::from_secs(3)))
                            .build()
                            .into();
                        let r = a
                            .get(&format!("http://{h}:{port}/api/health"))
                            .call()
                            .ok()
                            .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok());
                        serde_json::json!({ "machine": n, "host": h, "health": r })
                    })
                })
                .collect();
            let peers: Vec<serde_json::Value> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
            reply_json(req, 200, &peers);
        }
        (Method::Get, "/api/cf-log") => {
            // this machine's `tunnels cf` changes; with all=1, every peer's too,
            // gathered here so the browser only ever talks to one agent
            let mut recs: Vec<serde_json::Value> =
                crate::api::load_log(100).into_iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
            if query.contains("all=1") {
                let fleet = Fleet::load().ok().flatten().unwrap_or_default();
                let me = { let (m, _) = &*shared; m.lock().unwrap().machine.clone() };
                let port = fleet.policy.web_port;
                let handles: Vec<_> = fleet
                    .machines
                    .iter()
                    .filter(|(n, _)| **n != me)
                    .map(|(_, m)| {
                        let h = m.host.clone();
                        std::thread::spawn(move || {
                            let a: ureq::Agent = ureq::Agent::config_builder()
                                .timeout_global(Some(std::time::Duration::from_secs(3)))
                                .build()
                                .into();
                            a.get(&format!("http://{h}:{port}/api/cf-log"))
                                .call()
                                .ok()
                                .and_then(|mut r| r.body_mut().read_json::<Vec<serde_json::Value>>().ok())
                                .unwrap_or_default()
                        })
                    })
                    .collect();
                for h in handles {
                    recs.extend(h.join().unwrap_or_default());
                }
                recs.sort_by(|a, b| b["at"].as_str().unwrap_or("").cmp(a["at"].as_str().unwrap_or("")));
                recs.truncate(200);
            }
            reply_json(req, 200, &recs);
        }
        (Method::Get, "/api/logs") => {
            let name = query.split('&').find_map(|kv| kv.strip_prefix("tunnel=")).unwrap_or("");
            let name = name.replace("%20", " ");
            let config = Config::load().unwrap_or_default();
            if name == "agent" {
                let p = launchd::log_dir().join("agent.log");
                let text = std::fs::read_to_string(p).unwrap_or_default();
                let tail: Vec<&str> = text.lines().rev().take(300).collect();
                let _ = req.respond(Response::from_string(tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
            } else if config.tunnel_by_name(&name).is_some() {
                let text = launchd::read_logs(&name, 300).unwrap_or_default();
                let _ = req.respond(Response::from_string(text));
            } else {
                reply_err(req, 404, "no tunnel by that name runs here");
            }
        }
        (Method::Post, "/api/cf-forward") => {
            // a peer with no token for an account asks this machine to make a
            // `tunnels cf` call with ours. The token stays here; the change is
            // logged here, with who asked.
            let remote = req.remote_addr().map(|a| a.ip());
            let fleet = Fleet::load().ok().flatten().unwrap_or_default();
            let config = Config::load().unwrap_or_default();
            let me = { let (m, _) = &*shared; m.lock().unwrap().machine.clone() };
            let call: crate::api::Call = match serde_json::from_str(&body) {
                Ok(c) => c,
                Err(e) => return reply_err(req, 400, &format!("not a call: {e}")),
            };
            let Some(from) = call.requested_by.clone() else {
                return reply_err(req, 400, "who is asking? (requested_by)");
            };
            if let Err(why) = may_forward(&fleet, &from, remote) {
                return reply_err(req, 403, &why);
            }
            let dir = crate::api::directory(&config);
            if !crate::api::can_handle(call.path, call.account, &fleet, &dir, &config) {
                return reply_err(req, 409, "no token here for that");
            }
            let c = crate::api::Call { forward: false, ..call };
            match crate::api::call(c, &config, &fleet, &me) {
                Ok(out) => reply_json(req, 200, &out),
                Err(e) => reply_err(req, 400, &format!("{e:#}")),
            }
        }
        (Method::Post, "/api/notify") => {
            #[derive(Deserialize, Default)]
            struct N {
                #[serde(default)]
                from: Option<String>,
            }
            let n: N = serde_json::from_str(&body).unwrap_or_default();
            agent::wake(&shared, n.from.filter(|h| !h.is_empty()));
            reply_json(req, 200, &serde_json::json!({ "ok": true }));
        }
        (Method::Post, "/api/apply") => {
            #[derive(Deserialize, Default)]
            struct A {
                #[serde(default)]
                prune: bool,
                #[serde(default)]
                yes: bool,
                #[serde(default)]
                allow_destroy: bool,
                #[serde(default)]
                hosts: Option<Vec<String>>,
            }
            let a: A = serde_json::from_str(&body).unwrap_or_default();
            let opts = Options {
                prune: a.prune,
                yes: a.yes,
                allow_destroy: a.allow_destroy,
                only_hosts: a.hosts,
                ..Default::default()
            };
            match apply_now(&opts) {
                Ok(r) => {
                    invalidate();
                    agent::wake(&shared, None);
                    reply_json(req, 200, &r)
                }
                Err(e) => reply_err(req, 500, &format!("{e:#}")),
            }
        }
        (Method::Post, "/api/promote") | (Method::Post, "/api/failback") => {
            #[derive(Deserialize)]
            struct H {
                host: String,
            }
            let promote = path == "/api/promote";
            let Ok(h) = serde_json::from_str::<H>(&body) else {
                reply_err(req, 400, "want {\"host\": \"…\"}");
                return;
            };
            match switch(&h.host, promote) {
                Ok(r) => {
                    invalidate();
                    agent::wake(&shared, None);
                    reply_json(req, 200, &r)
                }
                Err(e) => reply_err(req, 400, &format!("{e:#}")),
            }
        }
        _ => reply_err(req, 404, "not found"),
    }
}

/// May `from` use this machine's tokens? It must be a fleet machine on the
/// `policy.remote_from` list, and the request must come from its tailnet
/// address (or loopback), so one machine cannot claim to be another.
fn may_forward(fleet: &Fleet, from: &str, remote: Option<std::net::IpAddr>) -> Result<(), String> {
    let m = fleet.machines.get(from).ok_or_else(|| format!("`{from}` is not a machine in the fleet"))?;
    if let Some(list) = &fleet.policy.remote_from {
        if !list.iter().any(|x| x == from) {
            return Err(format!("`{from}` is not on policy.remote_from"));
        }
    }
    let Some(ip) = remote else { return Err("no remote address".into()) };
    if ip.is_loopback() {
        return Ok(());
    }
    use std::net::ToSocketAddrs;
    let theirs: Vec<std::net::IpAddr> = (m.host.as_str(), 0).to_socket_addrs().map(|a| a.map(|x| x.ip()).collect()).unwrap_or_default();
    if theirs.contains(&ip) {
        Ok(())
    } else {
        Err(format!("this request did not come from {from}'s address"))
    }
}

fn invalidate() {
    *STATUS_CACHE.lock().unwrap() = None;
}

/// The whole fleet as this machine sees it, cached for half a minute —
/// a full look is a few dozen Cloudflare calls.
pub fn full_status(fresh: bool) -> Result<FleetStatus> {
    if !fresh {
        if let Some((at, s)) = STATUS_CACHE.lock().unwrap().as_ref() {
            if util::now_epoch() - at < 30 {
                return Ok(s.clone());
            }
        }
    }
    let config = Config::load()?;
    let fleet = Fleet::load()?.ok_or_else(|| anyhow!("no fleet file on this machine yet"))?;
    let me = fleet::this_machine(&config, Some(&fleet));
    let snap = observe::observe(&config, &Want::default());
    let local = observe::observe_local(&config, &me);
    let mut p = plan::plan(&fleet, &snap, Some(&local));
    p.findings.extend(plan::probe_origins(&fleet, &me));
    let s = status::build(&fleet, &snap, Some(&local), p, &me);
    *STATUS_CACHE.lock().unwrap() = Some((util::now_epoch(), s.clone()));
    Ok(s)
}

fn apply_now(opts: &Options) -> Result<apply::Report> {
    let mut config = Config::load()?;
    let fleet = Fleet::load()?.ok_or_else(|| anyhow!("no fleet file on this machine yet"))?;
    let me = fleet::this_machine(&config, Some(&fleet));
    let snap = observe::observe(&config, &Want::default());
    let local = observe::observe_local(&config, &me);
    let p = plan::plan(&fleet, &snap, Some(&local));
    Ok(apply::apply(&p, &snap, &mut config, opts))
}

/// Promote a hostname to its standby, or return it to its primary: write
/// the decision to the fleet file, tell the peers, and move DNS now.
pub fn switch(host: &str, promote: bool) -> Result<apply::Report> {
    let config = Config::load()?;
    let fleet = Fleet::load()?.ok_or_else(|| anyhow!("no fleet file on this machine yet"))?;
    let me = fleet::this_machine(&config, Some(&fleet));
    let r = fleet.find_route(host).ok_or_else(|| anyhow!("{host} is not in the fleet file"))?;
    if r.standby.is_none() {
        return Err(anyhow!("{host} has no standby"));
    }
    let f = Fleet::edit(&me, |f| {
        let r = f.find_route_mut(host).unwrap();
        r.active = if promote { Some("standby".into()) } else { None };
        Ok(())
    })?;
    sync::notify(&f, &me);
    apply_now(&Options { yes: true, only_hosts: Some(vec![host.to_string()]), no_local: true, ..Default::default() })
}
