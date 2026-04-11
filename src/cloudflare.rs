use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
struct CfConnection {
    colo_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CfConfigResponse {
    success: bool,
    #[serde(default)]
    result: Option<CfConfigResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct CfConfigResult {
    config: CfConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct CfConfig {
    #[serde(default)]
    ingress: Vec<CfIngress>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CfIngress {
    #[serde(default)]
    pub(crate) hostname: Option<String>,
    pub(crate) service: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CfTunnelResponse {
    success: bool,
    #[serde(default)]
    result: Option<CfTunnelDetail>,
}

#[derive(Debug, Clone, Deserialize)]
struct CfTunnelDetail {
    #[serde(default)]
    name: String,
    #[serde(default)]
    connections: Vec<CfConnection>,
}

/// Tunnel info fetched from CF API
#[derive(Debug, Clone)]
pub(crate) struct TunnelInfo {
    pub(crate) cf_name: String,
    pub(crate) connections: String,
    pub(crate) connection_count: usize,
}

/// An ingress rule resolved to a port
#[derive(Debug, Clone)]
pub(crate) struct IngressRoute {
    pub(crate) hostname: String,
    pub(crate) tunnel_name: String,
    pub(crate) tunnel_id: String,
    pub(crate) scheme: String,
}

/// An account that needs an API token
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnreachedAccount {
    pub(crate) account_id: String,
    pub(crate) tunnel_names: Vec<String>,
    pub(crate) tunnel_id: String,
}

/// Result of a CF sync operation
pub(crate) struct SyncResult {
    /// tunnel_id -> TunnelInfo (CF name + connection status)
    pub(crate) tunnel_info: HashMap<String, TunnelInfo>,
    pub(crate) ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub(crate) status: String,
    pub(crate) unreached: Vec<UnreachedAccount>,
}

/// Verify an API token works for a given account/tunnel
pub(crate) fn verify_token(api_token: &str, account_id: &str, tunnel_id: &str) -> bool {
    fetch_tunnel_config_check(api_token, account_id, tunnel_id)
}

/// Check if an API token can list at least one Cloudflare zone (for DNS management).
/// Returns the zone names if successful.
pub(crate) fn verify_token_has_zones(api_token: &str) -> Option<Vec<String>> {
    let output = Command::new("curl")
        .args([
            "-s",
            "https://api.cloudflare.com/client/v4/zones?per_page=10",
            "-H",
            &format!("Authorization: Bearer {}", api_token),
        ])
        .output()
        .ok()?;

    let val: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    if !val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }
    let results = val.get("result").and_then(|v| v.as_array())?;
    if results.is_empty() {
        return None;
    }
    let names: Vec<String> = results.iter()
        .filter_map(|z| z.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .collect();
    Some(names)
}

/// Sync: fetch ingress routes for all accounts using configured API tokens.
/// cf_api_tokens: user-configured API tokens (one per CF account)
/// tunnel_tokens: Vec<(config_name, base64_token)>
pub(crate) fn sync(cf_api_tokens: &[&str], tunnel_tokens: &[(String, String)]) -> SyncResult {
    // Decode all tunnel tokens to get (name, account_id, tunnel_id) triples
    let decoded: Vec<(String, String, String)> = tunnel_tokens.iter()
        .filter_map(|(name, tok)| {
            crate::config::decode_token(tok).ok().map(|p| {
                (name.clone(), p.account_id, p.tunnel_id)
            })
        })
        .collect();

    // Group tunnels by account_id
    let mut accounts: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (name, account_id, tunnel_id) in &decoded {
        accounts.entry(account_id.clone())
            .or_default()
            .push((name.clone(), tunnel_id.clone()));
    }

    if accounts.is_empty() {
        return SyncResult {
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            status: "No tunnels configured".into(),
            unreached: Vec::new(),
        };
    }

    let api_tokens: Vec<&str> = cf_api_tokens.iter()
        .filter(|t| !t.is_empty())
        .copied()
        .collect();

    // If no API tokens at all, every account is unreached
    if api_tokens.is_empty() {
        let unreached: Vec<UnreachedAccount> = accounts.iter()
            .map(|(account_id, tunnels)| UnreachedAccount {
                account_id: account_id.clone(),
                tunnel_names: tunnels.iter().map(|(n, _)| n.clone()).collect(),
                tunnel_id: tunnels.first().map(|(_, id)| id.clone()).unwrap_or_default(),
            })
            .collect();
        return SyncResult {
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            status: format!("{} account(s) need API tokens — press T", unreached.len()),
            unreached,
        };
    }

    let mut port_map: HashMap<u16, Vec<IngressRoute>> = HashMap::new();
    let mut tunnel_info: HashMap<String, TunnelInfo> = HashMap::new();
    let mut total_routes = 0;
    let mut accounts_reached = 0;
    let mut unreached = Vec::new();

    for (account_id, tunnels) in &accounts {
        let mut account_ok = false;

        for api_token in &api_tokens {
            let probe_ok = tunnels.first()
                .map(|(_, id)| fetch_tunnel_config_check(api_token, account_id, id))
                .unwrap_or(false);

            if !probe_ok {
                continue;
            }

            for (name, tunnel_id) in tunnels {
                // Fetch tunnel details (name, connections)
                if let Some(detail) = fetch_tunnel_detail(api_token, account_id, tunnel_id) {
                    let conns = if detail.connections.is_empty() {
                        "no connections".to_string()
                    } else {
                        let colos: Vec<&str> = detail.connections.iter()
                            .map(|c| c.colo_name.as_str()).collect();
                        format!("{}x edge ({})", colos.len(), colos.join(", "))
                    };
                    let connection_count = detail.connections.len();
                    tunnel_info.insert(tunnel_id.clone(), TunnelInfo {
                        cf_name: detail.name,
                        connections: conns,
                        connection_count,
                    });
                }

                // Fetch ingress routes
                let ingress = fetch_tunnel_config(api_token, account_id, tunnel_id);
                for rule in ingress {
                    let hostname = match rule.hostname {
                        Some(h) => h,
                        None => continue,
                    };
                    if let Some(p) = parse_port_from_service(&rule.service) {
                        total_routes += 1;
                        let scheme = parse_scheme_from_service(&rule.service);
                        port_map.entry(p).or_default().push(IngressRoute {
                            hostname,
                            tunnel_name: name.clone(),
                            tunnel_id: tunnel_id.clone(),
                            scheme,
                        });
                    }
                }
            }

            account_ok = true;
            accounts_reached += 1;
            break;
        }

        if !account_ok {
            unreached.push(UnreachedAccount {
                account_id: account_id.clone(),
                tunnel_names: tunnels.iter().map(|(n, _)| n.clone()).collect(),
                tunnel_id: tunnels.first().map(|(_, id)| id.clone()).unwrap_or_default(),
            });
        }
    }

    let status = if !unreached.is_empty() {
        format!(
            "Synced {} route(s) from {} account(s) — {} need tokens (T)",
            total_routes, accounts_reached, unreached.len(),
        )
    } else {
        format!("Synced {} route(s) from {} account(s)", total_routes, accounts_reached)
    };

    SyncResult { tunnel_info, ingress_routes: port_map, status, unreached }
}

fn parse_port_from_service(service: &str) -> Option<u16> {
    service
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
}

/// Extract the scheme from a cloudflared service URL.
/// e.g. "ssh://localhost:22" → "ssh", "http://localhost:3000" → "https" (proxied via CF)
fn parse_scheme_from_service(service: &str) -> String {
    match service.split("://").next() {
        Some("ssh") => "ssh".into(),
        Some("tcp") => "tcp".into(),
        Some("rdp") => "rdp".into(),
        Some("unix" | "unix+tls") => "https".into(),
        _ => "https".into(),
    }
}

fn fetch_tunnel_config_check(api_token: &str, account_id: &str, tunnel_id: &str) -> bool {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        account_id, tunnel_id
    );
    let output = Command::new("curl")
        .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return false,
    };
    serde_json::from_slice::<CfConfigResponse>(&output)
        .map(|r| r.success)
        .unwrap_or(false)
}

fn fetch_tunnel_detail(api_token: &str, account_id: &str, tunnel_id: &str) -> Option<CfTunnelDetail> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}",
        account_id, tunnel_id
    );
    let output = Command::new("curl")
        .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return None,
    };
    let resp: CfTunnelResponse = serde_json::from_slice(&output).ok()?;
    if resp.success { resp.result } else { None }
}

