//! The command line. Every command says where it acts — this Mac, the fleet
//! file, Cloudflare — in its help and before it acts, and carries the same
//! in `--json`. See `scope.rs` for why.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::collections::BTreeSet;
use std::io::IsTerminal;
use std::time::Duration;
use tunnels::apply::{self, Options};
use tunnels::config::{self, Config};
use tunnels::fleet::{self, Fleet, Route, TunnelDecl};
use tunnels::observe::{self, Snapshot, Want};
use tunnels::scope::{self, Scope};
use tunnels::{cf, launchd, plan, scan, status, sync, util, web};

#[derive(Parser)]
#[command(
    name = "tunnels",
    version,
    about = "Cloudflare tunnels across a fleet of Macs: a config file, a CLI, an agent on every machine.",
    long_about = "Cloudflare tunnels across a fleet of Macs.\n\n\
        The fleet file (~/.config/tunnels/fleet.toml) says which machine runs which tunnel and where \
        every hostname goes. `tunnels plan` shows how Cloudflare and this Mac differ from it; \
        `tunnels apply` closes the gap; the agent on each machine keeps closing it.\n\n\
        Every command says where it acts: [this Mac only], [read-only], [fleet file + cloudflare], …",
    after_help = "Start here:\n  tunnels status            everything, everywhere\n  tunnels import            write the fleet file from what exists\n  tunnels agent install     keep this Mac in line, serve the web UI\n\nConfig: ~/.config/tunnels/  ·  Logs: ~/Library/Logs/tunnels/  ·  Docs: https://github.com/Dorky-Robot/tunnels"
)]
struct Cli {
    /// machine-readable output
    #[arg(long, short = 'j', global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// [read-only] Every tunnel, connector, route and DNS record across every account; what differs from the fleet file
    Status {
        /// only this Mac: its tunnels and LaunchAgents, no Cloudflare
        #[arg(long)]
        local: bool,
    },
    /// [read-only] What `apply` would change to match the fleet file (exit 2 when there is something to do)
    Plan,
    /// [this Mac + cloudflare] Make Cloudflare and this Mac match the fleet file
    Apply(ApplyArgs),
    /// [read-only] Only the problems: down tunnels, orphans, dead origins, missing DNS
    Doctor,
    /// [fleet file + cloudflare] Write this Mac and what it runs into the fleet file (merges; never removes)
    Import {
        /// this machine's name in the fleet (default: its hostname)
        #[arg(long)]
        machine: Option<String>,
        /// the name other machines reach this one by on the tailnet (default: its hostname)
        #[arg(long)]
        host: Option<String>,
        /// show what would be added, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// The fleet file itself
    #[command(subcommand)]
    Fleet(FleetCmd),
    /// Hostnames: where each goes
    #[command(subcommand)]
    Route(RouteCmd),
    /// [fleet file + cloudflare] Send a hostname's traffic to its standby now
    Promote { host: String },
    /// [fleet file + cloudflare] Send a hostname's traffic back to its primary
    Failback { host: String },
    /// Tunnels: create, run, forget, destroy, rotate
    #[command(subcommand)]
    Tunnel(TunnelCmd),
    /// Cloudflare API tokens on this Mac
    #[command(subcommand)]
    Token(TokenCmd),
    /// The agent that keeps this Mac in line and serves the web UI
    #[command(subcommand)]
    Agent(AgentCmd),
    /// The whole Cloudflare API, by name, with guardrails (reads free, writes previewed, logged, undoable)
    #[command(subcommand)]
    Cf(CfCmd),
    /// [read-only] Print (or open) this Mac's web UI address
    Web {
        #[arg(long)]
        open: bool,
    },
    /// [this Mac only] Listening TCP ports here, and which project each belongs to
    Scan,

    // --- the old commands, kept so scripts keep working
    #[command(hide = true, alias = "ls")]
    List,
    #[command(hide = true)]
    Routes { tunnel: Option<String> },
    #[command(hide = true)]
    Start { name: String },
    #[command(hide = true)]
    Stop { name: String },
    #[command(hide = true)]
    Restart { name: String },
    #[command(hide = true)]
    Logs {
        name: String,
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    #[command(hide = true)]
    Add {
        name: String,
        #[arg(long)]
        token: String,
    },
    #[command(hide = true, alias = "remove")]
    Rm { name: Option<String> },
    #[command(hide = true)]
    Sync,
    #[command(hide = true)]
    Heal,
}

#[derive(Args, Clone, Default)]
struct ApplyArgs {
    /// also take hostnames from tunnels serving them now (live takeovers)
    #[arg(long)]
    yes: bool,
    /// also remove ingress and DNS the fleet file does not mention
    #[arg(long)]
    prune: bool,
    /// also destroy tunnels marked `destroy = true` — kills their connector tokens
    #[arg(long)]
    allow_destroy: bool,
    /// only actions about this hostname (repeatable)
    #[arg(long = "host")]
    hosts: Vec<String>,
    /// leave this Mac's LaunchAgents alone
    #[arg(long)]
    no_local: bool,
}

#[derive(Subcommand)]
enum FleetCmd {
    /// [this Mac only] Print the fleet file
    Show,
    /// [this Mac only] Print where the fleet file is
    Path,
    /// [this Mac only] Check the fleet file
    Validate,
    /// [fleet file + cloudflare] Edit the fleet file in $EDITOR; checked and shared with the other machines when you save
    Edit,
    /// [this Mac only] Earlier versions of the fleet file kept here
    History,
    /// [this Mac only] Take the fleet file from another machine's agent
    Join {
        /// the machine to take it from (tailnet name or address)
        host: String,
        /// this machine's name in the fleet
        #[arg(long)]
        machine: Option<String>,
        #[arg(long, default_value_t = fleet::DEFAULT_WEB_PORT)]
        port: u16,
    },
    /// [this Mac only] Take the newest fleet file from the peers now, and tell them about ours
    Sync,
}

#[derive(Subcommand)]
enum RouteCmd {
    /// [read-only] Every hostname: its tunnel, service, and where DNS really sends it
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        tunnel: Option<String>,
    },
    /// [fleet file + cloudflare] Send a hostname to a service through a tunnel (adds or updates; idempotent)
    Add {
        host: String,
        /// a port on the tunnel's machine (3000) or a URL (http://localhost:3000, ssh://localhost:22)
        service: String,
        #[arg(long)]
        tunnel: String,
        /// a second tunnel that also carries it, for failover
        #[arg(long)]
        standby: Option<String>,
        /// manual (default) or auto
        #[arg(long)]
        failover: Option<String>,
        #[arg(long)]
        note: Option<String>,
        /// only write the fleet file; let the agents carry it out
        #[arg(long)]
        no_apply: bool,
        /// allow taking the hostname from a tunnel serving it now
        #[arg(long)]
        yes: bool,
    },
    /// [fleet file + cloudflare] Stop routing a hostname: its ingress and DNS go
    #[command(alias = "remove")]
    Rm {
        host: String,
        #[arg(long)]
        no_apply: bool,
    },
    /// [fleet file + cloudflare] Rename a hostname, keeping its tunnel and service
    #[command(alias = "rename")]
    Mv {
        old: String,
        new: String,
        #[arg(long)]
        no_apply: bool,
    },
}

#[derive(Subcommand)]
enum TunnelCmd {
    /// [this Mac only] Tunnels this Mac has connector tokens for, and their LaunchAgents
    #[command(alias = "ls")]
    List,
    /// [this Mac only] Start a tunnel here
    Start { name: String },
    /// [this Mac only] Stop a tunnel here (the agent starts it again if the fleet says it runs here)
    Stop { name: String },
    /// [this Mac only] Restart a tunnel here (safe over the ssh it carries)
    Restart { name: String },
    /// [this Mac only] A tunnel's cloudflared logs
    Logs {
        name: String,
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// [this Mac only] Keep a connector token here under a name
    Add {
        name: String,
        #[arg(long)]
        token: String,
    },
    /// [this Mac only] Stop a tunnel here and delete its token and LaunchAgent here. Nothing in Cloudflare changes: the tunnel and its tokens keep working
    Forget { name: String },
    /// [this Mac + cloudflare] DELETE a tunnel in Cloudflare: its DNS, its connections, the tunnel. Every connector token for it stops working
    Destroy {
        tunnel: String,
        #[arg(long)]
        yes: bool,
    },
    /// [this Mac + cloudflare] Give a tunnel a new secret: every old connector token stops working; agents fetch the new one
    Rotate { tunnel: String },
    /// [fleet file + cloudflare] Create a tunnel in Cloudflare and add it to the fleet
    Create {
        alias: String,
        /// the fleet account to create it in
        #[arg(long)]
        account: String,
        /// the machine that should run it
        #[arg(long)]
        machine: Option<String>,
    },
    /// [fleet file + cloudflare] Add an existing Cloudflare tunnel to the fleet
    Adopt {
        /// its id, or its Cloudflare name
        tunnel: String,
        #[arg(long = "as")]
        alias: String,
        #[arg(long)]
        machine: Option<String>,
    },
    /// [fleet file + cloudflare] Change which machine runs a tunnel (--none: no fleet machine)
    Assign {
        tunnel: String,
        #[arg(long, conflicts_with = "none")]
        machine: Option<String>,
        #[arg(long)]
        none: bool,
    },
    /// [fleet file + cloudflare] Rename a tunnel's alias in the fleet file (Cloudflare's name is untouched)
    Rename { old: String, new: String },
    /// [this Mac only] Take in cloudflared LaunchAgents this Mac's config does not know
    ImportPlists,
}

#[derive(Subcommand)]
enum TokenCmd {
    /// [this Mac; reads cloudflare] Keep a Cloudflare API token here, after finding out what it reaches
    Add { token: String },
    /// [this Mac only] The API tokens here, and the accounts and domains each reaches
    #[command(alias = "ls")]
    List,
    /// [this Mac only] Forget an API token, by its number in `token list`
    #[command(alias = "remove")]
    Rm { index: usize },
}

#[derive(Args, Clone)]
struct CfArgs {
    /// an API path; names resolve: {account:<alias>} {zone:<name>} {tunnel:<alias>} {record:<hostname>}
    path: String,
    /// the request body, as JSON
    #[arg(long, conflicts_with = "data_file")]
    data: Option<String>,
    /// the request body, from a file (- for stdin)
    #[arg(long)]
    data_file: Option<String>,
    /// send it; without this a write is only previewed
    #[arg(long)]
    yes: bool,
    /// the account, when the path does not name one (fleet alias or id)
    #[arg(long)]
    account: Option<String>,
    /// allow changing API tokens
    #[arg(long)]
    i_mean_tokens: bool,
    /// allow a write that cannot be undone
    #[arg(long)]
    not_undoable: bool,
}

#[derive(Subcommand)]
enum CfCmd {
    /// [read-only] GET a Cloudflare API path
    #[command(alias = "GET")]
    Get(CfArgs),
    /// [cloudflare] POST to a Cloudflare API path (previewed until --yes; logged; undoable)
    #[command(alias = "POST")]
    Post(CfArgs),
    /// [cloudflare] PUT a Cloudflare API path (previewed until --yes; logged; undoable)
    #[command(alias = "PUT")]
    Put(CfArgs),
    /// [cloudflare] PATCH a Cloudflare API path (previewed until --yes; logged; undoable)
    #[command(alias = "PATCH")]
    Patch(CfArgs),
    /// [cloudflare] DELETE a Cloudflare API path (previewed until --yes; logged; undoable where possible)
    #[command(alias = "DELETE")]
    Delete(CfArgs),
    /// [this Mac only] Changes made through `tunnels cf` on this Mac, newest first
    Log {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// [cloudflare] Put back what a logged change changed (previewed until --yes)
    Undo {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// [this Mac + cloudflare] Run the agent in the foreground (launchd does this)
    Run,
    /// [this Mac + cloudflare] One agent pass, now, in the foreground
    Once,
    /// [this Mac only] Install the agent as a LaunchAgent and start it
    Install,
    /// [this Mac only] Stop the agent and remove its LaunchAgent
    Uninstall,
    /// [this Mac only] Is the agent running, and what has it done lately
    Status,
}

/// Every command's scope, in one place. The dispatcher declares it before
/// running anything, and `a_commands_help_says_its_scope` holds the help
/// text to it.
const SCOPES: &[(&str, Scope)] = &[
    ("status", Scope::ReadOnly),
    ("plan", Scope::ReadOnly),
    ("apply", Scope::LocalAndCloudflare),
    ("doctor", Scope::ReadOnly),
    ("import", Scope::Fleet),
    ("fleet show", Scope::Local),
    ("fleet path", Scope::Local),
    ("fleet validate", Scope::Local),
    ("fleet edit", Scope::Fleet),
    ("fleet history", Scope::Local),
    ("fleet join", Scope::Local),
    ("fleet sync", Scope::Local),
    ("route list", Scope::ReadOnly),
    ("route add", Scope::Fleet),
    ("route rm", Scope::Fleet),
    ("route mv", Scope::Fleet),
    ("promote", Scope::Fleet),
    ("failback", Scope::Fleet),
    ("tunnel list", Scope::Local),
    ("tunnel start", Scope::Local),
    ("tunnel stop", Scope::Local),
    ("tunnel restart", Scope::Local),
    ("tunnel logs", Scope::Local),
    ("tunnel add", Scope::Local),
    ("tunnel forget", Scope::Local),
    ("tunnel destroy", Scope::LocalAndCloudflare),
    ("tunnel rotate", Scope::LocalAndCloudflare),
    ("tunnel create", Scope::Fleet),
    ("tunnel adopt", Scope::Fleet),
    ("tunnel assign", Scope::Fleet),
    ("tunnel rename", Scope::Fleet),
    ("tunnel import-plists", Scope::Local),
    ("token add", Scope::LocalReadsCloudflare),
    ("token list", Scope::Local),
    ("token rm", Scope::Local),
    ("agent run", Scope::LocalAndCloudflare),
    ("agent once", Scope::LocalAndCloudflare),
    ("agent install", Scope::Local),
    ("agent uninstall", Scope::Local),
    ("agent status", Scope::Local),
    ("cf get", Scope::ReadOnly),
    ("cf post", Scope::Cloudflare),
    ("cf put", Scope::Cloudflare),
    ("cf patch", Scope::Cloudflare),
    ("cf delete", Scope::Cloudflare),
    ("cf log", Scope::Local),
    ("cf undo", Scope::Cloudflare),
    ("web", Scope::ReadOnly),
    ("scan", Scope::Local),
];

fn scope_of(path: &str) -> Scope {
    SCOPES.iter().find(|(p, _)| *p == path).map(|(_, s)| *s).unwrap_or(Scope::LocalAndCloudflare)
}

fn cmd_path(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Status { local: true } => "tunnel list".into(),
        Cmd::Status { .. } => "status".into(),
        Cmd::Plan => "plan".into(),
        Cmd::Apply(_) => "apply".into(),
        Cmd::Doctor => "doctor".into(),
        Cmd::Import { .. } => "import".into(),
        Cmd::Fleet(f) => format!(
            "fleet {}",
            match f {
                FleetCmd::Show => "show",
                FleetCmd::Path => "path",
                FleetCmd::Validate => "validate",
                FleetCmd::Edit => "edit",
                FleetCmd::History => "history",
                FleetCmd::Join { .. } => "join",
                FleetCmd::Sync => "sync",
            }
        ),
        Cmd::Route(r) => format!(
            "route {}",
            match r {
                RouteCmd::List { .. } => "list",
                RouteCmd::Add { .. } => "add",
                RouteCmd::Rm { .. } => "rm",
                RouteCmd::Mv { .. } => "mv",
            }
        ),
        Cmd::Promote { .. } => "promote".into(),
        Cmd::Failback { .. } => "failback".into(),
        Cmd::Tunnel(t) => format!(
            "tunnel {}",
            match t {
                TunnelCmd::List => "list",
                TunnelCmd::Start { .. } => "start",
                TunnelCmd::Stop { .. } => "stop",
                TunnelCmd::Restart { .. } => "restart",
                TunnelCmd::Logs { .. } => "logs",
                TunnelCmd::Add { .. } => "add",
                TunnelCmd::Forget { .. } => "forget",
                TunnelCmd::Destroy { .. } => "destroy",
                TunnelCmd::Rotate { .. } => "rotate",
                TunnelCmd::Create { .. } => "create",
                TunnelCmd::Adopt { .. } => "adopt",
                TunnelCmd::Assign { .. } => "assign",
                TunnelCmd::Rename { .. } => "rename",
                TunnelCmd::ImportPlists => "import-plists",
            }
        ),
        Cmd::Token(t) => format!(
            "token {}",
            match t {
                TokenCmd::Add { .. } => "add",
                TokenCmd::List => "list",
                TokenCmd::Rm { .. } => "rm",
            }
        ),
        Cmd::Agent(a) => format!(
            "agent {}",
            match a {
                AgentCmd::Run => "run",
                AgentCmd::Once => "once",
                AgentCmd::Install => "install",
                AgentCmd::Uninstall => "uninstall",
                AgentCmd::Status => "status",
            }
        ),
        Cmd::Cf(c) => format!(
            "cf {}",
            match c {
                CfCmd::Get(_) => "get",
                CfCmd::Post(_) => "post",
                CfCmd::Put(_) => "put",
                CfCmd::Patch(_) => "patch",
                CfCmd::Delete(_) => "delete",
                CfCmd::Log { .. } => "log",
                CfCmd::Undo { .. } => "undo",
            }
        ),
        Cmd::Web { .. } => "web".into(),
        Cmd::Scan => "scan".into(),
        Cmd::List => "tunnel list".into(),
        Cmd::Routes { .. } => "route list".into(),
        Cmd::Start { .. } => "tunnel start".into(),
        Cmd::Stop { .. } => "tunnel stop".into(),
        Cmd::Restart { .. } => "tunnel restart".into(),
        Cmd::Logs { .. } => "tunnel logs".into(),
        Cmd::Add { .. } => "tunnel add".into(),
        Cmd::Rm { .. } => "tunnel forget".into(),
        Cmd::Sync => "status".into(),
        Cmd::Heal => "agent once".into(),
    }
}

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    let cmd = cli.cmd.unwrap_or(Cmd::Status { local: false });
    let path = cmd_path(&cmd);
    scope::enter(scope_of(&path));
    let code = match run(cmd, json) {
        Ok(code) => code,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "error": format!("{e:#}"), "scope": scope_of(&path) }));
            } else {
                eprintln!("✗ {e:#}");
            }
            1
        }
    };
    std::process::exit(code);
}

