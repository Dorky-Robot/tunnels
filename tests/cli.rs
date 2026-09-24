//! The binary, end to end, against a fake Cloudflare. Each test is one of
//! the ways this tool has hurt somebody, or could.

mod support;

use serde_json::json;
use support::*;

const PROD: &str = "6da56f03-7687-46a4-b023-7557decbc04b";
const STBY: &str = "51e97a99-f3d8-4da9-9a7f-2fd2a0567776";
const HOME: &str = "bc598bf7-c3a7-4b4c-aa94-4ac056aa87bc";
const STRAY: &str = "56b5b4ef-68ce-4ed2-9eb0-46ef8e23d179";

fn rule(h: &str, s: &str) -> serde_json::Value {
    json!({ "hostname": h, "service": s })
}

fn catch_all() -> serde_json::Value {
    json!({ "service": "http_status:404" })
}

fn tunnel(id: &str, name: &str, account: &str, up: bool, rules: Vec<serde_json::Value>) -> FakeTunnel {
    FakeTunnel { id: id.into(), name: name.into(), account: account.into(), up, secret: "s0".into(), ingress: rules, deleted: false }
}

fn cname(id: &str, zone: &str, name: &str, tunnel: &str) -> Record {
    Record { id: id.into(), zone: zone.into(), name: name.into(), rtype: "CNAME".into(), content: format!("{tunnel}.cfargotunnel.com") }
}

fn world() -> World {
    let mut w = World::default();
    w.accounts = vec![("acct-vet".into(), "Vet account".into()), ("acct-home".into(), "Home account".into())];
    w.zones.insert("z-vet".into(), ("everyday.vet".into(), "acct-vet".into()));
    w.zones.insert("z-home".into(), ("felixflor.es".into(), "acct-home".into()));
    w.tunnels = vec![
        tunnel(PROD, "dorkyrobot1", "acct-vet", true, vec![rule("admin.everyday.vet", "http://localhost:3300"), catch_all()]),
        tunnel(STBY, "DorkyRobot2", "acct-vet", true, vec![rule("admin.everyday.vet", "http://localhost:3300"), catch_all()]),
        tunnel(HOME, "DorkyRobot2", "acct-home", true, vec![
            json!({ "hostname": "media.felixflor.es", "service": "http://localhost:2283", "originRequest": { "noTLSVerify": true } }),
            catch_all(),
        ]),
        tunnel(STRAY, "mac-2024", "acct-vet", true, vec![rule("staging-admin.everyday.vet", "http://localhost:3000"), catch_all()]),
    ];
    w.records = vec![
        cname("r1", "z-vet", "admin.everyday.vet", PROD),
        cname("r2", "z-home", "media.felixflor.es", HOME),
        cname("r3", "z-vet", "staging-admin.everyday.vet", STRAY),
    ];
    w
}

const FLEET: &str = r#"
serial = 1
[policy]
web_port = 9
[machines.dr1]
host = "127.0.0.1"
[machines.dr2]
host = "127.0.0.1"
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
"#;

fn sandbox() -> Sandbox {
    let s = Sandbox::new(world(), "dr2");
    s.write_config(json!({
        "tunnels": [
            { "name": "DorkyRobot", "token": token_for("acct-vet", STBY, "s0") },
            { "name": "DorkyRobot2", "token": token_for("acct-home", HOME, "s0") },
        ],
        "cf_api_tokens": [{ "token": "good-token" }],
    }));
    s.write_fleet(FLEET);
    s
}

