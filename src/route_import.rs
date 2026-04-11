//! Parsing and planning for `tunnels route import`.
//!
//! Pure logic only — no I/O, no Cloudflare API, no launchd. The CLI glue in
//! `main.rs` reads stdin, calls these helpers, then applies the result via
//! `cloudflare::add_route`.
//!
//! Input format matches `tunnels routes --json` output exactly:
//! ```json
//! [{"tunnel": "mac-mini", "hostname": "app.example.com", "service": "http://localhost:3000"}]
//! ```

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RouteImportEntry {
    pub(crate) tunnel: String,
    pub(crate) hostname: String,
    pub(crate) service: String,
}

/// Parse a JSON array of route entries, as emitted by `tunnels routes --json`.
/// Empty/whitespace input parses to an empty list (so piping from a no-routes
/// tunnel is a no-op, not an error).
pub(crate) fn parse_entries(json: &str) -> Result<Vec<RouteImportEntry>> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let entries: Vec<RouteImportEntry> = serde_json::from_str(trimmed)
        .map_err(|e| anyhow!("invalid route JSON: {}", e))?;
    for e in &entries {
        if e.hostname.is_empty() || e.hostname == "(catch-all)" {
            return Err(anyhow!(
                "cannot import catch-all entry (hostname='{}')",
                e.hostname
            ));
        }
        if e.tunnel.is_empty() {
            return Err(anyhow!("entry for '{}' is missing tunnel", e.hostname));
        }
        if e.service.is_empty() {
            return Err(anyhow!("entry for '{}' is missing service", e.hostname));
        }
    }
    Ok(entries)
}

/// Apply a target tunnel override: if `target` is set, rewrite every entry's
/// tunnel to that name. This is the cross-tunnel-move use case:
///   tunnels routes mac-mini --json | tunnels route import --tunnel home-mesh
pub(crate) fn retarget(
    entries: Vec<RouteImportEntry>,
    target: Option<&str>,
) -> Vec<RouteImportEntry> {
    match target {
        Some(name) => entries
            .into_iter()
            .map(|mut e| {
                e.tunnel = name.to_string();
                e
            })
            .collect(),
        None => entries,
    }
}

/// Group entries by target tunnel, preserving first-seen order of both tunnels
/// and the entries within each tunnel. Lets the caller resolve each tunnel's
/// API credentials once, then loop through its routes.
pub(crate) fn group_by_tunnel(
    entries: Vec<RouteImportEntry>,
) -> Vec<(String, Vec<RouteImportEntry>)> {
    let mut groups: Vec<(String, Vec<RouteImportEntry>)> = Vec::new();
    for e in entries {
        if let Some(group) = groups.iter_mut().find(|(name, _)| name == &e.tunnel) {
            group.1.push(e);
        } else {
            groups.push((e.tunnel.clone(), vec![e]));
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tunnel: &str, hostname: &str, service: &str) -> RouteImportEntry {
        RouteImportEntry {
            tunnel: tunnel.into(),
            hostname: hostname.into(),
            service: service.into(),
        }
    }

    #[test]
    fn parses_routes_json_output_format() {
        let json = r#"[
            {"tunnel": "mac-mini", "hostname": "katulong.felixflor.es", "service": "http://localhost:3001"},
            {"tunnel": "mac-2024", "hostname": "admin.everyday.vet", "service": "http://localhost:3000"}
        ]"#;
        let entries = parse_entries(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], entry("mac-mini", "katulong.felixflor.es", "http://localhost:3001"));
        assert_eq!(entries[1], entry("mac-2024", "admin.everyday.vet", "http://localhost:3000"));
    }

    #[test]
    fn parses_empty_array() {
        let entries = parse_entries("[]").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn empty_input_is_no_op_not_error() {
        assert!(parse_entries("").unwrap().is_empty());
        assert!(parse_entries("   \n\t").unwrap().is_empty());
    }

    #[test]
    fn rejects_catchall_entries() {
        let json = r#"[{"tunnel": "t", "hostname": "(catch-all)", "service": "http_status:404"}]"#;
        let err = parse_entries(json).unwrap_err().to_string();
        assert!(err.contains("catch-all"), "expected catch-all rejection, got: {}", err);
    }

    #[test]
    fn rejects_missing_tunnel() {
        let json = r#"[{"tunnel": "", "hostname": "h.example.com", "service": "http://localhost:3000"}]"#;
        let err = parse_entries(json).unwrap_err().to_string();
        assert!(err.contains("missing tunnel"), "got: {}", err);
    }

    #[test]
    fn rejects_missing_service() {
        let json = r#"[{"tunnel": "t", "hostname": "h.example.com", "service": ""}]"#;
        let err = parse_entries(json).unwrap_err().to_string();
        assert!(err.contains("missing service"), "got: {}", err);
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_entries("not json").is_err());
        assert!(parse_entries("{}").is_err()); // object where array expected
    }

    #[test]
    fn retarget_rewrites_tunnel_name_on_all_entries() {
        let entries = vec![
            entry("old-a", "a.example.com", "http://localhost:3000"),
            entry("old-b", "b.example.com", "http://localhost:3001"),
        ];
        let out = retarget(entries, Some("home-mesh"));
        assert_eq!(out[0].tunnel, "home-mesh");
        assert_eq!(out[1].tunnel, "home-mesh");
        // hostname/service untouched
        assert_eq!(out[0].hostname, "a.example.com");
        assert_eq!(out[1].service, "http://localhost:3001");
    }

    #[test]
    fn retarget_none_is_passthrough() {
        let entries = vec![entry("keep-me", "a.example.com", "http://localhost:3000")];
        let out = retarget(entries.clone(), None);
        assert_eq!(out, entries);
    }

    #[test]
    fn group_by_tunnel_preserves_order_within_and_across_groups() {
        let entries = vec![
            entry("a", "h1", "s1"),
            entry("b", "h2", "s2"),
            entry("a", "h3", "s3"),
            entry("c", "h4", "s4"),
            entry("b", "h5", "s5"),
        ];
        let groups = group_by_tunnel(entries);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, "a");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[0].1[0].hostname, "h1");
        assert_eq!(groups[0].1[1].hostname, "h3");
        assert_eq!(groups[1].0, "b");
        assert_eq!(groups[1].1[0].hostname, "h2");
        assert_eq!(groups[1].1[1].hostname, "h5");
        assert_eq!(groups[2].0, "c");
    }

    #[test]
    fn parse_then_retarget_then_group_end_to_end() {
        let json = r#"[
            {"tunnel": "mac-mini", "hostname": "a.example.com", "service": "http://localhost:3000"},
            {"tunnel": "mac-2024", "hostname": "b.example.com", "service": "http://localhost:3001"}
        ]"#;
        let entries = parse_entries(json).unwrap();
        let retargeted = retarget(entries, Some("home-mesh"));
        let groups = group_by_tunnel(retargeted);
        // Both entries collapse into a single home-mesh group.
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "home-mesh");
        assert_eq!(groups[0].1.len(), 2);
    }
}