/// Say where this command acts before it acts. On stderr, so `--json` output stays clean.
fn announce(what: &str) {
    let path_scope = current_scope_tag();
    eprintln!("{path_scope} {what}");
}

fn current_scope_tag() -> &'static str {
    // the scope was declared in main; recover it from the command table via argv
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).map(|a| a.to_ascii_lowercase()).collect();
    for n in [2, 1] {
        if args.len() >= n {
            let p = args[..n].join(" ");
            if let Some((_, s)) = SCOPES.iter().find(|(x, _)| *x == p) {
                return s.tag();
            }
        }
    }
    Scope::LocalAndCloudflare.tag()
}

fn print_json(v: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

fn run(cmd: Cmd, json: bool) -> Result<i32> {
    match cmd {
        Cmd::Status { local: true } | Cmd::List => tunnel_list(json),
        Cmd::Status { local: false } | Cmd::Sync => status_cmd(json, false),
        Cmd::Doctor => status_cmd(json, true),
        Cmd::Plan => plan_cmd(json),
        Cmd::Apply(a) => apply_cmd(a, json),
        Cmd::Import { machine, host, dry_run } => import_cmd(machine, host, dry_run, json),
        Cmd::Fleet(f) => fleet_cmd(f, json),
        Cmd::Route(r) => route_cmd(r, json),
        Cmd::Routes { tunnel } => route_cmd(RouteCmd::List { tunnel }, json),
        Cmd::Promote { host } => switch_cmd(&host, true, json),
        Cmd::Failback { host } => switch_cmd(&host, false, json),
        Cmd::Tunnel(t) => tunnel_cmd(t, json),
        Cmd::Start { name } => tunnel_cmd(TunnelCmd::Start { name }, json),
        Cmd::Stop { name } => tunnel_cmd(TunnelCmd::Stop { name }, json),
        Cmd::Restart { name } => tunnel_cmd(TunnelCmd::Restart { name }, json),
        Cmd::Logs { name, lines } => tunnel_cmd(TunnelCmd::Logs { name, lines }, json),
        Cmd::Add { name, token } => tunnel_cmd(TunnelCmd::Add { name, token }, json),
        Cmd::Rm { name } => {
            // `rm` said "Delete a tunnel" and only forgot it here. It is gone
            // rather than reworded: the two things it could mean now have
            // names that cannot be mistaken for each other.
            let n = name.unwrap_or_else(|| "<name>".into());
            bail!(
                "`tunnels rm` is gone — it never deleted anything in Cloudflare. Say which you mean:\n  \
                 tunnels tunnel forget {n}     [this Mac only] stop it here; the tunnel and its tokens keep working\n  \
                 tunnels tunnel destroy {n}    [this Mac + cloudflare] delete it in Cloudflare; its tokens stop working"
            )
        }
        Cmd::Token(t) => token_cmd(t, json),
        Cmd::Agent(a) => agent_cmd(a, json),
        Cmd::Heal => agent_cmd(AgentCmd::Once, json),
        Cmd::Cf(c) => cf_cmd(c, json),
        Cmd::Web { open } => web_cmd(open, json),
        Cmd::Scan => scan_cmd(json),
    }
}

// ---------------------------------------------------------------- context

struct Ctx {
    config: Config,
    fleet: Option<Fleet>,
    me: String,
}

fn ctx() -> Result<Ctx> {
    let config = Config::load()?;
    let fleet = Fleet::load()?;
    let me = fleet::this_machine(&config, fleet.as_ref());
    Ok(Ctx { config, fleet, me })
}

/// Before changing the fleet file, take the newest copy the peers have, so
/// this edit builds on it rather than racing it.
fn pull_first(me: &str) {
    match sync::pull(me, &[], Duration::from_secs(3)) {
        Ok(Some((host, serial))) => eprintln!("  (took fleet serial {serial} from {host} first)"),
        _ => {}
    }
}

fn observe_all(config: &Config) -> Snapshot {
    observe::observe(config, &Want::default())
}

fn fleet_or_default(c: &Ctx) -> Fleet {
    c.fleet.clone().unwrap_or_default()
}

// ---------------------------------------------------------------- status / plan / apply

fn status_cmd(json: bool, problems_only: bool) -> Result<i32> {
    let c = ctx()?;
    let fleet = fleet_or_default(&c);
    let snap = observe_all(&c.config);
    let local = observe::observe_local(&c.config, &c.me);
    let mut p = plan::plan(&fleet, &snap, Some(&local));
    p.findings.extend(plan::probe_origins(&fleet, &c.me));
    let st = status::build(&fleet, &snap, Some(&local), p, &c.me);
    if json {
        if problems_only {
            print_json(&serde_json::json!({ "scope": Scope::ReadOnly, "findings": st.plan.findings, "errors": st.errors }))?;
        } else {
            print_json(&st)?;
        }
        return Ok(0);
    }
    if c.fleet.is_none() {
        eprintln!("(no fleet file here yet, so everything shows as not in the fleet — `tunnels import` writes one)\n");
    }
    if problems_only {
        let mut p = st.plan.clone();
        p.actions.clear();
        let text = status::render_plan(&p);
        print!("{}", text.replace("\nin line with the fleet file — nothing to do\n", ""));
        if st.plan.findings.is_empty() {
            println!("no problems found");
        }
        for e in &st.errors {
            println!("  ! {e}");
        }
    } else {
        print!("{}", status::render(&st));
    }
    Ok(0)
}

fn plan_cmd(json: bool) -> Result<i32> {
    let c = ctx()?;
    let fleet = Fleet::load_required()?;
    let snap = observe_all(&c.config);
    let local = observe::observe_local(&c.config, &c.me);
    let p = plan::plan(&fleet, &snap, Some(&local));
    if json {
        print_json(&serde_json::json!({ "scope": Scope::ReadOnly, "serial": fleet.serial, "plan": p, "errors": snap.errors }))?;
    } else {
        print!("{}", status::render_plan(&p));
        for e in &snap.errors {
            println!("  ! {e}");
        }
    }
    Ok(if p.actions.is_empty() { 0 } else { 2 })
}

fn print_report(r: &apply::Report, json: bool) -> Result<i32> {
    if json {
        print_json(&serde_json::json!({ "scope": Scope::LocalAndCloudflare, "report": r }))?;
    } else {
        for o in &r.done {
            println!("  {} {} — {}", if o.ok { "✓" } else { "✗" }, o.summary, o.detail);
        }
        for (s, why) in &r.held {
            println!("  · held: {s} ({why})");
        }
        if r.done.is_empty() && r.held.is_empty() {
            println!("  nothing to do");
        }
    }
    Ok(if r.failed() > 0 { 1 } else { 0 })
}

fn apply_cmd(a: ApplyArgs, json: bool) -> Result<i32> {
    let mut c = ctx()?;
    let fleet = Fleet::load_required()?;
    announce(&format!("applying fleet serial {}", fleet.serial));
    let snap = observe_all(&c.config);
    let local = observe::observe_local(&c.config, &c.me);
    let p = plan::plan(&fleet, &snap, Some(&local));
    let opts = Options {
        prune: a.prune,
        yes: a.yes,
        allow_destroy: a.allow_destroy,
        only_hosts: if a.hosts.is_empty() { None } else { Some(a.hosts.clone()) },
        no_local: a.no_local,
        only_owner: None,
    };
    let r = apply::apply(&p, &snap, &mut c.config, &opts);
    // a tunnel destroyed in Cloudflare leaves the fleet file too
    let destroyed: Vec<String> = p
        .actions
        .iter()
        .filter(|x| x.destroy)
        .filter(|x| r.done.iter().any(|o| o.ok && o.summary == x.summary))
        .filter_map(|x| match &x.kind {
            plan::Kind::Destroy { tunnel, .. } => Some(tunnel.clone()),
            _ => None,
        })
        .collect();
    if !destroyed.is_empty() {
        let f = Fleet::edit(&c.me, |f| {
            for t in &destroyed {
                remove_tunnel_from_fleet(f, t);
            }
            Ok(())
        })?;
        sync::notify(&f, &c.me);
    }
    print_report(&r, json)
}

fn remove_tunnel_from_fleet(f: &mut Fleet, alias: &str) {
    f.tunnels.remove(alias);
    f.routes.retain(|r| r.tunnel != alias);
    for r in &mut f.routes {
        if r.standby.as_deref() == Some(alias) {
            r.standby = None;
            r.failover = None;
            r.active = None;
        }
    }
}

/// Apply only what concerns these hostnames, right after a fleet edit.
fn apply_hosts(c: &mut Ctx, hosts: &[String], yes: bool, prune: bool, json: bool) -> Result<i32> {
    let fleet = Fleet::load_required()?;
    let snap = observe_all(&c.config);
    let p = plan::plan(&fleet, &snap, None);
    let opts = Options { yes, prune, only_hosts: Some(hosts.to_vec()), no_local: true, ..Default::default() };
    let r = apply::apply(&p, &snap, &mut c.config, &opts);
    let code = print_report(&r, json)?;
    if !json && r.held.iter().any(|(_, w)| w.contains("--yes")) {
        println!("\n  rerun with --yes to take it over — or pick another hostname");
    }
    if !json && r.failed() > 0 {
        println!("\n  the fleet file has the change; the agent that owns it will keep trying");
    }
    Ok(code)
}

// ---------------------------------------------------------------- import

/// A short name for an account: its first domain without the TLD.
fn account_alias(zones: &[String], name: &str, taken: &BTreeSet<String>) -> String {
    let base = zones
        .first()
        .map(|z| z.rsplit_once('.').map(|(a, _)| a).unwrap_or(z).replace('.', "-"))
        .unwrap_or_else(|| name.split(['@', ' ', '\'']).next().unwrap_or("account").to_ascii_lowercase());
    let mut alias = base.clone();
    let mut n = 2;
    while taken.contains(&alias) {
        alias = format!("{base}-{n}");
        n += 1;
    }
    alias
}

fn import_cmd(machine: Option<String>, host: Option<String>, dry_run: bool, json: bool) -> Result<i32> {
    let mut c = ctx()?;
    if let Some(m) = &machine {
        if !dry_run {
            c.config.set_machine(m)?;
        }
        c.me = m.clone();
    }
    if c.fleet.is_some() && !dry_run {
        pull_first(&c.me);
    }
    let me = c.me.clone();
    let snap = observe_all(&c.config);
    for e in &snap.errors {
        eprintln!("  ! {e}");
    }
    let mut notes: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();

    let build = |f: &mut Fleet, notes: &mut Vec<String>, added: &mut Vec<String>| -> Result<()> {
        let my_host = host.clone().unwrap_or_else(util::short_hostname);
        if !f.machines.contains_key(&me) {
            f.machines.insert(me.clone(), fleet::Machine { host: my_host.clone(), note: String::new() });
            added.push(format!("machine {me} (reached at {my_host})"));
        }
        // accounts every token here reaches
        for a in snap.accounts.iter().filter(|a| a.reachable || !a.zones.is_empty()) {
            if f.account_alias_for_id(&a.id).is_some() {
                // keep its zone list current
                let alias = f.account_alias_for_id(&a.id).unwrap().clone();
                let acct = f.accounts.get_mut(&alias).unwrap();
                for z in &a.zones {
                    if !acct.zones.contains(z) {
                        acct.zones.push(z.clone());
                        acct.zones.sort();
                    }
                }
                continue;
            }
            let taken: BTreeSet<String> = f.accounts.keys().cloned().collect();
            let alias = account_alias(&a.zones, &a.name, &taken);
            f.accounts.insert(alias.clone(), fleet::Account { id: a.id.clone(), name: a.name.clone(), zones: a.zones.clone() });
            added.push(format!("account {alias} ({})", a.zones.join(", ")));
        }
        // the tunnels this Mac runs
        for t in &c.config.tunnels {
            let Some(id) = t.tunnel_id() else { continue };
            if let Some(alias) = f.alias_for_id(&id) {
                let decl = &f.tunnels[alias];
                if decl.machine.as_deref() != Some(me.as_str()) {
                    notes.push(format!(
                        "{} ({}) is here, but the fleet runs it as {alias} on {} — left as is",
                        t.name,
                        short(&id),
                        decl.machine.as_deref().unwrap_or("no machine")
                    ));
                }
                continue;
            }
            let Some(obs) = snap.tunnel(&id) else {
                notes.push(format!("{} ({}): Cloudflare does not show this tunnel (deleted, or no token here reaches its account) — skipped", t.name, short(&id)));
                continue;
            };
            let Some(acct) = f.account_alias_for_id(&obs.account_id).cloned() else { continue };
            let base = format!("{me}-{acct}").to_ascii_lowercase();
            let mut alias = base.clone();
            let mut n = 2;
            while f.find_tunnel(&alias).is_some() {
                alias = format!("{base}-{n}");
                n += 1;
            }
            f.tunnels.insert(
                alias.clone(),
                TunnelDecl {
                    id: id.clone(),
                    account: acct,
                    machine: Some(me.clone()),
                    note: format!("Cloudflare name {:?}; local name {:?}", obs.tunnel.name, t.name),
                    destroy: false,
                },
            );
            added.push(format!("tunnel {alias} = {} ({}) on {me}", obs.tunnel.name, short(&id)));
        }
        // routes: a hostname whose ingress and DNS agree on a fleet tunnel
        let ids: Vec<(String, String)> = f.tunnels.iter().map(|(a, t)| (a.clone(), t.id.clone())).collect();
        for (alias, id) in ids {
            let Some(obs) = snap.tunnel(&id) else { continue };
            for (host, service) in obs.routes() {
                if f.find_route(&host).is_some() {
                    continue;
                }
                let dns = snap.dns_for(&host);
                let target = dns.first().and_then(|r| r.tunnel_target());
                match target {
                    Some(t) if t.eq_ignore_ascii_case(&id) => {
                        f.routes.push(Route { host: host.clone(), tunnel: alias.clone(), service: service.clone(), ..Default::default() });
                        added.push(format!("route {host} → {service} on {alias}"));
                    }
                    Some(t) => {
                        let other = f
                            .alias_for_id(&t)
                            .cloned()
                            .or_else(|| snap.tunnel(&t).map(|o| format!("{} ({})", o.tunnel.name, short(&t))))
                            .unwrap_or_else(|| format!("{} (a tunnel that no longer exists)", short(&t)));
                        notes.push(format!(
                            "{host} is in {alias}'s ingress but DNS sends it to {other} — not imported here{}",
                            if f.alias_for_id(&t).is_some()
                                && f.account_for_host(&host).map(|(a, _)| a) == f.tunnels.get(&alias).map(|x| &x.account)
                            {
                                format!("; to keep {alias} as a warm standby: tunnels route add {host} {service} --tunnel {other} --standby {alias}")
                            } else {
                                String::new()
                            }
                        ));
                    }
                    None if snap.dns_known_for(&host) => {
                        notes.push(format!("{host} is in {alias}'s ingress but has no DNS — not imported (a route nothing reaches)"))
                    }
                    None => notes.push(format!("{host}: no token here can see its zone's DNS — not imported")),
                }
            }
        }
        f.routes.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(())
    };

    if dry_run {
        let mut f = c.fleet.clone().unwrap_or_default();
        build(&mut f, &mut notes, &mut added)?;
        let problems = f.validate();
        if json {
            print_json(&serde_json::json!({ "scope": Scope::Fleet, "dry_run": true, "added": added, "notes": notes, "problems": problems }))?;
        } else {
            for a in &added {
                println!("  + {a}");
            }
            for n in &notes {
                println!("  · {n}");
            }
            for p in &problems {
                println!("  ✗ {p}");
            }
            println!("\n(dry run — nothing written)");
        }
        return Ok(0);
    }
    announce(&format!("importing {me} into {}", Fleet::path().display()));
    let f = Fleet::edit(&me, |f| build(f, &mut notes, &mut added))?;
    sync::notify(&f, &me);
    if json {
        print_json(&serde_json::json!({ "scope": Scope::Fleet, "serial": f.serial, "added": added, "notes": notes }))?;
    } else {
        for a in &added {
            println!("  + {a}");
        }
        for n in &notes {
            println!("  · {n}");
        }
        if added.is_empty() {
            println!("  nothing new — the fleet already had everything here");
        }
        println!("\nfleet serial {} written to {}", f.serial, Fleet::path().display());
        println!("next: `tunnels plan` to see what differs, `tunnels agent install` to keep this Mac in line");
    }
    Ok(0)
}

// ---------------------------------------------------------------- fleet

fn fleet_cmd(cmd: FleetCmd, json: bool) -> Result<i32> {
    match cmd {
        FleetCmd::Show => {
            let f = Fleet::load_required()?;
            if json {
                print_json(&f)?;
            } else {
                print!("{}", std::fs::read_to_string(Fleet::path())?);
            }
        }
        FleetCmd::Path => println!("{}", Fleet::path().display()),
        FleetCmd::Validate => {
            let f = Fleet::load_required()?;
            let problems = f.validate();
            if json {
                print_json(&serde_json::json!({ "serial": f.serial, "valid": problems.is_empty(), "problems": problems }))?;
            } else if problems.is_empty() {
                println!("✓ fleet serial {} is valid", f.serial);
            } else {
                for p in &problems {
                    println!("✗ {p}");
                }
            }
            return Ok(if problems.is_empty() { 0 } else { 1 });
        }
        FleetCmd::Edit => {
            let c = ctx()?;
            pull_first(&c.me);
            let before = Fleet::load_required()?;
            let tmp = std::env::temp_dir().join(format!("fleet-{}.toml", std::process::id()));
            std::fs::write(&tmp, before.to_toml())?;
            let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
            loop {
                let st = std::process::Command::new("sh").args(["-c", &format!("{editor} \"$1\""), "sh"]).arg(&tmp).status()?;
                if !st.success() {
                    bail!("the editor exited with {st}; nothing saved");
                }
                let text = std::fs::read_to_string(&tmp)?;
                match Fleet::parse(&text).map(|f| (f.validate(), f)) {
                    Ok((p, edited)) if p.is_empty() => {
                        if edited == before {
                            println!("no changes");
                            return Ok(0);
                        }
                        let f = Fleet::edit(&c.me, |f| {
                            let serial = f.serial;
                            *f = edited;
                            f.serial = serial;
                            Ok(())
                        })?;
                        sync::notify(&f, &c.me);
                        println!("✓ fleet serial {} saved and shared — `tunnels plan` to see what it changes", f.serial);
                        return Ok(0);
                    }
                    Ok((p, _)) => eprintln!("✗ not valid:\n  {}", p.join("\n  ")),
                    Err(e) => eprintln!("✗ {e:#}"),
                }
                if !std::io::stdin().is_terminal() || !ask("edit again? [Y/n] ", true) {
                    bail!("nothing saved (your edit is in {})", tmp.display());
                }
            }
        }
        FleetCmd::History => {
            let dir = Fleet::path().with_file_name("fleet.history");
            let mut files: Vec<_> = std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            files.sort();
            for p in files.iter().rev() {
                if let Ok(f) = std::fs::read_to_string(p).map_err(anyhow::Error::from).and_then(|t| Fleet::parse(&t)) {
                    println!("serial {:>4}  {}  by {}   {}", f.serial, f.updated_at, f.updated_by, p.display());
                }
            }
        }
        FleetCmd::Join { host, machine, port } => {
            let f = sync::fetch(&host, port, Duration::from_secs(8))?;
            if let Some(existing) = Fleet::load()? {
                if !f.newer_than(&existing) {
                    println!("this machine already has fleet serial {} (theirs: {}) — kept", existing.serial, f.serial);
                    return Ok(0);
                }
            }
            if let Some(m) = &machine {
                let mut c = Config::load()?;
                c.set_machine(m)?;
            }
            f.save()?;
            println!("✓ took fleet serial {} from {host}", f.serial);
            println!("next: `tunnels import{}` to add this Mac's tunnels, then `tunnels agent install`", machine.map(|m| format!(" --machine {m}")).unwrap_or_default());
        }
        FleetCmd::Sync => {
            let c = ctx()?;
            match sync::pull(&c.me, &[], Duration::from_secs(5))? {
                Some((h, s)) => println!("✓ took fleet serial {s} from {h}"),
                None => println!("this machine's copy is the newest it can find"),
            }
            if let Some(f) = Fleet::load()? {
                sync::notify(&f, &c.me);
            }
        }
    }
    Ok(0)
}

fn ask(prompt: &str, default_yes: bool) -> bool {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return false;
    }
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() { default_yes } else { s == "y" || s == "yes" }
}

