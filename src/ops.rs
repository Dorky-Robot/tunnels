//! Operations, once.
//!
//! There are two front doors — a CLI and a TUI — and for a while each
//! implemented the same operations separately. They shared storage, so
//! the drift was quiet: "what does this API token cover?" was answered
//! one way for `tunnels token add` and another way inside the TUI, an
//! hour apart, by the same person. The TUI rejected a token that reached
//! nothing; the CLI stored it happily.
//!
//! So operations live here and return **values**, never printed text and
//! never UI state. A front door's whole job is to collect arguments and
//! render the outcome:
//!
//! ```text
//!   cli_token_add   ─┐                        ┌─ println!
//!                    ├─ ops::add_api_token ───┤
//!   TUI  t,a        ─┘                        └─ status_msg
//! ```
//!
//! The rule this file exists to enforce: **if a behaviour would be
//! surprising when it differs between the CLI and the TUI, it belongs
//! here.** Wording differs at the edges; decisions do not.

use crate::cloudflare::{self, UnreachedAccount};
use crate::config::Config;
use anyhow::Result;

/// What happened when a token was added — enough for either front door to
/// say something useful, without either deciding anything.
pub struct TokenAdded {
    /// what the token turned out to reach, as stored
    pub covers: String,
    /// it can read the tunnel account that was asking for a token
    pub answered_the_need: bool,
    /// an account still without a token — the caller pasted one for a
    /// different Cloudflare account, which is valid and still not enough
    pub still_waiting: Option<String>,
}

/// Add a Cloudflare API token, after finding out what it reaches.
///
/// A token is refused only when it reaches nothing at all: no tunnel
/// account, no DNS zone. Anything else is stored — a token for the wrong
/// account is still useful for its own zones — but the result says so, so
/// a caller can tell somebody rather than leave them to discover it at a
/// route that fails days later.
pub fn add_api_token(
    config: &mut Config,
    token: &str,
    unreached: &[UnreachedAccount],
) -> Result<TokenAdded> {
    let matched = unreached
        .iter()
        .find(|a| cloudflare::verify_token(token, &a.account_id, &a.tunnel_id));
    // which accounts, and which domains in each — the shape a person with
    // two Cloudflare accounts actually needs to see
    let reach = cloudflare::token_reach(token);

    let covers = match (&matched, reach.is_empty()) {
        (_, false) => reach
            .iter()
            .map(|r| format!("{} ({})", r.account_name, r.zones.join(", ")))
            .collect::<Vec<_>>()
            .join(" · "),
        (Some(a), true) => format!("tunnels: {}", a.tunnel_names.join(", ")),
        (None, true) => anyhow::bail!(
            "this token reaches no tunnel account and no DNS zone — \
             wrong Cloudflare account, or missing permissions"
        ),
    };

    config.add_api_token(token.to_string(), covers.clone(), reach)?;

    Ok(TokenAdded {
        covers,
        answered_the_need: matched.is_some(),
        still_waiting: match matched {
            None => unreached.first().map(|a| a.account_id.clone()),
            Some(_) => None,
        },
    })
}

/// Forget a token by its position in [`Config::api_tokens`]. Returns what
/// it covered, so a caller can say what was let go of.
pub fn remove_api_token(config: &mut Config, idx: usize) -> Result<String> {
    config.remove_api_token(idx)
}