fn fetch_tunnel_config(api_token: &str, account_id: &str, tunnel_id: &str) -> Vec<CfIngress> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        account_id, tunnel_id
    );
    let output = Command::new("curl")
        .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    let resp: CfConfigResponse = match serde_json::from_slice(&output) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    if !resp.success {
        return Vec::new();
    }
    resp.result.map(|r| r.config.ingress).unwrap_or_default()
}

/// Add an ingress rule (subdomain mapping) to a tunnel's configuration.
/// Idempotent: if the route already exists, just ensures DNS is correct.
pub(crate) fn add_route(
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    service: &str,
) -> Result<RouteResult, String> {
    let current = fetch_tunnel_config(api_token, account_id, tunnel_id);
    if current.is_empty() {
        return Err("Could not fetch current tunnel config".into());
    }

    let already_exists = current.iter().any(|r| r.hostname.as_deref() == Some(hostname));

    if !already_exists {
        // Build new ingress list: existing rules (minus catch-all) + new rule + catch-all
        let mut ingress: Vec<serde_json::Value> = Vec::new();
        for rule in &current {
            if rule.hostname.is_none() {
                continue;
            }
            let mut entry = serde_json::json!({ "service": rule.service });
            if let Some(ref h) = rule.hostname {
                entry["hostname"] = serde_json::json!(h);
            }
            ingress.push(entry);
        }

        ingress.push(serde_json::json!({
            "hostname": hostname,
            "service": service,
        }));

        let catchall_service = current.iter()
            .find(|r| r.hostname.is_none())
            .map(|r| r.service.as_str())
            .unwrap_or("http_status:404");
        ingress.push(serde_json::json!({ "service": catchall_service }));

        put_tunnel_config(api_token, account_id, tunnel_id, ingress)?;
    }

    // Ensure DNS — works for both new routes and fixing existing ones
    match create_dns_record(api_token, hostname, tunnel_id) {
        Ok(()) => {
            if already_exists {
                Ok(RouteResult::AlreadyExists)
            } else {
                Ok(RouteResult::Ok)
            }
        }
        Err(e) => Ok(RouteResult::DnsFailure(e)),
    }
}