// ---------------------------------------------------------------- routes

fn route_cmd(cmd: RouteCmd, json: bool) -> Result<i32> {
    match cmd {
        RouteCmd::List { tunnel } => {
            let c = ctx()?;
            let fleet = fleet_or_default(&c);
            let snap = observe_all(&c.config);
            let st = status::build(&fleet, &snap, None, plan::Plan::default(), &c.me);
            let rows: Vec<serde_json::Value> = st
                .tunnels
                .iter()
                .filter(|t| match &tunnel {
                    None => true,
                    Some(k) => {
                        t.alias.as_deref().map(|a| a.eq_ignore_ascii_case(k)).unwrap_or(false)
                            || t.id.starts_with(&k.to_ascii_lowercase())
                            || t.cf_name.eq_ignore_ascii_case(k)
                    }
                })
                .flat_map(|t| {
                    t.routes.iter().map(move |r| {
                        serde_json::json!({
                            "host": r.host, "service": r.service,
                            "tunnel": t.alias.clone().unwrap_or_else(|| t.cf_name.clone()),
                            "tunnel_id": t.id, "machine": t.machine, "role": r.role, "active": r.active,
                            "dns": r.dns, "dns_target": r.dns_target, "tunnel_state": t.state,
                        })
                    })
                })
                .collect();
            if json {
                print_json(&rows)?;
            } else {
                println!("{:<40} {:<30} {:<22} {:<8} {}", "HOSTNAME", "SERVICE", "TUNNEL", "ROLE", "DNS");
                for r in &rows {
                    let dns = match (r["dns"].as_str().unwrap_or(""), r["dns_target"].as_str()) {
                        ("elsewhere", Some(t)) => format!("→ {t}"),
                        (d, _) => d.to_string(),
                    };
                    println!(
                        "{:<40} {:<30} {:<22} {:<8} {}",
                        r["host"].as_str().unwrap_or(""),
                        r["service"].as_str().unwrap_or(""),
                        r["tunnel"].as_str().unwrap_or(""),
                        r["role"].as_str().unwrap_or(""),
                        dns
                    );
                }
            }
            Ok(0)
        }
        RouteCmd::Add { host, service, tunnel, standby, failover, note, no_apply, yes } => {
            let mut c = ctx()?;
            if c.fleet.is_none() {
                bail!("no fleet file here — `tunnels import` first (it takes a minute and writes down what exists)");
            }
            pull_first(&c.me);
            let host = host.to_ascii_lowercase();
            let service = fleet::normalize_service(&service);
            let failover = match failover.as_deref() {
                None => None,
                Some("manual") => Some(fleet::Failover::Manual),
                Some("auto") => Some(fleet::Failover::Auto),
                Some(x) => bail!("--failover is manual or auto, not {x}"),
            };
            let mut what = String::new();
            let f = Fleet::edit(&c.me, |f| {
                let alias = f
                    .find_tunnel(&tunnel)
                    .map(|(a, _)| a.clone())
                    .ok_or_else(|| anyhow!("no tunnel `{tunnel}` in the fleet — `tunnels status` lists them; `tunnels tunnel adopt` adds one"))?;
                let sb = match &standby {
                    Some(s) => Some(f.find_tunnel(s).map(|(a, _)| a.clone()).ok_or_else(|| anyhow!("no tunnel `{s}` in the fleet"))?),
                    None => None,
                };
                match f.find_route_mut(&host) {
                    Some(r) => {
                        if r.tunnel != alias {
                            what = format!("moving {host} from {} to {alias}", r.tunnel);
                            r.active = None;
                        } else {
                            what = format!("updating {host}");
                        }
                        r.tunnel = alias;
                        r.service = service.clone();
                        if sb.is_some() {
                            r.standby = sb;
                        }
                        if failover.is_some() {
                            r.failover = failover;
                        }
                        if let Some(n) = &note {
                            r.note = n.clone();
                        }
                    }
                    None => {
                        what = format!("{host} → {service} on {alias}");
                        f.routes.push(Route {
                            host: host.clone(),
                            tunnel: alias,
                            service: service.clone(),
                            standby: sb,
                            failover,
                            active: None,
                            note: note.clone().unwrap_or_default(),
                        });
                        f.routes.sort_by(|a, b| a.host.cmp(&b.host));
                    }
                }
                Ok(())
            })?;
            announce(&what);
            eprintln!("  fleet serial {}", f.serial);
            sync::notify(&f, &c.me);
            if no_apply {
                println!("✓ in the fleet file; the agent on {} will carry it out", f.find_route(&host).and_then(|r| f.machine_of(&r.tunnel)).unwrap_or("its machine"));
                return Ok(0);
            }
            apply_hosts(&mut c, &[host], yes, false, json)
        }
        RouteCmd::Rm { host, no_apply } => {
            let mut c = ctx()?;
            pull_first(&c.me);
            let f = Fleet::edit(&c.me, |f| {
                let before = f.routes.len();
                f.routes.retain(|r| !r.host.eq_ignore_ascii_case(&host));
                if f.routes.len() == before {
                    bail!("{host} is not in the fleet file");
                }
                Ok(())
            })?;
            announce(&format!("removing {host}"));
            sync::notify(&f, &c.me);
            if no_apply {
                println!("✓ out of the fleet file; `tunnels apply --prune --host {host}` removes its ingress and DNS");
                return Ok(0);
            }
            // removing it is exactly what was asked, so prune it
            apply_hosts(&mut c, &[host], false, true, json)
        }
        RouteCmd::Mv { old, new, no_apply } => {
            let mut c = ctx()?;
            pull_first(&c.me);
            let new = new.to_ascii_lowercase();
            let f = Fleet::edit(&c.me, |f| {
                if f.find_route(&new).is_some() {
                    bail!("{new} is already routed");
                }
                let r = f.find_route_mut(&old).ok_or_else(|| anyhow!("{old} is not in the fleet file"))?;
                r.host = new.clone();
                f.routes.sort_by(|a, b| a.host.cmp(&b.host));
                Ok(())
            })?;
            announce(&format!("renaming {old} → {new}"));
            sync::notify(&f, &c.me);
            if no_apply {
                return Ok(0);
            }
            // the new name first; only once it is in place does the old one go
            let code = apply_hosts(&mut c, &[new.clone()], false, false, json)?;
            if code != 0 {
                println!("  {old} left in place, since {new} did not come up");
                return Ok(code);
            }
            apply_hosts(&mut c, &[old], false, true, json)
        }
    }
}

