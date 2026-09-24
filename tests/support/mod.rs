//! A fake Cloudflare on localhost, and a sandbox to run the real binary in.
//!
//! The fake keeps accounts, zones, tunnels, ingress and DNS in memory, logs
//! every request, and can be told to fail DNS writes — enough to test what
//! the CLI does to Cloudflare, and what it must not do, end to end.

#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
pub struct FakeTunnel {
    pub id: String,
    pub name: String,
    pub account: String,
    pub up: bool,
    pub secret: String,
    pub ingress: Vec<Value>,
    pub deleted: bool,
}

#[derive(Default, Clone)]
pub struct Record {
    pub id: String,
    pub zone: String,
    pub name: String,
    pub rtype: String,
    pub content: String,
}

#[derive(Default)]
pub struct World {
    pub accounts: Vec<(String, String)>,
    /// zone id → (name, account id)
    pub zones: BTreeMap<String, (String, String)>,
    pub tunnels: Vec<FakeTunnel>,
    pub records: Vec<Record>,
    pub log: Vec<(String, String)>,
    pub fail_dns_writes: bool,
    /// zone id → ssl mode
    pub ssl: BTreeMap<String, String>,
    /// account id → Access apps
    pub access_apps: BTreeMap<String, Vec<Value>>,
    next: u32,
}

impl World {
    pub fn tunnel(&self, id: &str) -> &FakeTunnel {
        self.tunnels.iter().find(|t| t.id == id).unwrap()
    }

    pub fn hosts_on(&self, id: &str) -> Vec<String> {
        self.tunnel(id).ingress.iter().filter_map(|r| r.get("hostname").and_then(|h| h.as_str()).map(String::from)).collect()
    }

    pub fn cname(&self, host: &str) -> Option<String> {
        self.records.iter().find(|r| r.name == host && r.rtype == "CNAME").map(|r| r.content.clone())
    }

    pub fn writes(&self) -> Vec<(String, String)> {
        self.log.iter().filter(|(m, _)| m != "GET").cloned().collect()
    }
}

pub fn token_for(account: &str, tunnel: &str, secret: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(json!({ "a": account, "t": tunnel, "s": secret }).to_string())
}

pub struct Fake {
    pub world: Arc<Mutex<World>>,
    pub url: String,
}

fn ok(result: Value) -> (u16, Value) {
    (200, json!({ "success": true, "errors": [], "result": result, "result_info": { "total_pages": 1 } }))
}

fn err(code: u16, msg: &str) -> (u16, Value) {
    (code, json!({ "success": false, "errors": [{ "code": 1000, "message": msg }], "result": null }))
}

fn query(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}="))).map(|v| v.replace("%20", " "))
}

impl Fake {
    pub fn start(world: World) -> Fake {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let world = Arc::new(Mutex::new(world));
        let w = world.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let method = req.method().to_string();
                let full = req.url().to_string();
                let (path, q) = full.split_once('?').unwrap_or((full.as_str(), ""));
                let mut world = w.lock().unwrap();
                world.log.push((method.clone(), path.to_string()));
                let (code, v) = route(&mut world, &method, path, q, &body);
                drop(world);
                let _ = req.respond(
                    tiny_http::Response::from_string(v.to_string())
                        .with_status_code(code)
                        .with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap()),
                );
            }
        });
        Fake { world, url }
    }
}

fn tunnel_json(t: &FakeTunnel) -> Value {
    json!({
        "id": t.id, "name": t.name, "account_tag": t.account,
        "status": if t.up { "healthy" } else { "down" },
        "connections": if t.up { json!([{ "colo_name": "sfo", "origin_ip": "1.2.3.4", "client_version": "2026.9.1", "opened_at": "2026-09-23T00:00:00Z", "client_id": "c" }]) } else { json!([]) },
        "conns_inactive_at": if t.up { Value::Null } else { json!("2026-09-23T00:00:00Z") },
        "deleted_at": if t.deleted { json!("2026-09-23T00:00:00Z") } else { Value::Null },
    })
}