/// Remove an ingress rule (subdomain mapping) from a tunnel's configuration.
pub(crate) fn remove_route(
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
) -> Result<RouteResult, String> {
    let current = fetch_tunnel_config(api_token, account_id, tunnel_id);
    if current.is_empty() {
        return Err("Could not fetch current tunnel config".into());
    }

    if !current.iter().any(|r| r.hostname.as_deref() == Some(hostname)) {
        return Err(format!("No route found for '{}'", hostname));
    }

    let mut ingress: Vec<serde_json::Value> = Vec::new();
    for rule in &current {
        if rule.hostname.as_deref() == Some(hostname) {
            continue;
        }
        let mut entry = serde_json::json!({ "service": rule.service });
        if let Some(ref h) = rule.hostname {
            entry["hostname"] = serde_json::json!(h);
        }
        ingress.push(entry);
    }

    if !ingress.iter().any(|e| e.get("hostname").is_none()) {
        ingress.push(serde_json::json!({ "service": "http_status:404" }));
    }

    put_tunnel_config(api_token, account_id, tunnel_id, ingress)?;

    match delete_dns_record(api_token, hostname) {
        Ok(()) => Ok(RouteResult::Ok),
        Err(e) => Ok(RouteResult::DnsFailure(e)),
    }
}

/// List all ingress routes for a tunnel
pub(crate) fn list_routes(
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
) -> Vec<CfIngress> {
    fetch_tunnel_config(api_token, account_id, tunnel_id)
}

/// Result of a route add/remove/fix operation
#[derive(Debug, Clone)]
pub(crate) enum RouteResult {
    /// Everything worked
    Ok,
    /// Route existed already, DNS was ensured
    AlreadyExists,
    /// Route succeeded but DNS failed
    DnsFailure(String),
}