fn switch_cmd(host: &str, promote: bool, json: bool) -> Result<i32> {
    let c = ctx()?;
    pull_first(&c.me);
    announce(&format!("{} {host}", if promote { "promoting to standby:" } else { "failing back to primary:" }));
    let r = web::switch(host, promote)?;
    print_report(&r, json)
}

// ---------------------------------------------------------------- tunnels

/// A tunnel named any way a person might name it: fleet alias, id or id
/// prefix, this Mac's local name, or its Cloudflare name (when that is
/// unambiguous — two tunnels called DorkyRobot2 in two accounts is real).
struct Resolved {
    alias: Option<String>,
    id: String,
    account_id: String,
    cf_name: String,
    local: Option<String>,
}

fn resolve(key: &str, c: &Ctx, snap: &Snapshot) -> Result<Resolved> {
    let fleet = fleet_or_default(c);
    let local_of = |id: &str| c.config.tunnel_by_id(id).map(|t| t.name.clone());
    if let Some((alias, t)) = fleet.find_tunnel(key) {
        let obs = snap.tunnel(&t.id);
        return Ok(Resolved {
            alias: Some(alias.clone()),
            id: t.id.clone(),
            account_id: obs.map(|o| o.account_id.clone()).or_else(|| fleet.accounts.get(&t.account).map(|a| a.id.clone())).unwrap_or_default(),
            cf_name: obs.map(|o| o.tunnel.name.clone()).unwrap_or_default(),
            local: local_of(&t.id),
        });
    }
    if let Some(t) = c.config.tunnel_by_name(key) {
        let id = t.tunnel_id().ok_or_else(|| anyhow!("{key}'s connector token does not decode"))?;
        return Ok(Resolved {
            alias: fleet.alias_for_id(&id).cloned(),
            account_id: t.account_id().unwrap_or_default(),
            cf_name: snap.tunnel(&id).map(|o| o.tunnel.name.clone()).unwrap_or_default(),
            local: Some(t.name.clone()),
            id,
        });
    }
    let hits: Vec<_> = snap
        .tunnels
        .iter()
        .filter(|o| o.tunnel.id.eq_ignore_ascii_case(key) || (key.len() >= 8 && o.tunnel.id.starts_with(&key.to_ascii_lowercase())) || o.tunnel.name.eq_ignore_ascii_case(key))
        .collect();
    match hits.len() {
        1 => Ok(Resolved {
            alias: fleet.alias_for_id(&hits[0].tunnel.id).cloned(),
            id: hits[0].tunnel.id.clone(),
            account_id: hits[0].account_id.clone(),
            cf_name: hits[0].tunnel.name.clone(),
            local: local_of(&hits[0].tunnel.id),
        }),
        0 => bail!("no tunnel `{key}` — not a fleet alias, not a local name, and no Cloudflare tunnel by that name or id is visible from here"),
        _ => bail!(
            "`{key}` names {} tunnels; use an id:\n  {}",
            hits.len(),
            hits.iter().map(|o| format!("{} ({}) in account {}", o.tunnel.name, o.tunnel.id, short(&o.account_id))).collect::<Vec<_>>().join("\n  ")
        ),
    }
}

