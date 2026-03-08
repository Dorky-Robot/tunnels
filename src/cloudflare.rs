use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct CfTunnel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub connections: Vec<CfConnection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfConnection {
    pub colo_name: String,
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

/// An ingress rule resolved to a port
#[derive(Debug, Clone)]
pub struct IngressRoute {
    pub hostname: String,
    pub tunnel_name: String,
    pub tunnel_id: String,
}

/// API credentials for Cloudflare
struct ApiCreds {
    account_id: String,
    api_token: String,
}

/// Result of a CF sync operation
pub struct SyncResult {
    pub tunnels: Vec<CfTunnel>,
    pub ingress_routes: HashMap<u16, Vec<IngressRoute>>,
    pub status: String,
}

/// Query `cloudflared tunnel list --output json` for live tunnel data.
pub fn list_tunnels() -> Vec<CfTunnel> {
    let output = Command::new("cloudflared")
        .args(["tunnel", "list", "--output", "json"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };

    serde_json::from_slice(&output).unwrap_or_default()
}

/// Summarize connection status for a tunnel
pub fn connection_summary(tunnel: &CfTunnel) -> String {
    if tunnel.connections.is_empty() {
        return "no connections".to_string();
    }

    let colos: Vec<&str> = tunnel.connections.iter().map(|c| c.colo_name.as_str()).collect();
    format!("{}x edge ({})", colos.len(), colos.join(", "))
}

/// Find a CF tunnel by its ID
pub fn find_by_id<'a>(tunnels: &'a [CfTunnel], tunnel_id: &str) -> Option<&'a CfTunnel> {
    tunnels.iter().find(|t| t.id == tunnel_id)
}

/// Full sync: list tunnels + fetch ingress routes, trying all available auth methods.
/// Takes an optional CF API token from config and tunnel tokens for account_id extraction.
pub fn sync(cf_api_token: Option<&str>, tunnel_tokens: &[(String, String)]) -> SyncResult {
    let cf_tunnels = list_tunnels();

    // Try to get API credentials from multiple sources
    let creds = load_api_creds_multi(cf_api_token, tunnel_tokens);

    match creds {
        Some(creds) => {
            let ingress_routes = fetch_all_ingress(&creds, &cf_tunnels);
            let route_count: usize = ingress_routes.values().map(|v| v.len()).sum();
            let status = format!(
                "Synced {} tunnel(s), {} route(s) from Cloudflare",
                cf_tunnels.len(),
                route_count,
            );
            SyncResult { tunnels: cf_tunnels, ingress_routes, status }
        }
        None => {
            if cf_tunnels.is_empty() {
                SyncResult {
                    tunnels: cf_tunnels,
                    ingress_routes: HashMap::new(),
                    status: "No CF auth — run 'cloudflared tunnel login' or set cf_api_token in config".into(),
                }
            } else {
                // We got tunnels (cert.pem exists for list) but no API creds for routes
                let status = format!(
                    "Synced {} tunnel(s) — no ingress routes (set cf_api_token in config)",
                    cf_tunnels.len(),
                );
                SyncResult { tunnels: cf_tunnels, ingress_routes: HashMap::new(), status }
            }
        }
    }
}

/// Fetch all ingress routes for all known tunnels.
fn fetch_all_ingress(creds: &ApiCreds, tunnels: &[CfTunnel]) -> HashMap<u16, Vec<IngressRoute>> {
    let mut port_map: HashMap<u16, Vec<IngressRoute>> = HashMap::new();

    for tunnel in tunnels {
        let ingress = fetch_tunnel_config(creds, &tunnel.id);
        for rule in ingress {
            let hostname = match rule.hostname {
                Some(h) => h,
                None => continue, // skip catch-all
            };

            let route = IngressRoute {
                hostname,
                tunnel_name: tunnel.name.clone(),
                tunnel_id: tunnel.id.clone(),
            };

            if let Some(p) = parse_port_from_service(&rule.service) {
                port_map.entry(p).or_default().push(route);
            }
        }
    }

    port_map
}

/// Parse port from a service URL like "http://localhost:3001" or "ssh://localhost:22"
fn parse_port_from_service(service: &str) -> Option<u16> {
    service
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
}

/// Fetch ingress config for a single tunnel via the CF API
fn fetch_tunnel_config(creds: &ApiCreds, tunnel_id: &str) -> Vec<CfIngress> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        creds.account_id, tunnel_id
    );

    let output = Command::new("curl")
        .args([
            "-s",
            &url,
            "-H",
            &format!("Authorization: Bearer {}", creds.api_token),
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

    resp.result
        .map(|r| r.config.ingress)
        .unwrap_or_default()
}

/// Try multiple sources for API credentials:
/// 1. Config file cf_api_token + account_id from tunnel tokens
/// 2. cert.pem from `cloudflared tunnel login`
fn load_api_creds_multi(cf_api_token: Option<&str>, tunnel_tokens: &[(String, String)]) -> Option<ApiCreds> {
    // Method 1: cf_api_token from config + account_id from any tunnel token
    if let Some(token) = cf_api_token {
        if !token.is_empty() {
            // Extract account_id from the first tunnel token that decodes
            let account_id = tunnel_tokens.iter()
                .filter_map(|(_, tok)| crate::config::decode_token(tok).ok())
                .map(|p| p.account_id)
                .next();

            if let Some(account_id) = account_id {
                return Some(ApiCreds {
                    account_id,
                    api_token: token.to_string(),
                });
            }
        }
    }

    // Method 2: cert.pem
    load_api_creds_from_cert()
}

/// Load API credentials from ~/.cloudflared/cert.pem (Argo Tunnel Token)
fn load_api_creds_from_cert() -> Option<ApiCreds> {
    let cert_path = dirs::home_dir()?.join(".cloudflared/cert.pem");
    let content = std::fs::read_to_string(&cert_path).ok()?;

    // Extract base64 block between ARGO TUNNEL TOKEN markers
    let start = content.find("-----BEGIN ARGO TUNNEL TOKEN-----")?;
    let end = content.find("-----END ARGO TUNNEL TOKEN-----")?;

    let token_start = start + "-----BEGIN ARGO TUNNEL TOKEN-----".len();
    let token_b64: String = content[token_start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&token_b64).ok()?;

    #[derive(Deserialize)]
    struct ArgoToken {
        #[serde(rename = "accountID")]
        account_id: String,
        #[serde(rename = "apiToken")]
        api_token: String,
    }

    let token: ArgoToken = serde_json::from_slice(&bytes).ok()?;
    Some(ApiCreds {
        account_id: token.account_id,
        api_token: token.api_token,
    })
}
