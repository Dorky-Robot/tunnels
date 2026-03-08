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

/// API credentials extracted from cert.pem
struct ApiCreds {
    account_id: String,
    api_token: String,
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

/// Fetch all ingress routes for all known tunnels.
/// Returns a map of local port -> Vec<IngressRoute> since multiple tunnels can serve the same port.
pub fn fetch_ingress_routes(tunnels: &[CfTunnel]) -> HashMap<u16, Vec<IngressRoute>> {
    let creds = match load_api_creds() {
        Some(c) => c,
        None => return HashMap::new(),
    };

    let mut port_map: HashMap<u16, Vec<IngressRoute>> = HashMap::new();

    for tunnel in tunnels {
        let ingress = fetch_tunnel_config(&creds, &tunnel.id);
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
    // service is like "http://localhost:3001", "ssh://localhost:22", "http_status:404"
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

/// Load API credentials from ~/.cloudflared/cert.pem (Argo Tunnel Token)
fn load_api_creds() -> Option<ApiCreds> {
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
