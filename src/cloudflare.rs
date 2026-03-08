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
struct CfIngress {
    #[serde(default)]
    pub hostname: Option<String>,
    pub service: String,
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
pub struct TunnelInfo {
    pub cf_name: String,
    pub connections: String,
}

/// An ingress rule resolved to a port
#[derive(Debug, Clone)]
pub struct IngressRoute {
    pub hostname: String,
    pub tunnel_name: String,
    pub tunnel_id: String,
}

/// An account that needs an API token
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreachedAccount {
    pub account_id: String,
    pub tunnel_names: Vec<String>,
    pub tunnel_id: String,
}

/// Result of a CF sync operation
pub struct SyncResult {
    /// tunnel_id -> TunnelInfo (CF name + connection status)
    pub tunnel_info: HashMap<String, TunnelInfo>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub status: String,
    pub unreached: Vec<UnreachedAccount>,
}

/// Verify an API token works for a given account/tunnel
pub fn verify_token(api_token: &str, account_id: &str, tunnel_id: &str) -> bool {
    fetch_tunnel_config_check(api_token, account_id, tunnel_id)
}

/// Pre-decoded tunnel data for sync
pub struct TunnelSyncInput {
    pub name: String,
    pub account_id: String,
    pub tunnel_id: String,
    pub api_token: Option<String>,
}

/// Sync: fetch ingress routes using per-tunnel API tokens.
pub fn sync(tunnels: &[TunnelSyncInput]) -> SyncResult {
    // Group tunnels by account_id
    let mut accounts: HashMap<String, Vec<(String, String, Option<String>)>> = HashMap::new();
    for t in tunnels {
        if t.account_id.is_empty() {
            continue;
        }
        let (name, account_id, tunnel_id, api) =
            (&t.name, &t.account_id, &t.tunnel_id, &t.api_token);
        accounts.entry(account_id.clone()).or_default().push((
            name.clone(),
            tunnel_id.clone(),
            api.clone(),
        ));
    }

    if accounts.is_empty() {
        return SyncResult {
            tunnel_info: HashMap::new(),
            ingress_routes: HashMap::new(),
            status: "No tunnels configured".into(),
            unreached: Vec::new(),
        };
    }

    let mut port_map: HashMap<u16, Vec<IngressRoute>> = HashMap::new();
    let mut tunnel_info: HashMap<String, TunnelInfo> = HashMap::new();
    let mut total_routes = 0;
    let mut accounts_reached = 0;
    let mut unreached = Vec::new();

    for (account_id, tunnels) in &accounts {
        // Find an API token from any tunnel in this account
        let api_token = tunnels
            .iter()
            .filter_map(|(_, _, api)| api.as_deref())
            .find(|t| !t.is_empty());

        let Some(api_token) = api_token else {
            unreached.push(UnreachedAccount {
                account_id: account_id.clone(),
                tunnel_names: tunnels.iter().map(|(n, _, _)| n.clone()).collect(),
                tunnel_id: tunnels
                    .first()
                    .map(|(_, id, _)| id.clone())
                    .unwrap_or_default(),
            });
            continue;
        };

        for (name, tunnel_id, _) in tunnels {
            if let Some(detail) = fetch_tunnel_detail(api_token, account_id, tunnel_id) {
                let conns = if detail.connections.is_empty() {
                    "no connections".to_string()
                } else {
                    let colos: Vec<&str> = detail
                        .connections
                        .iter()
                        .map(|c| c.colo_name.as_str())
                        .collect();
                    format!("{}x edge ({})", colos.len(), colos.join(", "))
                };
                tunnel_info.insert(
                    tunnel_id.clone(),
                    TunnelInfo {
                        cf_name: detail.name,
                        connections: conns,
                    },
                );
            }

            let ingress = fetch_tunnel_config(api_token, account_id, tunnel_id);
            for rule in ingress {
                let hostname = match rule.hostname {
                    Some(h) => h,
                    None => continue,
                };
                if let Some(p) = parse_port_from_service(&rule.service) {
                    total_routes += 1;
                    port_map.entry(p).or_default().push(IngressRoute {
                        hostname,
                        tunnel_name: name.clone(),
                        tunnel_id: tunnel_id.clone(),
                    });
                }
            }
        }

        accounts_reached += 1;
    }

    let status = if !unreached.is_empty() {
        format!(
            "Synced {} route(s) from {} account(s) — {} need API tokens",
            total_routes,
            accounts_reached,
            unreached.len(),
        )
    } else {
        format!(
            "Synced {} route(s) from {} account(s)",
            total_routes, accounts_reached
        )
    };

    SyncResult {
        tunnel_info,
        ingress_routes: port_map,
        status,
        unreached,
    }
}

fn parse_port_from_service(service: &str) -> Option<u16> {
    service
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
}

fn fetch_tunnel_config_check(api_token: &str, account_id: &str, tunnel_id: &str) -> bool {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        account_id, tunnel_id
    );
    let output = Command::new("curl")
        .args([
            "-s",
            &url,
            "-H",
            &format!("Authorization: Bearer {}", api_token),
        ])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return false,
    };
    serde_json::from_slice::<CfConfigResponse>(&output)
        .map(|r| r.success)
        .unwrap_or(false)
}

fn fetch_tunnel_detail(
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
) -> Option<CfTunnelDetail> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}",
        account_id, tunnel_id
    );
    let output = Command::new("curl")
        .args([
            "-s",
            &url,
            "-H",
            &format!("Authorization: Bearer {}", api_token),
        ])
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
        .args([
            "-s",
            &url,
            "-H",
            &format!("Authorization: Bearer {}", api_token),
        ])
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
