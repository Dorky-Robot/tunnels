//! tunnels: Cloudflare tunnels across a fleet of Macs, driven by a config
//! file and a CLI, kept in line by an agent on every machine.
//!
//! ```text
//!   fleet.toml ──┐                        ┌── cf.rs (Cloudflare API)
//!                ├─ plan.rs ── apply.rs ──┤
//!   observe.rs ──┘                        └── launchd.rs (this Mac)
//! ```
//!
//! `observe` looks, `plan` decides (pure, tested without a network),
//! `apply` acts. The CLI, the agent and the web UI are three front doors to
//! the same three steps, so they cannot disagree.

pub mod agent;
pub mod api;
pub mod apply;
pub mod cf;
pub mod config;
pub mod fleet;
pub mod launchd;
pub mod observe;
pub mod plan;
pub mod scan;
pub mod scope;
pub mod status;
pub mod sync;
pub mod util;
pub mod web;