/// A tunnel on this Mac, by local name or by fleet alias/id (no Cloudflare).
fn local_name(key: &str, c: &Ctx) -> Result<String> {
    if let Some(t) = c.config.tunnel_by_name(key) {
        return Ok(t.name.clone());
    }
    if let Some((_, t)) = c.fleet.as_ref().and_then(|f| f.find_tunnel(key)) {
        if let Some(l) = c.config.tunnel_by_id(&t.id) {
            return Ok(l.name.clone());
        }
    }
    if let Some(l) = c.config.tunnels.iter().find(|t| t.tunnel_id().map(|i| i.starts_with(&key.to_ascii_lowercase())).unwrap_or(false) && key.len() >= 8) {
        return Ok(l.name.clone());
    }
    bail!("this Mac has no tunnel `{key}` — `tunnels tunnel list` shows the ones it has")
}

fn tunnel_list(json: bool) -> Result<i32> {
    let c = ctx()?;
    let local = observe::observe_local(&c.config, &c.me);
    let fleet = c.fleet.clone();
    let alias_of = |id: &Option<String>| id.as_ref().and_then(|i| fleet.as_ref().and_then(|f| f.alias_for_id(i).cloned()));
    if json {
        let rows: Vec<serde_json::Value> = local
            .tunnels
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name, "status": if t.state == "loaded" { if t.pid.is_some() { "running" } else { "loaded" } } else if t.state == "not loaded" { "stopped" } else { "inactive" },
                    "pid": t.pid, "tunnel_id": t.tunnel_id, "account_id": t.account_id, "fleet_alias": alias_of(&t.tunnel_id),
                    "label": t.label, "token_in_plist": t.inline_token,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "scope": Scope::Local, "machine": c.me, "agent_loaded": local.agent_loaded, "tunnels": rows, "stray_plists": local.stray_plists }))?;
        return Ok(0);
    }
    println!("{:<18} {:<11} {:<7} {:<10} {:<22} {}", "NAME", "STATE", "PID", "TUNNEL", "FLEET ALIAS", "TOKEN");
    for t in &local.tunnels {
        println!(
            "{:<18} {:<11} {:<7} {:<10} {:<22} {}",
            t.name,
            t.state,
            t.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            t.tunnel_id.as_deref().map(short).unwrap_or("?"),
            alias_of(&t.tunnel_id).unwrap_or_else(|| "(not in fleet)".into()),
            if t.inline_token { "in plist" } else if t.state == "no plist" { "-" } else { "file" }
        );
    }
    if local.tunnels.is_empty() {
        println!("(none)");
    }
    for s in &local.stray_plists {
        println!("  · {} has a LaunchAgent here but is not in this Mac's config (`tunnels tunnel import-plists`)", s);
    }
    println!("\nmachine {} · agent {}", c.me, if local.agent_loaded { "running" } else { "not installed (`tunnels agent install`)" });
    Ok(0)
}