pub const DNS_PERMISSION_HINT: &str = "\
Your API token needs these additional permissions for automatic DNS:
  • Zone > Zone > Read
  • Zone > DNS > Edit
Update at: dash.cloudflare.com/profile/api-tokens";

/// Check if a CNAME DNS record exists for a hostname
pub(crate) fn check_dns(api_token: &str, hostname: &str) -> Result<bool, String> {
    let zone_id = find_zone_id(api_token, hostname)?;
    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=CNAME&name={}",
        zone_id, hostname
    );
    let output = Command::new("curl")
        .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    let val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parse: {}", e))?;

    let results = val.get("result").and_then(|v| v.as_array());
    Ok(results.map_or(false, |r| !r.is_empty()))
}

/// Ensure DNS record exists for a hostname pointing at a tunnel.
/// This is idempotent — safe to call even if the record already exists.
pub(crate) fn ensure_dns(
    api_token: &str,
    hostname: &str,
    tunnel_id: &str,
) -> Result<RouteResult, String> {
    match create_dns_record(api_token, hostname, tunnel_id) {
        Ok(()) => Ok(RouteResult::Ok),
        Err(e) => Ok(RouteResult::DnsFailure(e)),
    }
}

// --- DNS management ---

/// Look up the Cloudflare zone ID for a hostname (e.g. "levee2.everyday.vet" → zone for "everyday.vet")
fn find_zone_id(api_token: &str, hostname: &str) -> Result<String, String> {
    // Try progressively shorter domain suffixes: "sub.example.com" → "example.com"
    let parts: Vec<&str> = hostname.split('.').collect();
    for i in 0..parts.len().saturating_sub(1) {
        let candidate = parts[i..].join(".");
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones?name={}",
            candidate
        );
        let output = Command::new("curl")
            .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", api_token)])
            .output()
            .map_err(|e| format!("curl: {}", e))?;

        if !output.status.success() {
            continue;
        }

        let val: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("parse: {}", e))?;

        if val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(results) = val.get("result").and_then(|v| v.as_array()) {
                if let Some(zone) = results.first() {
                    if let Some(id) = zone.get("id").and_then(|v| v.as_str()) {
                        return Ok(id.to_string());
                    }
                }
            }
        }
    }
    Err(format!("No Cloudflare zone found for '{}'", hostname))
}

/// Create a CNAME DNS record pointing hostname → tunnel_id.cfargotunnel.com
fn create_dns_record(api_token: &str, hostname: &str, tunnel_id: &str) -> Result<(), String> {
    let zone_id = find_zone_id(api_token, hostname)?;
    let target = format!("{}.cfargotunnel.com", tunnel_id);

    // Check if a CNAME record already exists
    let list_url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=CNAME&name={}",
        zone_id, hostname
    );
    let output = Command::new("curl")
        .args(["-s", &list_url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    if output.status.success() {
        let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if let Some(results) = val.get("result").and_then(|v| v.as_array()) {
            if !results.is_empty() {
                // CNAME already exists, update it
                if let Some(record_id) = results[0].get("id").and_then(|v| v.as_str()) {
                    return update_dns_record(api_token, &zone_id, record_id, hostname, &target);
                }
            }
        }
    }

    // Delete any conflicting A/AAAA records before creating the CNAME
    delete_conflicting_records(api_token, &zone_id, hostname)?;

    // Create new CNAME record
    let create_url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
        zone_id
    );
    let body = serde_json::json!({
        "type": "CNAME",
        "name": hostname,
        "content": target,
        "proxied": true,
    });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let output = Command::new("curl")
        .args([
            "-s", "-X", "POST",
            &create_url,
            "-H", &format!("Authorization: Bearer {}", api_token),
            "-H", "Content-Type: application/json",
            "-d", &body_str,
        ])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    parse_cf_response(&output.stdout).map(|_| ())
}

