//! Where a command acts: this Mac, Cloudflare, or both.
//!
//! `tunnels rm` used to say "Delete a tunnel" and only forgot it locally.
//! The tunnel, and every connector token ever issued for it, kept working
//! in Cloudflare, while the person who ran it believed it was gone. The fix
//! is not only better wording: every command declares its scope, prints it
//! before acting, carries it in `--json`, and a command declared local
//! cannot construct a Cloudflare client at all — [`assert_cloudflare_allowed`]
//! refuses, and the tests hold every local command to that.

use serde::Serialize;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// reads or changes nothing outside this Mac
    Local,
    /// reads Cloudflare, changes nothing anywhere
    ReadOnly,
    /// changes this Mac, and only reads Cloudflare
    LocalReadsCloudflare,
    /// changes Cloudflare (routes, DNS, tunnels)
    Cloudflare,
    /// changes this Mac and Cloudflare
    LocalAndCloudflare,
    /// changes the fleet file (which the agents then carry out)
    Fleet,
}

impl Scope {
    pub fn tag(self) -> &'static str {
        match self {
            Scope::Local => "[this Mac only]",
            Scope::ReadOnly => "[read-only]",
            Scope::LocalReadsCloudflare => "[this Mac; reads cloudflare]",
            Scope::Cloudflare => "[cloudflare]",
            Scope::LocalAndCloudflare => "[this Mac + cloudflare]",
            Scope::Fleet => "[fleet file + cloudflare]",
        }
    }

    fn code(self) -> u8 {
        match self {
            Scope::Local => 1,
            Scope::ReadOnly => 2,
            Scope::LocalReadsCloudflare => 6,
            Scope::Cloudflare => 3,
            Scope::LocalAndCloudflare => 4,
            Scope::Fleet => 5,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Declare the scope of the command now running.
pub fn enter(scope: Scope) {
    CURRENT.store(scope.code(), Ordering::SeqCst);
}

/// Called by every Cloudflare client constructor. A command that said it
/// only touches this Mac and then reaches for Cloudflare is a bug of exactly
/// the kind that made `rm` lie, so it stops here rather than quietly working.
pub fn assert_cloudflare_allowed() {
    if CURRENT.load(Ordering::SeqCst) == Scope::Local.code() {
        panic!("a command declared [this Mac only] tried to talk to Cloudflare");
    }
}