fn tunnel_cmd(cmd: TunnelCmd, json: bool) -> Result<i32> {
    let done = |msg: String| -> Result<i32> {
        if json {
            print_json(&serde_json::json!({ "scope": scope_of(&current_path()), "ok": true, "message": msg }))?;
        } else {
            println!("✓ {msg}");
        }
        Ok(0)
    };
    match cmd {
        TunnelCmd::List => tunnel_list(json),
        TunnelCmd::Start { name } => {
            let c = ctx()?;
            let n = local_name(&name, &c)?;
            announce(&format!("start {n}"));
            let t = c.config.tunnel_by_name(&n).unwrap();
            launchd::start(&n, &t.token)?;
            done(format!("started {n}"))
        }
        TunnelCmd::Stop { name } => {
            let c = ctx()?;
            let n = local_name(&name, &c)?;
            announce(&format!("stop {n}"));
            launchd::stop(&n)?;
            let runs_here = c
                .config
                .tunnel_by_name(&n)
                .and_then(|t| t.tunnel_id())
                .and_then(|id| c.fleet.as_ref().and_then(|f| f.alias_for_id(&id).map(|a| f.tunnels[a].machine.clone())))
                .flatten()
                .map(|m| m == c.me)
                .unwrap_or(false);
            done(format!(
                "stopped {n}{}",
                if runs_here { " — the fleet says it runs here, so the agent will start it again; `tunnels tunnel assign` changes that" } else { "" }
            ))
        }
        TunnelCmd::Restart { name } => {
            let c = ctx()?;
            let n = local_name(&name, &c)?;
            announce(&format!("restart {n}"));
            let t = c.config.tunnel_by_name(&n).unwrap();
            if launchd::is_loaded_name(&n) && launchd::plist_runs_token(&n, &t.token) && !launchd::plist_has_inline_token(&n) {
                launchd::kickstart(&n)?;
            } else {
                // writes the plist fresh (token in a file) — detached when over ssh
                launchd::restart(&n, &t.token)?;
            }
            done(format!("restarted {n}"))
        }
        TunnelCmd::Logs { name, lines } => {
            let c = ctx()?;
            let n = local_name(&name, &c)?;
            print!("{}", launchd::read_logs(&n, lines)?);
            Ok(0)
        }
        TunnelCmd::Add { name, token } => {
            let mut c = ctx()?;
            announce(&format!("keep a connector token here as {name}"));
            c.config.add(name.clone(), token)?;
            done(format!("added {name} — `tunnels tunnel start {name}` runs it"))
        }
        TunnelCmd::Forget { name } => {
            let mut c = ctx()?;
            let n = local_name(&name, &c)?;
            let id = c.config.tunnel_by_name(&n).and_then(|t| t.tunnel_id()).unwrap_or_default();
            announce(&format!("forget {n} on this Mac"));
            apply::forget_local(&mut c.config, &n)?;
            let assigned_here = c
                .fleet
                .as_ref()
                .and_then(|f| f.alias_for_id(&id).map(|a| (a.clone(), f.tunnels[a].machine.clone())))
                .filter(|(_, m)| m.as_deref() == Some(c.me.as_str()));
            let msg = format!(
                "forgot {n} on this Mac: stopped, LaunchAgent and token removed here.\n  \
                 Tunnel {id} STILL EXISTS in Cloudflare and every connector token for it STILL WORKS.\n  \
                 To delete it there: tunnels tunnel destroy {id}{}",
                match assigned_here {
                    Some((a, _)) => format!(
                        "\n  ⚠ the fleet says this Mac runs {a}, so the agent will fetch its token and start it again. \
                         `tunnels tunnel assign {a} --none` (or --machine <other>) first."
                    ),
                    None => String::new(),
                }
            );
            if json {
                print_json(&serde_json::json!({ "scope": Scope::Local, "forgot": n, "tunnel_id": id, "still_exists_in_cloudflare": true, "tokens_still_work": true }))?;
                Ok(0)
            } else {
                println!("✓ {msg}");
                Ok(0)
            }
        }
        TunnelCmd::Destroy { tunnel, yes } => destroy_cmd(&tunnel, yes, json),
        TunnelCmd::Rotate { tunnel } => rotate_cmd(&tunnel, json),
        TunnelCmd::Create { alias, account, machine } => {
            let mut c = ctx()?;
            let fleet = Fleet::load_required()?;
            pull_first(&c.me);
            let acct = fleet.accounts.get(&account).ok_or_else(|| anyhow!("no account `{account}` in the fleet ({})", fleet.accounts.keys().cloned().collect::<Vec<_>>().join(", ")))?;
            if fleet.find_tunnel(&alias).is_some() {
                bail!("there is already a tunnel `{alias}`");
            }
            if let Some(m) = &machine {
                if !fleet.machines.contains_key(m) {
                    bail!("no machine `{m}` in the fleet");
                }
            }
            let snap = observe::observe(&c.config, &Want { ingress_for: Some(BTreeSet::new()), dns_zones: Some(BTreeSet::new()), accounts: Some([acct.id.clone()].into()) });
            let client = snap.client_for_account(&acct.id).ok_or_else(|| anyhow!("no API token here reaches account {account}"))?;
            announce(&format!("create tunnel {alias} in {account}"));
            let t = client.create_tunnel(&acct.id, &alias)?;
            let f = Fleet::edit(&c.me, |f| {
                f.tunnels.insert(alias.clone(), TunnelDecl { id: t.id.clone(), account: account.clone(), machine: machine.clone(), note: String::new(), destroy: false });
                Ok(())
            })?;
            sync::notify(&f, &c.me);
            if machine.as_deref() == Some(c.me.as_str()) {
                let token = client.connector_token(&acct.id, &t.id)?;
                c.config.upsert_tunnel(&alias, &token)?;
                launchd::start(&alias, &token)?;
            }
            done(format!(
                "created {alias} ({}) in {account}{}",
                t.id,
                match &machine {
                    Some(m) if *m == c.me => " and started it here".to_string(),
                    Some(m) => format!(" — the agent on {m} will fetch its token and start it"),
                    None => " — no machine runs it yet (`tunnels tunnel assign`)".into(),
                }
            ))
        }
        TunnelCmd::Adopt { tunnel, alias, machine } => {
            let c = ctx()?;
            Fleet::load_required()?;
            pull_first(&c.me);
            let snap = observe_all(&c.config);
            let r = resolve(&tunnel, &c, &snap)?;
            if let Some(a) = &r.alias {
                bail!("already in the fleet as {a}");
            }
            let f = Fleet::edit(&c.me, |f| {
                let acct = f.account_alias_for_id(&r.account_id).cloned().ok_or_else(|| anyhow!("its account is not in the fleet — `tunnels import` adds the accounts a token here reaches"))?;
                if let Some(m) = &machine {
                    if !f.machines.contains_key(m) {
                        bail!("no machine `{m}` in the fleet");
                    }
                }
                f.tunnels.insert(alias.clone(), TunnelDecl { id: r.id.clone(), account: acct, machine: machine.clone(), note: format!("Cloudflare name {:?}", r.cf_name), destroy: false });
                Ok(())
            })?;
            announce(&format!("adopt {} as {alias}", r.cf_name));
            sync::notify(&f, &c.me);
            done(format!("{alias} = {} ({}) is in the fleet — its current routes show as undeclared until you add them (`tunnels import` does that for the machine that runs it)", r.cf_name, short(&r.id)))
        }
        TunnelCmd::Assign { tunnel, machine, none } => {
            let c = ctx()?;
            pull_first(&c.me);
            if machine.is_none() && !none {
                bail!("--machine <name> or --none");
            }
            let mut alias = String::new();
            let f = Fleet::edit(&c.me, |f| {
                if let Some(m) = &machine {
                    if !f.machines.contains_key(m) {
                        bail!("no machine `{m}` in the fleet");
                    }
                }
                alias = f.find_tunnel(&tunnel).map(|(a, _)| a.clone()).ok_or_else(|| anyhow!("no tunnel `{tunnel}` in the fleet"))?;
                f.tunnels.get_mut(&alias).unwrap().machine = machine.clone();
                Ok(())
            })?;
            announce(&format!("assign {alias}"));
            sync::notify(&f, &c.me);
            done(format!(
                "{alias} now runs on {} (the machine that ran it keeps running it until you `tunnels tunnel forget` it there)",
                machine.as_deref().unwrap_or("no fleet machine")
            ))
        }
        TunnelCmd::Rename { old, new } => {
            let c = ctx()?;
            pull_first(&c.me);
            let f = Fleet::edit(&c.me, |f| {
                let a = f.find_tunnel(&old).map(|(a, _)| a.clone()).ok_or_else(|| anyhow!("no tunnel `{old}` in the fleet"))?;
                f.rename_tunnel(&a, &new)
            })?;
            announce(&format!("rename {old} → {new}"));
            sync::notify(&f, &c.me);
            done(format!("renamed {old} → {new} in the fleet file (its Cloudflare name and this Mac's LaunchAgent label are unchanged)"))
        }
        TunnelCmd::ImportPlists => {
            let mut c = ctx()?;
            let mut n = 0;
            for d in launchd::discover_existing() {
                if c.config.tunnel_by_name(&d.name).is_none() && !d.is_daemon {
                    c.config.add(d.name.clone(), d.token)?;
                    println!("  + {}", d.name);
                    n += 1;
                }
            }
            done(format!("{n} LaunchAgent(s) taken in"))
        }
    }
}

fn current_path() -> String {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    for n in [2, 1] {
        if args.len() >= n {
            let p = args[..n].join(" ");
            if SCOPES.iter().any(|(x, _)| *x == p) {
                return p;
            }
        }
    }
    String::new()
}