/// Delete A and AAAA records that would conflict with a CNAME for the same hostname.
fn delete_conflicting_records(api_token: &str, zone_id: &str, hostname: &str) -> Result<(), String> {
    for record_type in &["A", "AAAA"] {
        let list_url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type={}&name={}",
            zone_id, record_type, hostname
        );
        let output = Command::new("curl")
            .args(["-s", &list_url, "-H", &format!("Authorization: Bearer {}", api_token)])
            .output()
            .map_err(|e| format!("curl: {}", e))?;

        if !output.status.success() {
            continue;
        }

        let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        let results = match val.get("result").and_then(|v| v.as_array()) {
            Some(r) => r,
            None => continue,
        };

        for record in results {
            let record_id = match record.get("id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => continue,
            };
            let delete_url = format!(
                "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
                zone_id, record_id
            );
            let del_output = Command::new("curl")
                .args([
                    "-s", "-X", "DELETE",
                    &delete_url,
                    "-H", &format!("Authorization: Bearer {}", api_token),
                ])
                .output()
                .map_err(|e| format!("curl: {}", e))?;

            // Best-effort: if deletion fails, the CNAME create will surface the error
            let _ = parse_cf_response(&del_output.stdout);
        }
    }
    Ok(())
}

fn update_dns_record(api_token: &str, zone_id: &str, record_id: &str, hostname: &str, target: &str) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
        zone_id, record_id
    );
    let body = serde_json::json!({
        "type": "CNAME",
        "name": hostname,
        "content": target,
        "proxied": true,
    });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let output = Command::new("curl")
        .args([
            "-s", "-X", "PUT",
            &url,
            "-H", &format!("Authorization: Bearer {}", api_token),
            "-H", "Content-Type: application/json",
            "-d", &body_str,
        ])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    parse_cf_response(&output.stdout).map(|_| ())
}

/// Delete the CNAME DNS record for a hostname
fn delete_dns_record(api_token: &str, hostname: &str) -> Result<(), String> {
    let zone_id = find_zone_id(api_token, hostname)?;

    let list_url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=CNAME&name={}",
        zone_id, hostname
    );
    let output = Command::new("curl")
        .args(["-s", &list_url, "-H", &format!("Authorization: Bearer {}", api_token)])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    if !output.status.success() {
        return Err("Failed to list DNS records".into());
    }

    let val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parse: {}", e))?;

    let results = val.get("result").and_then(|v| v.as_array())
        .ok_or_else(|| format!("No DNS record found for '{}'", hostname))?;

    if results.is_empty() {
        return Ok(()); // No record to delete
    }

    let record_id = results[0].get("id").and_then(|v| v.as_str())
        .ok_or("Could not get DNS record ID")?;

    let delete_url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
        zone_id, record_id
    );
    let output = Command::new("curl")
        .args([
            "-s", "-X", "DELETE",
            &delete_url,
            "-H", &format!("Authorization: Bearer {}", api_token),
        ])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    parse_cf_response(&output.stdout).map(|_| ())
}

fn put_tunnel_config(
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
    ingress: Vec<serde_json::Value>,
) -> Result<String, String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        account_id, tunnel_id
    );
    let body = serde_json::json!({
        "config": {
            "ingress": ingress
        }
    });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let output = Command::new("curl")
        .args([
            "-s", "-X", "PUT",
            &url,
            "-H", &format!("Authorization: Bearer {}", api_token),
            "-H", "Content-Type: application/json",
            "-d", &body_str,
        ])
        .output()
        .map_err(|e| format!("curl: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl exited {}: {}", output.status, stderr.trim()));
    }

    parse_cf_response(&output.stdout)
}

fn parse_cf_response(body: &[u8]) -> Result<String, String> {
    let val: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| format!("parse response: {}", e))?;

    let success = val.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    if success {
        return Ok("OK".into());
    }

    // Extract readable error messages
    let errors = val.get("errors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let code = e.get("code").and_then(|c| c.as_u64());
                    let msg = e.get("message").and_then(|m| m.as_str());
                    match (code, msg) {
                        (Some(c), Some(m)) => Some(format!("[{}] {}", c, m)),
                        (None, Some(m)) => Some(m.to_string()),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_else(|| "unknown error".into());

    Err(errors)
}