fn route(w: &mut World, method: &str, path: &str, q: &str, body: &str) -> (u16, Value) {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match (method, parts.as_slice()) {
        ("GET", ["user", "tokens", "verify"]) => ok(json!({ "status": "active" })),
        ("GET", ["accounts"]) => ok(Value::Array(w.accounts.iter().map(|(id, n)| json!({ "id": id, "name": n })).collect())),
        ("GET", ["zones"]) => ok(Value::Array(
            w.zones
                .iter()
                .map(|(id, (name, acct))| {
                    let an = w.accounts.iter().find(|a| &a.0 == acct).map(|a| a.1.clone()).unwrap_or_default();
                    json!({ "id": id, "name": name, "account": { "id": acct, "name": an } })
                })
                .collect(),
        )),
        ("GET", ["accounts", a, "cfd_tunnel"]) => {
            ok(Value::Array(w.tunnels.iter().filter(|t| t.account == *a && !t.deleted).map(tunnel_json).collect()))
        }
        ("POST", ["accounts", a, "cfd_tunnel"]) => {
            w.next += 1;
            let id = format!("00000000-0000-4000-8000-{:012}", w.next);
            let t = FakeTunnel {
                id: id.clone(),
                name: body["name"].as_str().unwrap_or("").into(),
                account: a.to_string(),
                up: false,
                secret: body["tunnel_secret"].as_str().unwrap_or("").into(),
                ..Default::default()
            };
            w.tunnels.push(t.clone());
            ok(tunnel_json(&t))
        }
        (_, ["accounts", a, "cfd_tunnel", t, rest @ ..]) => {
            let Some(tun) = w.tunnels.iter_mut().find(|x| x.id == *t && x.account == *a && !x.deleted) else {
                return err(404, "tunnel not found");
            };
            match (method, rest) {
                ("GET", []) => ok(tunnel_json(tun)),
                ("PATCH", []) => {
                    if let Some(s) = body["tunnel_secret"].as_str() {
                        tun.secret = s.into();
                    }
                    ok(tunnel_json(tun))
                }
                ("DELETE", []) => {
                    if tun.up {
                        return err(400, "tunnel has active connections");
                    }
                    tun.deleted = true;
                    ok(json!({}))
                }
                ("DELETE", ["connections"]) => {
                    tun.up = false;
                    ok(json!({}))
                }
                ("GET", ["token"]) => {
                    let tok = token_for(a, t, &tun.secret);
                    ok(json!(tok))
                }
                ("GET", ["configurations"]) => {
                    if tun.ingress.is_empty() {
                        ok(json!({ "config": null }))
                    } else {
                        ok(json!({ "config": { "ingress": tun.ingress } }))
                    }
                }
                ("PUT", ["configurations"]) => {
                    tun.ingress = body["config"]["ingress"].as_array().cloned().unwrap_or_default();
                    ok(json!({}))
                }
                _ => err(404, "no such endpoint"),
            }
        }
        ("GET", ["zones", z, "settings", "ssl"]) => {
            let v = w.ssl.get(*z).cloned().unwrap_or_else(|| "full".into());
            ok(json!({ "id": "ssl", "value": v, "editable": true, "modified_on": "t0" }))
        }
        ("PATCH", ["zones", z, "settings", "ssl"]) => {
            let v = body["value"].as_str().unwrap_or("").to_string();
            w.ssl.insert(z.to_string(), v.clone());
            ok(json!({ "id": "ssl", "value": v, "editable": true, "modified_on": "t1" }))
        }
        ("GET", ["accounts", a, "access", "apps"]) => ok(Value::Array(w.access_apps.get(*a).cloned().unwrap_or_default())),
        ("POST", ["accounts", a, "access", "apps"]) => {
            w.next += 1;
            let mut app = body.clone();
            app["id"] = json!(format!("app{}", w.next));
            app["aud"] = json!("aud-secretless");
            w.access_apps.entry(a.to_string()).or_default().push(app.clone());
            ok(app)
        }
        ("GET", ["accounts", a, "access", "apps", id]) => match w.access_apps.get(*a).and_then(|v| v.iter().find(|x| x["id"] == *id)) {
            Some(app) => ok(app.clone()),
            None => err(404, "app not found"),
        },
        ("DELETE", ["accounts", a, "access", "apps", id]) => {
            if let Some(v) = w.access_apps.get_mut(*a) {
                v.retain(|x| x["id"] != *id);
            }
            ok(json!({ "id": id }))
        }
        ("GET", ["accounts", a, "access", "identity_providers", _id]) => {
            let _ = a;
            ok(json!({ "id": "idp1", "name": "pocket-id", "config": { "client_id": "cid", "client_secret": "very-secret" } }))
        }
        ("GET", ["zones", z, "dns_records"]) => {
            let ty = query(q, "type");
            let name = query(q, "name");
            ok(Value::Array(
                w.records
                    .iter()
                    .filter(|r| r.zone == *z)
                    .filter(|r| ty.as_ref().map(|t| &r.rtype == t).unwrap_or(true))
                    .filter(|r| name.as_ref().map(|n| &r.name == n).unwrap_or(true))
                    .map(|r| json!({ "id": r.id, "name": r.name, "type": r.rtype, "content": r.content, "proxied": true }))
                    .collect(),
            ))
        }
        ("GET", ["zones", _z, "dns_records", id]) => match w.records.iter().find(|r| r.id == *id) {
            Some(r) => ok(json!({ "id": r.id, "name": r.name, "type": r.rtype, "content": r.content, "proxied": true })),
            None => err(404, "no record"),
        },
        ("POST", ["zones", z, "dns_records"]) => {
            if w.fail_dns_writes {
                return err(403, "Authentication error");
            }
            w.next += 1;
            let r = Record {
                id: format!("rec{}", w.next),
                zone: z.to_string(),
                name: body["name"].as_str().unwrap_or("").into(),
                rtype: body["type"].as_str().unwrap_or("").into(),
                content: body["content"].as_str().unwrap_or("").into(),
            };
            w.records.push(r.clone());
            ok(json!({ "id": r.id, "name": r.name, "type": r.rtype, "content": r.content, "proxied": true }))
        }
        ("PUT", ["zones", _z, "dns_records", id]) => {
            if w.fail_dns_writes {
                return err(403, "Authentication error");
            }
            let Some(r) = w.records.iter_mut().find(|r| r.id == *id) else { return err(404, "no record") };
            r.content = body["content"].as_str().unwrap_or("").into();
            ok(json!({ "id": r.id, "name": r.name, "type": r.rtype, "content": r.content }))
        }
        ("DELETE", ["zones", _z, "dns_records", id]) => {
            if w.fail_dns_writes {
                return err(403, "Authentication error");
            }
            w.records.retain(|r| r.id != *id);
            ok(json!({ "id": id }))
        }
        _ => err(404, "no such endpoint"),
    }
}