#[test]
fn forget_touches_nothing_in_cloudflare_and_says_so() {
    let s = sandbox();
    let out = s.run(&["tunnel", "forget", "DorkyRobot"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("[this Mac only]"), "scope announced: {}", out.stderr);
    assert!(out.stdout.contains("STILL EXISTS in Cloudflare"), "{}", out.stdout);
    assert!(out.stdout.contains("STILL WORKS"), "{}", out.stdout);
    assert!(out.stdout.contains(&format!("tunnels tunnel destroy {STBY}")), "{}", out.stdout);
    // and the fleet runs it here, so it warns that the agent will bring it back
    assert!(out.stdout.contains("agent will fetch its token and start it again"), "{}", out.stdout);
    assert!(s.world().log.is_empty(), "forget talked to Cloudflare: {:?}", s.world().log);
    assert!(!s.world().tunnel(STBY).deleted);
    let names: Vec<String> = s.config()["tunnels"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, vec!["DorkyRobot2"]);
}

#[test]
fn the_old_rm_refuses_and_names_both_meanings() {
    let s = sandbox();
    let out = s.run(&["rm", "DorkyRobot"]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("tunnel forget") && out.stderr.contains("tunnel destroy"), "{}", out.stderr);
    assert!(s.world().log.is_empty());
    assert_eq!(s.config()["tunnels"].as_array().unwrap().len(), 2, "nothing forgotten");
}

#[test]
fn destroy_kills_the_tunnel_its_dns_and_its_fleet_entry() {
    let s = sandbox();
    // a tunnel only a standby role points at can go; make one that carries nothing
    s.run(&["route", "add", "admin.everyday.vet", "3300", "--tunnel", "vet-prod", "--no-apply"]);
    let out = s.run(&["tunnel", "destroy", "vet-standby", "--yes"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("every connector token for it stops working"), "{}", out.stderr);
    let w = s.world();
    assert!(w.tunnel(STBY).deleted, "gone in Cloudflare");
    drop(w);
    assert!(!s.fleet().contains(STBY), "gone from the fleet file");
    assert!(!s.fleet().contains("standby = \"vet-standby\""), "and from the route that used it as standby");
    // this Mac held its token: forgotten here too
    assert!(!s.config().to_string().contains("\"DorkyRobot\""));
}

#[test]
fn destroy_refuses_a_tunnel_that_is_still_somebodys_primary() {
    let s = sandbox();
    let out = s.run(&["tunnel", "destroy", "dr2-home", "--yes"]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("primary for media.felixflor.es"), "{}", out.stderr);
    assert!(s.world().writes().is_empty(), "{:?}", s.world().writes());
}

#[test]
fn destroy_without_yes_and_without_a_terminal_does_nothing() {
    let s = sandbox();
    let out = s.run(&["tunnel", "destroy", STRAY]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("needs --yes"), "{}", out.stderr);
    assert!(s.world().writes().is_empty());
}

#[test]
fn a_new_route_gets_ingress_and_dns_and_keeps_the_other_rules_whole() {
    let s = sandbox();
    let out = s.run(&["route", "add", "id.felixflor.es", "1411", "--tunnel", "dr2-home"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let w = s.world();
    assert!(w.hosts_on(HOME).contains(&"id.felixflor.es".to_string()));
    assert_eq!(w.cname("id.felixflor.es").as_deref(), Some(format!("{HOME}.cfargotunnel.com").as_str()));
    // the rule we did not touch kept its originRequest
    let media = w.tunnel(HOME).ingress.iter().find(|r| r["hostname"] == "media.felixflor.es").unwrap().clone();
    assert_eq!(media["originRequest"]["noTLSVerify"], true, "{media}");
    // and the catch-all stays last
    assert!(w.tunnel(HOME).ingress.last().unwrap().get("hostname").is_none());
    drop(w);
    assert!(s.fleet().contains("id.felixflor.es"));
}

#[test]
fn a_failed_dns_step_takes_its_ingress_back() {
    let s = sandbox();
    s.world().fail_dns_writes = true;
    let before = s.world().tunnel(HOME).ingress.clone();
    let out = s.run(&["route", "add", "id.felixflor.es", "1411", "--tunnel", "dr2-home"]);
    assert_ne!(out.code, 0, "{}", out.stdout);
    assert!(out.stdout.contains("undone"), "{}", out.stdout);
    assert!(out.stdout.contains("Zone › DNS › Edit"), "says what is wrong: {}", out.stdout);
    assert_eq!(s.world().tunnel(HOME).ingress, before, "no half-made route left behind");
}

#[test]
fn taking_a_hostname_from_a_live_tunnel_needs_yes() {
    let s = sandbox();
    // staging-admin: DNS points at mac2024's live tunnel
    let out = s.run(&["route", "add", "staging-admin.everyday.vet", "3312", "--tunnel", "vet-standby"]);
    assert!(out.stdout.contains("held") && out.stdout.contains("--yes"), "{}{}", out.stdout, out.stderr);
    assert_eq!(s.world().cname("staging-admin.everyday.vet").as_deref(), Some(format!("{STRAY}.cfargotunnel.com").as_str()), "DNS untouched");
    let out = s.run(&["route", "add", "staging-admin.everyday.vet", "3312", "--tunnel", "vet-standby", "--yes"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(s.world().cname("staging-admin.everyday.vet").as_deref(), Some(format!("{STBY}.cfargotunnel.com").as_str()));
}

#[test]
fn a_hostname_aimed_at_a_tunnel_in_the_other_account_is_refused_before_anything_is_touched() {
    let s = sandbox();
    let out = s.run(&["route", "add", "staging-admin.everyday.vet", "3312", "--tunnel", "dr2-home"]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("its own zone's account"), "{}", out.stderr);
    assert!(s.world().writes().is_empty(), "{:?}", s.world().writes());
    assert!(!s.fleet().contains("staging-admin"));
}

#[test]
fn plan_says_there_is_work_and_apply_does_it() {
    let s = sandbox();
    s.world().records.retain(|r| r.name != "media.felixflor.es");
    let out = s.run(&["plan", "--json"]);
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let ops: Vec<&str> = v["plan"]["actions"].as_array().unwrap().iter().map(|a| a["op"].as_str().unwrap()).collect();
    assert!(ops.contains(&"create-dns"), "{ops:?}");
    // the orphan is named, with both ways out
    let orphan = v["plan"]["findings"].as_array().unwrap().iter().find(|f| f["subject"] == "mac-2024").unwrap().clone();
    assert!(orphan["message"].as_str().unwrap().contains("tokens still work"));
    assert!(orphan["fix"].as_str().unwrap().contains("tunnels tunnel destroy"));

    let out = s.run(&["apply", "--no-local"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(s.world().cname("media.felixflor.es").is_some());
    let out = s.run(&["plan", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let left: Vec<&serde_json::Value> = v["plan"]["actions"].as_array().unwrap().iter().filter(|a| a["scope"] != "local").collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn undeclared_ingress_waits_for_prune() {
    let s = sandbox();
    s.world().tunnels.iter_mut().find(|t| t.id == HOME).unwrap().ingress.insert(0, rule("old.felixflor.es", "http://localhost:9"));
    let out = s.run(&["apply", "--no-local"]);
    assert!(out.stdout.contains("held") && out.stdout.contains("--prune"), "{}", out.stdout);
    assert!(s.world().hosts_on(HOME).contains(&"old.felixflor.es".to_string()));
    s.run(&["apply", "--no-local", "--prune"]);
    assert!(!s.world().hosts_on(HOME).contains(&"old.felixflor.es".to_string()));
}

#[test]
fn rotate_kills_the_old_token_and_this_mac_takes_the_new_one() {
    let s = sandbox();
    let out = s.run(&["tunnel", "rotate", "dr2-home"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("no longer works"), "{}", out.stdout);
    let secret = s.world().tunnel(HOME).secret.clone();
    assert_ne!(secret, "s0");
    let tok = s.config()["tunnels"].as_array().unwrap().iter().find(|t| t["name"] == "DorkyRobot2").unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(tok, token_for("acct-home", HOME, &secret));
    // and the token is in a 0600 file, not in the plist
    let file = s.path(&format!("config/tokens/{HOME}"));
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777;
    assert_eq!(mode, 0o600);
    let plist = std::fs::read_to_string(s.path("LaunchAgents/com.cloudflare.cloudflared-DorkyRobot2.plist")).unwrap();
    assert!(plist.contains("--token-file") && !plist.contains(&tok), "{plist}");
}

#[test]
fn promote_and_failback_move_dns_and_record_it_in_the_fleet() {
    let s = sandbox();
    let out = s.run(&["promote", "admin.everyday.vet"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(s.world().cname("admin.everyday.vet").as_deref(), Some(format!("{STBY}.cfargotunnel.com").as_str()));
    assert!(s.fleet().contains("active = \"standby\""));
    let out = s.run(&["failback", "admin.everyday.vet"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(s.world().cname("admin.everyday.vet").as_deref(), Some(format!("{PROD}.cfargotunnel.com").as_str()));
    assert!(!s.fleet().contains("active ="));
}

#[test]
fn import_writes_down_what_exists_and_skips_what_does_not_agree() {
    let s = sandbox();
    std::fs::remove_file(s.path("config/fleet.toml")).unwrap();
    let out = s.run(&["import", "--machine", "dr2", "--host", "127.0.0.1"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let f = s.fleet();
    assert!(f.contains(HOME) && f.contains(STBY), "{f}");
    assert!(f.contains("media.felixflor.es"), "a route whose ingress and DNS agree is imported");
    assert!(!f.contains(PROD), "a tunnel another machine runs is not claimed");
    // the standby carries admin, but DNS sends it elsewhere: named, not imported
    assert!(out.stdout.contains("admin.everyday.vet is in") && out.stdout.contains("not imported"), "{}", out.stdout);
    let out = s.run(&["fleet", "validate"]);
    assert_eq!(out.code, 0, "{}", out.stdout);
}

#[test]
fn json_output_carries_the_scope() {
    let s = sandbox();
    let out = s.run(&["tunnel", "list", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["scope"], "local");
    assert!(s.world().log.is_empty(), "listing this Mac's tunnels asked Cloudflare: {:?}", s.world().log);
}

// ------------------------------------------------------------------ tunnels cf

#[test]
fn cf_reads_by_name_and_never_shows_a_token() {
    let s = sandbox();
    let out = s.run(&["cf", "GET", "/zones/{zone:everyday.vet}/settings/ssl"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("\"value\": \"full\""), "{}", out.stdout);
    assert!(out.stderr.contains("{zone:everyday.vet} = z-vet"), "shows what the name became: {}", out.stderr);
    assert!(!out.stdout.contains("good-token") && !out.stderr.contains("good-token"));
    // a connector token is a secret too, even when asked for directly
    let out = s.run(&["cf", "get", "/accounts/{account:home}/cfd_tunnel/{tunnel:dr2-home}/token"]);
    assert!(out.stdout.contains("hidden by tunnels"), "{}", out.stdout);
    assert!(!out.stdout.contains(&token_for("acct-home", HOME, "s0")));
    // and so is an identity provider's client secret
    let out = s.run(&["cf", "get", "/accounts/{account:home}/access/identity_providers/idp1"]);
    assert!(out.stdout.contains("hidden by tunnels") && !out.stdout.contains("very-secret"), "{}", out.stdout);
}

#[test]
fn cf_ambiguous_names_are_errors_not_guesses() {
    let s = sandbox();
    // two Cloudflare tunnels are called DorkyRobot2, in two accounts
    let out = s.run(&["cf", "get", "/accounts/{account:vet}/cfd_tunnel/{tunnel:DorkyRobot2}"]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("names 2 tunnels"), "{}", out.stderr);
}

#[test]
fn cf_writes_are_previews_until_yes_then_logged_and_undoable() {
    let s = sandbox();
    let path = "/zones/{zone:everyday.vet}/settings/ssl";
    let out = s.run(&["cf", "patch", path, "--data", r#"{"value":"strict"}"#]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("preview only"), "{}", out.stdout);
    assert!(out.stderr.contains("[cloudflare]"), "scope announced: {}", out.stderr);
    assert!(s.world().writes().is_empty(), "a preview sent something: {:?}", s.world().writes());

    let out = s.run(&["cf", "patch", path, "--data", r#"{"value":"strict"}"#, "--yes", "--json"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["scope"], "cloudflare");
    assert_eq!(v["after"]["value"], "strict", "it read the change back");
    assert_eq!(v["undo"]["body"], json!({ "value": "full" }));
    let id = v["log_id"].as_str().unwrap().to_string();
    assert_eq!(s.world().ssl.get("z-vet").map(String::as_str), Some("strict"));

    let log = s.run(&["cf", "log"]);
    assert!(log.stdout.contains(&id) && log.stdout.contains("value: \"full\" → \"strict\""), "{}", log.stdout);

    let out = s.run(&["cf", "undo", &id, "--yes"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(s.world().ssl.get("z-vet").map(String::as_str), Some("full"), "undone");
    let log = s.run(&["cf", "log"]);
    assert!(log.stdout.contains(&format!("(undid {id})")), "{}", log.stdout);
}

#[test]
fn cf_a_post_is_undone_by_deleting_what_it_made() {
    let s = sandbox();
    let out = s.run(&["cf", "post", "/accounts/{account:home}/access/apps", "--data", r#"{"name":"tunnels","domain":"tunnels.felixflor.es"}"#, "--yes", "--json"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["after"]["domain"], "tunnels.felixflor.es");
    assert_eq!(s.world().access_apps["acct-home"].len(), 1);
    let id = v["log_id"].as_str().unwrap().to_string();
    let out = s.run(&["cf", "undo", &id, "--yes"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(s.world().access_apps["acct-home"].is_empty());
}

#[test]
fn cf_refuses_what_tunnels_owns() {
    let s = sandbox();
    let out = s.run(&["cf", "put", "/accounts/{account:home}/cfd_tunnel/{tunnel:dr2-home}/configurations", "--data", "{}", "--yes"]);
    assert_ne!(out.code, 0);
    assert!(out.stdout.contains("refused") && out.stdout.contains("tunnels route"), "{}", out.stdout);
    let out = s.run(&[
        "cf", "post", "/zones/{zone:felixflor.es}/dns_records",
        "--data", &format!(r#"{{"type":"CNAME","name":"x.felixflor.es","content":"{HOME}.cfargotunnel.com"}}"#), "--yes",
    ]);
    assert_ne!(out.code, 0);
    assert!(out.stdout.contains("belong to its route"), "{}", out.stdout);
    // and an existing tunnel CNAME cannot be changed or deleted through it either
    let out = s.run(&["cf", "delete", "/zones/{zone:felixflor.es}/dns_records/{record:media.felixflor.es}", "--yes"]);
    assert_ne!(out.code, 0);
    assert!(out.stdout.contains("belong to its route"), "{}", out.stdout);
    assert!(s.world().writes().is_empty(), "{:?}", s.world().writes());
}

#[test]
fn cf_token_changes_need_their_own_flag() {
    let s = sandbox();
    let out = s.run(&["cf", "post", "/user/tokens", "--data", "{}", "--yes", "--account", "home"]);
    assert_ne!(out.code, 0);
    assert!(out.stdout.contains("--i-mean-tokens"), "{}", out.stdout);
    assert!(s.world().writes().is_empty());
}

#[test]
fn token_refresh_labels_what_a_bare_token_reaches() {
    let s = sandbox();
    let out = s.run(&["token", "refresh"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("everyday.vet") && out.stdout.contains("felixflor.es"), "{}", out.stdout);
    let covers = s.config()["cf_api_tokens"][0]["covers"].as_str().unwrap().to_string();
    assert!(covers.contains("Vet account"), "{covers}");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Two machines on one fake Cloudflare: `b` holds the token and runs its
/// agent; `a` holds none.
fn two_machines(remote_from: Option<&str>) -> (Sandbox, Sandbox, std::process::Child) {
    let port = free_port();
    let fleet = FLEET
        .replace("web_port = 9", &format!("web_port = {port}{}", remote_from.map(|r| format!("\nremote_from = {r}")).unwrap_or_default()))
        .replace("[machines.dr1]", "[machines.a]\nhost = \"127.0.0.1\"\n[machines.dr1]");
    let b = sandbox();
    b.write_fleet(&fleet);
    let mut agent_b = b.command(&["agent", "run"]);
    agent_b.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let child = agent_b.spawn().unwrap();
    let a = Sandbox::new(World::default(), "a");
    // a's fake Cloudflare is b's: the same world, seen without a token
    let a = Sandbox { fake: Fake { world: b.fake.world.clone(), url: b.fake.url.clone() }, ..a };
    a.write_config(json!({ "tunnels": [] }));
    a.write_fleet(&fleet);
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    (a, b, child)
}

#[test]
fn cf_without_a_token_goes_through_a_peer_that_has_one() {
    let (a, b, mut child) = two_machines(None);
    let out = a.run(&["cf", "patch", "/zones/{zone:everyday.vet}/settings/ssl", "--data", r#"{"value":"strict"}"#, "--yes"]);
    let _ = child.kill();
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("made by dr1") || out.stderr.contains("made by dr2"), "says who made it: {}", out.stderr);
    assert_eq!(b.world().ssl.get("z-vet").map(String::as_str), Some("strict"));
    // logged on the machine that made it, saying who asked
    let log = std::fs::read_dir(b.path("config/cf-log")).unwrap().next().unwrap().unwrap().path();
    let rec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(log).unwrap()).unwrap();
    assert_eq!(rec["requested_by"], "a");
    assert!(!a.path("config/cf-log").exists(), "nothing logged on the machine that only asked");
}

#[test]
fn a_machine_off_the_allowlist_cannot_use_a_peers_token() {
    let (a, b, mut child) = two_machines(Some(r#"["dr1", "dr2"]"#));
    let out = a.run(&["cf", "patch", "/zones/{zone:everyday.vet}/settings/ssl", "--data", r#"{"value":"strict"}"#, "--yes"]);
    let _ = child.kill();
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("not on policy.remote_from"), "{}", out.stderr);
    assert!(b.world().ssl.get("z-vet").is_none(), "nothing changed");
}

// ------------------------------------------------------------------ the web UI on the internet, signed in with pocket-id

struct Web {
    s: Sandbox,
    child: std::process::Child,
    port: u16,
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn published_ui() -> Web {
    let port = free_port();
    let s = sandbox();
    let fleet = FLEET.replace(
        "web_port = 9",
        &format!(
            "web_port = {port}\nremote_from = [\"dr1\", \"dr2\"]\n[policy.web]\npublic_host = \"tunnels.felixflor.es\"\nissuer = \"{}\"\nclient_id = \"tunnels\"\nadmins = [\"felix@example.com\"]",
            s.fake.url
        ),
    );
    s.write_fleet(&fleet);
    let mut c = s.command(&["agent", "run"]);
    c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let child = c.spawn().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Web { s, child, port }
}

struct Resp {
    code: u16,
    location: String,
    cookie: String,
    body: String,
}

/// A request as it arrives through the tunnel: from this Mac's cloudflared,
/// with Cloudflare's headers, for the public host.
fn public(w: &Web, method: &str, path: &str, cookie: Option<&str>, host: &str, body: Option<serde_json::Value>) -> Resp {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
    let url = format!("http://127.0.0.1:{}{path}", w.port);
    let mut r = if method == "GET" {
        let mut q = a.get(&url).header("Host", host).header("Cf-Ray", "test");
        if let Some(c) = cookie {
            q = q.header("Cookie", c);
        }
        q.call().unwrap()
    } else {
        let mut q = a.post(&url).header("Host", host).header("Cf-Ray", "test").header("X-Tunnels", "1");
        if let Some(c) = cookie {
            q = q.header("Cookie", c);
        }
        q.send_json(body.unwrap_or(json!({}))).unwrap()
    };
    let h = |n: &str| r.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (location, cookie) = (h("location"), h("set-cookie"));
    Resp { code: r.status().as_u16(), location, cookie, body: r.body_mut().read_to_string().unwrap_or_default() }
}

/// Sign in as `email` the way a browser would: /auth/login sends it to
/// pocket-id, pocket-id sends it back to /auth/callback with a code.
fn sign_in(w: &Web, email: &str) -> String {
    let r = public(w, "GET", "/auth/login", None, "tunnels.felixflor.es", None);
    assert_eq!(r.code, 302, "{}", r.body);
    assert!(r.location.starts_with(&format!("{}/authorize?", w.s.fake.url)), "{}", r.location);
    assert!(r.location.contains("code_challenge_method=S256") && r.location.contains("client_id=tunnels"), "{}", r.location);
    assert!(r.location.contains("redirect_uri=https%3A%2F%2Ftunnels.felixflor.es%2Fauth%2Fcallback"), "{}", r.location);
    let state = r.location.split('&').find_map(|kv| kv.strip_prefix("state=")).unwrap().to_string();
    let r = public(w, "GET", &format!("/auth/callback?code=code-{email}&state={state}"), None, "tunnels.felixflor.es", None);
    assert_eq!(r.code, 302, "{}", r.body);
    assert!(r.cookie.contains("HttpOnly") && r.cookie.contains("Secure"), "{}", r.cookie);
    r.cookie.split(';').next().unwrap().to_string()
}

#[test]
fn from_the_internet_nobody_gets_in_without_signing_in() {
    let w = published_ui();
    let r = public(&w, "GET", "/", None, "tunnels.felixflor.es", None);
    assert_eq!((r.code, r.location.as_str()), (302, "/auth/login"), "a browser is sent to sign in");
    let r = public(&w, "GET", "/api/status", None, "tunnels.felixflor.es", None);
    assert_eq!(r.code, 401, "the API says so plainly");
    let r = public(&w, "GET", "/api/whoami", Some("tunnels_session=made-up"), "tunnels.felixflor.es", None);
    assert_eq!(r.code, 401, "an invented session is nobody");
    let r = public(&w, "GET", "/api/whoami", None, "other.felixflor.es", None);
    assert_eq!(r.code, 403, "only the declared public host");
    // a callback nobody started is refused
    let r = public(&w, "GET", "/auth/callback?code=code-felix@example.com&state=never-issued", None, "tunnels.felixflor.es", None);
    assert_eq!(r.code, 403, "{}", r.body);
    assert!(r.cookie.is_empty());
}

#[test]
fn a_guest_can_look_but_not_change_anything() {
    let w = published_ui();
    let c = sign_in(&w, "guest@example.com");
    let r = public(&w, "GET", "/api/whoami", Some(&c), "tunnels.felixflor.es", None);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!((v["who"].as_str(), v["admin"].as_bool()), (Some("guest@example.com"), Some(false)));
    let r = public(&w, "POST", "/api/relay", Some(&c), "tunnels.felixflor.es", Some(json!({ "machine": "dr2", "action": "agent-pass" })));
    assert_eq!(r.code, 403, "{}", r.body);
    assert!(r.body.contains("can look but not change"), "{}", r.body);
    assert_eq!(public(&w, "POST", "/api/apply", Some(&c), "tunnels.felixflor.es", None).code, 403);
}

#[test]
fn an_admin_acts_is_named_in_the_record_and_can_sign_out() {
    let w = published_ui();
    let c = sign_in(&w, "Felix@Example.com");
    assert!(!w.s.world().oidc_verifiers.is_empty(), "the PKCE verifier went to pocket-id");
    let r = public(&w, "POST", "/api/relay", Some(&c), "tunnels.felixflor.es", Some(json!({ "machine": "dr2", "action": "agent-pass" })));
    assert_eq!(r.code, 200, "{}", r.body);
    let a: ureq::Agent = ureq::Agent::config_builder().build().into();
    let ev: serde_json::Value = a.get(&format!("http://127.0.0.1:{}/api/agent", w.port)).call().unwrap().body_mut().read_json().unwrap();
    assert!(ev["events"].to_string().contains("felix@example.com: agent pass started"), "{}", ev["events"]);
    let r = public(&w, "GET", "/auth/logout", Some(&c), "tunnels.felixflor.es", None);
    assert_eq!(r.code, 302);
    assert_eq!(public(&w, "GET", "/api/whoami", Some(&c), "tunnels.felixflor.es", None).code, 401, "signed out means signed out");
}

#[test]
fn machine_to_machine_endpoints_are_never_reachable_from_the_internet() {
    let w = published_ui();
    let c = sign_in(&w, "felix@example.com");
    for path in ["/api/cf-forward", "/api/relay-exec", "/api/notify"] {
        let r = public(&w, "POST", path, Some(&c), "tunnels.felixflor.es", Some(json!({ "requested_by": "dr1", "action": "agent-pass", "method": "GET", "path": "/x" })));
        assert_eq!(r.code, 403, "{path}: {}", r.body);
    }
}

#[test]
fn a_relay_is_only_taken_from_a_machine_on_the_allowlist() {
    let w = published_ui();
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let post = |from: &str| {
        let mut r = a
            .post(&format!("http://127.0.0.1:{}/api/relay-exec", w.port))
            .header("X-Tunnels", "1")
            .send_json(json!({ "action": "agent-pass", "requested_by": from, "actor": "felix@example.com" }))
            .unwrap();
        (r.status().as_u16(), r.body_mut().read_to_string().unwrap_or_default())
    };
    assert_eq!(post("dr1").0, 200);
    let (code, body) = post("doug-mini");
    assert_eq!(code, 403, "{body}");
}