fn destroy_cmd(key: &str, yes: bool, json: bool) -> Result<i32> {
    let mut c = ctx()?;
    let snap = observe_all(&c.config);
    let r = resolve(key, &c, &snap)?;
    let obs = snap.tunnel(&r.id).ok_or_else(|| anyhow!("Cloudflare does not show tunnel {} from here — already deleted, or no token reaches its account", r.id))?;
    let fleet = c.fleet.clone();
    if let (Some(f), Some(alias)) = (&fleet, &r.alias) {
        let primaries: Vec<&str> = f.routes.iter().filter(|x| &x.tunnel == alias).map(|x| x.host.as_str()).collect();
        if !primaries.is_empty() {
            bail!(
                "{alias} is the primary for {} — move or remove those routes first (`tunnels route add <host> <svc> --tunnel <other>` or `tunnels route rm <host>`)",
                primaries.join(", ")
            );
        }
    }
    let dns: Vec<String> = snap
        .dns
        .iter()
        .filter(|d| d.tunnel_target().as_deref() == Some(r.id.to_ascii_lowercase().as_str()))
        .map(|d| d.name.clone())
        .collect();
    let name = r.alias.clone().unwrap_or_else(|| r.cf_name.clone());
    eprintln!("{} destroy {name}", Scope::LocalAndCloudflare.tag());
    eprintln!("  in Cloudflare:");
    eprintln!("    delete tunnel {} ({}) — every connector token for it stops working", r.cf_name, r.id);
    if obs.up() {
        eprintln!("    it has {} live connector(s) right now; they will be cut off", obs.tunnel.connections.len());
    }
    for d in &dns {
        eprintln!("    delete DNS {d} (points at it)");
    }
    if let Some(l) = &r.local {
        eprintln!("  on this Mac: stop and forget {l}");
    }
    if let Some(a) = &r.alias {
        eprintln!("  in the fleet file: remove {a}");
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("destroying needs --yes when not run from a terminal");
        }
        eprint!("type the name `{name}` to destroy it: ");
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        if s.trim() != name {
            bail!("not destroyed");
        }
    }
    // out of the fleet first, so no agent starts it again while it is being
    // deleted; if the deletion then fails it shows up as an orphan, with this
    // same command as the fix
    if let (Some(_), Some(alias)) = (&fleet, &r.alias) {
        let f = Fleet::edit(&c.me, |f| {
            remove_tunnel_from_fleet(f, alias);
            Ok(())
        })?;
        sync::notify(&f, &c.me);
    }
    let notes = apply::destroy(&snap, &mut c.config, &obs.account_id, &r.id)?;
    if json {
        print_json(&serde_json::json!({ "scope": Scope::LocalAndCloudflare, "destroyed": r.id, "notes": notes }))?;
    } else {
        println!("✓ destroyed {name}: {notes}");
    }
    Ok(0)
}

fn rotate_cmd(key: &str, json: bool) -> Result<i32> {
    let mut c = ctx()?;
    let snap = observe_all(&c.config);
    let r = resolve(key, &c, &snap)?;
    let client = snap.client_for_account(&r.account_id).ok_or_else(|| anyhow!("no API token here reaches that tunnel's account"))?;
    let name = r.alias.clone().unwrap_or_else(|| r.cf_name.clone());
    announce(&format!("rotate {name}"));
    client.rotate_secret(&r.account_id, &r.id).context("giving the tunnel a new secret")?;
    let token = client.connector_token(&r.account_id, &r.id).context("fetching the new connector token")?;
    let mut msg = format!("{name} has a new secret — every connector token issued before now no longer works");
    if let Some(l) = &r.local {
        c.config.upsert_tunnel(l, &token)?;
        launchd::write_token_file(&token)?;
        if launchd::plist_has_inline_token(l) || !launchd::plist_runs_token(l, &token) || !launchd::is_loaded_name(l) {
            // writes the plist fresh, token in a file; detached when over ssh
            launchd::restart(l, &token)?;
        } else {
            launchd::kickstart(l)?;
        }
        msg.push_str(&format!("; this Mac took the new one and restarted {l}"));
    }
    let owner = c.fleet.as_ref().and_then(|f| r.alias.as_ref().and_then(|a| f.machine_of(a).map(String::from)));
    if let Some(m) = owner.filter(|m| *m != c.me) {
        msg.push_str(&format!("; the agent on {m} fetches the new token on its next pass"));
        if let Some(f) = &c.fleet {
            sync::notify(f, &c.me);
        }
    }
    let others: Vec<String> = c.config.tunnels.iter().filter(|t| t.tunnel_id().as_deref() == Some(r.id.as_str()) && Some(&t.name) != r.local.as_ref()).map(|t| t.name.clone()).collect();
    if !others.is_empty() {
        msg.push_str(&format!("; also here under {}: updated", others.join(", ")));
    }
    if json {
        print_json(&serde_json::json!({ "scope": Scope::LocalAndCloudflare, "rotated": r.id, "old_tokens_work": false, "message": msg }))?;
    } else {
        println!("✓ {msg}");
    }
    Ok(0)
}

// ---------------------------------------------------------------- tokens

fn token_cmd(cmd: TokenCmd, json: bool) -> Result<i32> {
    match cmd {
        TokenCmd::Add { token } => {
            let mut c = ctx()?;
            let client = cf::Client::new(&token);
            client.verify().context("Cloudflare does not accept this token")?;
            let zones = client.zones().unwrap_or_default();
            let mut reach: Vec<config::Reach> = Vec::new();
            for z in &zones {
                match reach.iter_mut().find(|r| r.account_id == z.account_id) {
                    Some(r) => r.zones.push(z.name.clone()),
                    None => reach.push(config::Reach { account_id: z.account_id.clone(), account_name: z.account_name.clone(), zones: vec![z.name.clone()] }),
                }
            }
            for a in client.accounts().unwrap_or_default() {
                if !reach.iter().any(|r| r.account_id == a.id) {
                    reach.push(config::Reach { account_id: a.id, account_name: a.name, zones: vec![] });
                }
            }
            if reach.is_empty() {
                bail!("this token reaches no account and no zone — wrong Cloudflare account, or missing permissions");
            }
            let covers = reach.iter().map(|r| format!("{} ({})", r.account_name, r.zones.join(", "))).collect::<Vec<_>>().join(" · ");
            announce("keep an API token here");
            c.config.add_api_token(token, covers.clone(), reach)?;
            if json {
                print_json(&serde_json::json!({ "scope": Scope::LocalReadsCloudflare, "covers": covers }))?;
            } else {
                println!("✓ API token kept — {covers}");
                println!("  it needs Account › Cloudflare Tunnel › Edit and Zone › DNS › Edit to manage routes");
            }
            Ok(0)
        }
        TokenCmd::List => {
            let c = ctx()?;
            let tokens = c.config.api_tokens();
            if json {
                let rows: Vec<_> = tokens.iter().enumerate().map(|(i, t)| serde_json::json!({ "index": i, "hint": t.hint(), "reach": t.reach })).collect();
                print_json(&rows)?;
                return Ok(0);
            }
            if tokens.is_empty() {
                println!("no Cloudflare API tokens here — `tunnels token add <token>`");
            }
            for (i, t) in tokens.iter().enumerate() {
                if t.reach.is_empty() {
                    println!("{i}. {}", if t.covers.is_empty() { "(nothing recorded)" } else { &t.covers });
                }
                for r in &t.reach {
                    println!("{i}. {}  ({})", r.account_name, short(&r.account_id));
                    println!("     {}", r.zones.join(" · "));
                }
                println!("     via {}", t.hint());
            }
            Ok(0)
        }
        TokenCmd::Rm { index } => {
            let mut c = ctx()?;
            announce("forget an API token here");
            let covers = c.config.remove_api_token(index)?;
            println!("✓ forgot token {index}{} (only here: if Cloudflare still accepts it, revoke it at dash.cloudflare.com/profile/api-tokens)", if covers.is_empty() { String::new() } else { format!(" — {covers}") });
            Ok(0)
        }
    }
}

// ---------------------------------------------------------------- agent / web / scan

fn exe_for_launchd() -> String {
    // the stable path (Homebrew's symlink) rather than this version's
    // Cellar path, so `brew upgrade` is picked up by the running agent
    for p in ["/opt/homebrew/bin/tunnels", "/usr/local/bin/tunnels"] {
        if let (Ok(a), Ok(b)) = (std::fs::canonicalize(p), std::env::current_exe().and_then(std::fs::canonicalize)) {
            if a == b {
                return p.to_string();
            }
        }
    }
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "tunnels".into())
}

fn agent_cmd(cmd: AgentCmd, json: bool) -> Result<i32> {
    match cmd {
        AgentCmd::Run => {
            tunnels::agent::run()?;
            Ok(0)
        }
        AgentCmd::Once => {
            // one pass in the foreground, using the running agent if there is one
            let c = ctx()?;
            let port = c.fleet.as_ref().map(|f| f.policy.web_port).unwrap_or(fleet::DEFAULT_WEB_PORT);
            if launchd::agent_loaded() {
                let a: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(3))).build().into();
                if a.post(&format!("http://127.0.0.1:{port}/api/notify")).header("X-Tunnels", "1").send_json(serde_json::json!({})).is_ok() {
                    println!("✓ asked the running agent for a pass now — `tunnels agent status` shows what it did");
                    return Ok(0);
                }
            }
            let fleet = Fleet::load_required()?;
            let mut config = c.config.clone();
            let mine: BTreeSet<String> = fleet.tunnels.iter().filter(|(_, t)| t.machine.as_deref() == Some(c.me.as_str())).map(|(a, _)| a.clone()).collect();
            let snap = observe::observe(&config, &tunnels::agent::want_for(&fleet, &mine));
            let local = observe::observe_local(&config, &c.me);
            let p = plan::plan(&fleet, &snap, Some(&local));
            let opts = Options { only_owner: Some(c.me.clone()), prune: fleet.policy.prune, ..Default::default() };
            let r = apply::apply(&p, &snap, &mut config, &opts);
            print_report(&r, json)
        }
        AgentCmd::Install => {
            let exe = exe_for_launchd();
            announce(&format!("install the agent ({exe})"));
            launchd::install_agent(&exe)?;
            // the agent does what the watchdog and the periodic `tunnels heal`
            // did; two of them doing it is one too many
            for label in ["com.dorkyrobot.tunnel-watchdog", "com.tunnels.heal"] {
                let p = launchd::plist_dir().join(format!("{label}.plist"));
                if p.exists() {
                    let uid = unsafe { libc::getuid() };
                    let _ = std::process::Command::new("launchctl").args(["bootout", &format!("gui/{uid}/{label}")]).output();
                    let _ = std::fs::remove_file(&p);
                    println!("  removed the old {label} LaunchAgent (the agent does its job now)");
                }
            }
            let port = Fleet::load().ok().flatten().map(|f| f.policy.web_port).unwrap_or(fleet::DEFAULT_WEB_PORT);
            println!("✓ agent installed and started");
            match util::tailnet_ipv4() {
                Some(ip) => println!("  web UI: http://{ip}:{port}  (tailnet only)"),
                None => println!("  web UI: http://127.0.0.1:{port} — and on the tailnet once Tailscale is up"),
            }
            Ok(0)
        }
        AgentCmd::Uninstall => {
            announce("uninstall the agent");
            launchd::uninstall_agent()?;
            println!("✓ agent stopped and removed (tunnels keep running; nothing heals them now)");
            Ok(0)
        }
        AgentCmd::Status => {
            let c = ctx()?;
            let port = c.fleet.as_ref().map(|f| f.policy.web_port).unwrap_or(fleet::DEFAULT_WEB_PORT);
            let a: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(3))).build().into();
            let v: Option<serde_json::Value> = a.get(&format!("http://127.0.0.1:{port}/api/agent")).call().ok().and_then(|mut r| r.body_mut().read_json().ok());
            if json {
                print_json(&serde_json::json!({ "loaded": launchd::agent_loaded(), "agent": v }))?;
                return Ok(0);
            }
            println!("agent: {}", if launchd::agent_loaded() { "loaded" } else { "not installed" });
            let Some(v) = v else {
                println!("  not answering on 127.0.0.1:{port}");
                return Ok(if launchd::agent_loaded() { 1 } else { 0 });
            };
            println!("  {} v{} · running since {}", v["machine"].as_str().unwrap_or("?"), v["version"].as_str().unwrap_or("?"), v["started_at"].as_str().unwrap_or("?"));
            if let Some(t) = v.get("last_tick").filter(|t| !t.is_null()) {
                println!("  last pass {} · fleet serial {} · {} ms", t["at"].as_str().unwrap_or("?"), t["serial"], t["took_ms"]);
                if let Some(e) = t["error"].as_str() {
                    println!("  ✗ {e}");
                }
                for w in t["waiting"].as_array().into_iter().flatten() {
                    println!("  · waiting for a person: {} ({})", w[0].as_str().unwrap_or(""), w[1].as_str().unwrap_or(""));
                }
            }
            println!("  recent:");
            for e in v["events"].as_array().into_iter().flatten().take(15) {
                println!("    {} {:<8} {}", e["at"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["message"].as_str().unwrap_or(""));
            }
            Ok(0)
        }
    }
}