/// A private HOME for one test: config, fleet, LaunchAgents, logs — and a
/// launchctl that does nothing.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub fake: Fake,
    pub machine: String,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Sandbox {
    pub fn new(world: World, machine: &str) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        Sandbox { dir, fake: Fake::start(world), machine: machine.into() }
    }

    pub fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    pub fn write_config(&self, v: Value) {
        std::fs::write(self.path("config/config.json"), v.to_string()).unwrap();
    }

    pub fn config(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.path("config/config.json")).unwrap()).unwrap()
    }

    pub fn write_fleet(&self, toml: &str) {
        std::fs::write(self.path("config/fleet.toml"), toml).unwrap();
    }

    pub fn fleet(&self) -> String {
        std::fs::read_to_string(self.path("config/fleet.toml")).unwrap_or_default()
    }

    pub fn command(&self, args: &[&str]) -> std::process::Command {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_tunnels"));
        c.args(args)
            .env("TUNNELS_CONFIG", self.path("config/config.json"))
            .env("TUNNELS_FLEET", self.path("config/fleet.toml"))
            .env("TUNNELS_CF_API", &self.fake.url)
            .env("TUNNELS_MACHINE", &self.machine)
            .env("TUNNELS_LAUNCH_AGENTS", self.path("LaunchAgents"))
            .env("TUNNELS_LOG_DIR", self.path("logs"))
            .env("TUNNELS_LAUNCHCTL", "/usr/bin/true")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY");
        c
    }

    pub fn run(&self, args: &[&str]) -> Out {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_tunnels"))
            .args(args)
            .env("TUNNELS_CONFIG", self.path("config/config.json"))
            .env("TUNNELS_FLEET", self.path("config/fleet.toml"))
            .env("TUNNELS_CF_API", &self.fake.url)
            .env("TUNNELS_MACHINE", &self.machine)
            .env("TUNNELS_LAUNCH_AGENTS", self.path("LaunchAgents"))
            .env("TUNNELS_LOG_DIR", self.path("logs"))
            .env("TUNNELS_LAUNCHCTL", "/usr/bin/true")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into(),
            stderr: String::from_utf8_lossy(&out.stderr).into(),
        }
    }

    pub fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.fake.world.lock().unwrap()
    }
}
