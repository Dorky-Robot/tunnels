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

/// API credentials for Cloudflare (scoped to one account)
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

/// Full sync: list tunnels + fetch ingress routes across all accounts.
/// cf_api_tokens: user-configured API tokens (one per CF account)
/// tunnel_tokens: Vec<(config_name, base64_token)>
pub fn sync(cf_api_tokens: &[&str], tunnel_tokens: &[(String, String)]) -> SyncResult {
    let cf_tunnels = list_tunnels();

    // Decode all tunnel tokens to get (name, account_id, tunnel_id) triples
    let decoded: Vec<(String, String, String)> = tunnel_tokens.iter()
        .filter_map(|(name, tok)| {
            crate::config::decode_token(tok).ok().map(|p| {
                (name.clone(), p.account_id, p.tunnel_id)
            })
        })
        .collect();

    // Collect all available API tokens: user-configured + cert.pem
    let mut api_tokens: Vec<String> = cf_api_tokens.iter()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    if let Some(cert_creds) = load_api_creds_from_cert() {
        if !api_tokens.iter().any(|t| t == &cert_creds.api_token) {
            api_tokens.push(cert_creds.api_token);
        }
    }

    if api_tokens.is_empty() {
        let status = if cf_tunnels.is_empty() {
            "No CF auth — set cf_api_tokens in config (one per account)".to_string()
        } else {
            format!("Synced {} tunnel(s) — set cf_api_tokens for ingress routes", cf_tunnels.len())
        };
        return SyncResult { tunnels: cf_tunnels, ingress_routes: HashMap::new(), status };
    }

    // Group tunnels by account_id for multi-account support
    let mut accounts: HashMap<String, Vec<(String, String)>> = HashMap::new(); // account_id -> [(name, tunnel_id)]
    for (name, account_id, tunnel_id) in &decoded {
        accounts.entry(account_id.clone())
            .or_default()
            .push((name.clone(), tunnel_id.clone()));
    }

    // Also add tunnels from `cloudflared tunnel list` that aren't in our config
    // (they're from the cert.pem account)
    if let Some(cert_creds) = load_api_creds_from_cert() {
        let entry = accounts.entry(cert_creds.account_id).or_default();
        for cf in &cf_tunnels {
            if !entry.iter().any(|(_, id)| id == &cf.id) {
                entry.push((cf.name.clone(), cf.id.clone()));
            }
        }
    }

    let mut port_map: HashMap<u16, Vec<IngressRoute>> = HashMap::new();
    let mut total_routes = 0;
    let mut accounts_reached = 0;
    let mut unreached_accounts = 0;

    for (account_id, tunnels) in &accounts {
        // Try each API token until one succeeds for this account
        let mut account_ok = false;
        for api_token in &api_tokens {
            let creds = ApiCreds {
                account_id: account_id.clone(),
                api_token: api_token.clone(),
            };

            // Probe: try the first tunnel to check if this token has access
            let probe_ok = tunnels.first()
                .map(|(_, id)| fetch_tunnel_config_check(&creds, id))
                .unwrap_or(false);

            if !probe_ok {
                continue; // this token can't access this account, try next
            }

            // This token works — fetch ingress for all tunnels in this account
            for (name, tunnel_id) in tunnels {
                let ingress = fetch_tunnel_config(&creds, tunnel_id);
                for rule in ingress {
                    let hostname = match rule.hostname {
                        Some(h) => h,
                        None => continue,
                    };

                    let route = IngressRoute {
                        hostname,
                        tunnel_name: name.clone(),
                        tunnel_id: tunnel_id.clone(),
                    };

                    if let Some(p) = parse_port_from_service(&rule.service) {
                        total_routes += 1;
                        port_map.entry(p).or_default().push(route);
                    }
                }
            }

            account_ok = true;
            accounts_reached += 1;
            break;
        }

        if !account_ok {
            unreached_accounts += 1;
        }
    }

    let status = if unreached_accounts > 0 {
        format!(
            "Synced {} route(s) from {} account(s) — {} account(s) need cf_api_tokens",
            total_routes, accounts_reached, unreached_accounts,
        )
    } else {
        format!(
            "Synced {} route(s) from {} account(s)",
            total_routes, accounts_reached,
        )
    };

    SyncResult { tunnels: cf_tunnels, ingress_routes: port_map, status }
}

/// Parse port from a service URL like "http://localhost:3001" or "ssh://localhost:22"
fn parse_port_from_service(service: &str) -> Option<u16> {
    service
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
}

/// Check if an API token has access to a tunnel's account (returns true if API responds with success)
fn fetch_tunnel_config_check(creds: &ApiCreds, tunnel_id: &str) -> bool {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel/{}/configurations",
        creds.account_id, tunnel_id
    );

    let output = Command::new("curl")
        .args(["-s", &url, "-H", &format!("Authorization: Bearer {}", creds.api_token)])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return false,
    };

    // Just check if the API returned success (vs auth error)
    serde_json::from_slice::<CfConfigResponse>(&output)
        .map(|r| r.success)
        .unwrap_or(false)
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
fn load_api_creds_from_cert() -> Option<ApiCreds> {
    let cert_path = dirs::home_dir()?.join(".cloudflared/cert.pem");
    let content = std::fs::read_to_string(&cert_path).ok()?;

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