fn cf_cmd(cmd: CfCmd, json: bool) -> Result<i32> {
    use tunnels::api;
    let c = ctx()?;
    let fleet = c.fleet.clone().unwrap_or_default();
    let (method, a) = match cmd {
        CfCmd::Get(a) => ("GET", a),
        CfCmd::Post(a) => ("POST", a),
        CfCmd::Put(a) => ("PUT", a),
        CfCmd::Patch(a) => ("PATCH", a),
        CfCmd::Delete(a) => ("DELETE", a),
        CfCmd::Log { limit } => {
            let recs = api::load_log(limit);
            if json {
                print_json(&serde_json::json!({ "scope": Scope::Local, "machine": c.me, "records": recs }))?;
                return Ok(0);
            }
            if recs.is_empty() {
                println!("no changes made through `tunnels cf` on this Mac");
            }
            for r in &recs {
                println!(
                    "{}  {}  {} {:<6} {}{}{}",
                    r.id,
                    r.at,
                    if r.ok { "✓" } else { "✗" },
                    r.method,
                    r.path,
                    if r.undo.is_some() { "" } else { "   (not undoable)" },
                    r.undoes.as_ref().map(|u| format!("   (undid {u})")).unwrap_or_default()
                );
                for d in api::diff(r.before.as_ref(), r.after.as_ref()) {
                    println!("        {d}");
                }
            }
            return Ok(0);
        }
        CfCmd::Undo { id, yes } => {
            let rec = api::find(&id).ok_or_else(|| anyhow!("no change {id} in this Mac's log — `tunnels cf log`; an undo runs on the machine that made the change"))?;
            let u = rec.undo.clone().ok_or_else(|| anyhow!("change {id} ({} {}) cannot be undone", rec.method, rec.path))?;
            announce(&format!("undo {id}: {} {}", u.method, u.path));
            if u.best_effort {
                eprintln!("  (best effort: re-creating a deleted resource may not give it back exactly as it was)");
            }
            let out = api::call(
                api::Call {
                    method: &u.method,
                    path: &u.path,
                    body: u.body.clone(),
                    yes,
                    account: rec.account_id.as_deref(),
                    i_mean_tokens: api::is_token_path(&u.path),
                    not_undoable: true,
                    undoes: Some(id.clone()),
                },
                &c.config,
                &fleet,
                &c.me,
            )?;
            return print_cf(&out, json, scope_of("cf undo"));
        }
    };
    let body = match (&a.data, &a.data_file) {
        (Some(d), _) => Some(serde_json::from_str(d).context("--data is not JSON")?),
        (None, Some(f)) => {
            let text = if f == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(f)?
            };
            Some(serde_json::from_str(&text).context("--data-file is not JSON")?)
        }
        (None, None) => None,
    };
    if method != "GET" {
        announce(&format!("{method} {}", a.path));
    }
    let out = api::call(
        api::Call {
            method,
            path: &a.path,
            body,
            yes: a.yes,
            account: a.account.as_deref(),
            i_mean_tokens: a.i_mean_tokens,
            not_undoable: a.not_undoable,
            undoes: None,
        },
        &c.config,
        &fleet,
        &c.me,
    )?;
    print_cf(&out, json, if method == "GET" { Scope::ReadOnly } else { Scope::Cloudflare })
}

fn print_cf(out: &tunnels::api::Outcome, json: bool, scope: Scope) -> Result<i32> {
    use tunnels::api;
    let code = if out.refused.is_some() || (out.sent && !out.ok) { 1 } else { 0 };
    if json {
        let mut v = serde_json::to_value(out)?;
        v["scope"] = serde_json::to_value(scope)?;
        print_json(&v)?;
        return Ok(code);
    }
    for n in &out.notes {
        eprintln!("  {n}");
    }
    if out.resolved_path != out.path {
        eprintln!("  → {} {}", out.method, out.resolved_path);
    }
    if let Some(why) = &out.refused {
        println!("✗ refused: {why}");
        return Ok(1);
    }
    if out.method == "GET" {
        let v = out.response.clone().unwrap_or_default();
        let shown = if out.ok { v.get("result").cloned().unwrap_or(v) } else { v };
        println!("{}", serde_json::to_string_pretty(&shown)?);
        if out.hidden_secrets {
            eprintln!("  (secrets in the response were hidden)");
        }
        if !out.ok {
            eprintln!("✗ Cloudflare answered {}", out.status.unwrap_or(0));
            if out.status == Some(403) {
                eprintln!("  {}", api::permission_hint(&out.resolved_path));
            }
        }
        return Ok(code);
    }
    if !out.sent {
        println!("before:  {}", out.before.as_ref().map(|b| b.to_string()).unwrap_or_else(|| "(nothing read)".into()));
        match &out.undo {
            Some(u) => println!("undo:    {} {} {}", u.method, u.path, u.body.as_ref().map(|b| b.to_string()).unwrap_or_default()),
            None => println!("undo:    none — this cannot be undone"),
        }
        println!("\npreview only — nothing sent. Add --yes to send it.");
        return Ok(0);
    }
    if out.ok {
        println!("✓ {} {} — Cloudflare answered {}", out.method, out.path, out.status.unwrap_or(0));
    } else {
        let v = out.response.clone().unwrap_or_default();
        println!("✗ {} {} — Cloudflare answered {}: {}", out.method, out.path, out.status.unwrap_or(0), v.get("errors").cloned().unwrap_or(v));
        if out.status == Some(403) {
            println!("  {}", api::permission_hint(&out.resolved_path));
        }
    }
    let d = api::diff(out.before.as_ref(), out.after.as_ref());
    if out.ok {
        if d.is_empty() {
            println!("  check: reading it back shows no difference — the change may not have taken, or it was already so");
        } else {
            println!("  check: read back after the change:");
            for x in d {
                println!("    {x}");
            }
        }
    }
    if let Some(id) = &out.log_id {
        println!("  logged as {id}{}", if out.undo.is_some() { format!(" — undo with: tunnels cf undo {id}") } else { " (cannot be undone)".into() });
    }
    Ok(code)
}

fn web_cmd(open: bool, json: bool) -> Result<i32> {
    let port = Fleet::load().ok().flatten().map(|f| f.policy.web_port).unwrap_or(fleet::DEFAULT_WEB_PORT);
    let url = match util::tailnet_ipv4() {
        Some(ip) => format!("http://{ip}:{port}"),
        None => format!("http://127.0.0.1:{port}"),
    };
    if json {
        print_json(&serde_json::json!({ "url": url, "agent_loaded": launchd::agent_loaded() }))?;
    } else {
        println!("{url}{}", if launchd::agent_loaded() { "" } else { "   (the agent is not running here — `tunnels agent install`)" });
    }
    if open {
        let _ = std::process::Command::new("open").arg(&url).status();
    }
    Ok(0)
}

fn scan_cmd(json: bool) -> Result<i32> {
    let found = scan::scan_services();
    if json {
        let rows: Vec<_> = found.iter().map(|s| serde_json::json!({ "name": s.name, "port": s.port })).collect();
        print_json(&rows)?;
        return Ok(0);
    }
    println!("{:<24} {}", "NAME", "PORT");
    for s in &found {
        println!("{:<24} {}", s.name, s.port);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Every command's help must carry the tag of the scope it declares —
    /// the gap between "Delete a tunnel" and what `rm` did is this test.
    #[test]
    fn a_commands_help_says_its_scope() {
        let root = Cli::command();
        for (path, scope) in SCOPES {
            let mut cmd = &root;
            for part in path.split(' ') {
                cmd = cmd.find_subcommand(part).unwrap_or_else(|| panic!("no command `{path}`"));
            }
            let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
            assert!(about.starts_with(scope.tag()), "`tunnels {path}` says {about:?} but declares {}", scope.tag());
        }
    }

    #[test]
    fn every_visible_command_declares_a_scope() {
        let root = Cli::command();
        for sub in root.get_subcommands().filter(|s| !s.is_hide_set()) {
            let name = sub.get_name();
            if sub.has_subcommands() {
                for leaf in sub.get_subcommands() {
                    let p = format!("{name} {}", leaf.get_name());
                    assert!(SCOPES.iter().any(|(x, _)| *x == p), "`tunnels {p}` has no scope in SCOPES");
                }
            } else if name != "help" {
                assert!(SCOPES.iter().any(|(x, _)| *x == name), "`tunnels {name}` has no scope in SCOPES");
            }
        }
    }

    #[test]
    fn forget_is_local_and_destroy_is_not() {
        assert_eq!(scope_of("tunnel forget"), Scope::Local);
        assert_eq!(scope_of("tunnel destroy"), Scope::LocalAndCloudflare);
    }

    #[test]
    fn account_aliases_come_from_their_first_domain() {
        let taken = BTreeSet::new();
        assert_eq!(account_alias(&["felixflor.es".into(), "sarameig.gs".into()], "x", &taken), "felixflor");
        let taken: BTreeSet<String> = ["felixflor".to_string()].into();
        assert_eq!(account_alias(&["felixflor.es".into()], "x", &taken), "felixflor-2");
    }
}
