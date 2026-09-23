//! The Cloudflare API, typed.
//!
//! This used to shell out to `curl` for every call, with the bearer token in
//! argv — readable by anything on the box that can run `ps` — and every
//! failure collapsed into "empty list", so a revoked token, a network blip
//! and a tunnel with no routes all looked the same. Here a call either
//! returns what Cloudflare said or an error that says what went wrong.
//!
//! `TUNNELS_CF_API` points the client somewhere else; the tests run a fake
//! Cloudflare on localhost and count what hits it.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";

#[derive(Debug, Clone)]
pub struct CfError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for CfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "Cloudflare said {}: {}", self.status, self.message)
        }
    }
}

impl std::error::Error for CfError {}

pub type CfResult<T> = Result<T, CfError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub account_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Connection {
    #[serde(default)]
    pub colo_name: String,
    #[serde(default)]
    pub origin_ip: String,
    #[serde(default)]
    pub client_version: String,
    #[serde(default)]
    pub opened_at: String,
    #[serde(default)]
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Tunnel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub conns_active_at: Option<String>,
    #[serde(default)]
    pub conns_inactive_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub account_tag: String,
    #[serde(default)]
    pub remote_config: bool,
}

/// One ingress rule, as Cloudflare holds it. The whole rule is kept in
/// `raw` so that writing a tunnel's config back never drops what this tool
/// does not model — `originRequest`, `path`, whatever comes next. The old
/// code rebuilt every rule from hostname + service and silently erased the
/// rest on each route change.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingress {
    pub hostname: Option<String>,
    pub service: String,
    pub path: Option<String>,
    pub raw: Value,
}

impl Ingress {
    pub fn new(hostname: &str, service: &str) -> Self {
        Ingress {
            hostname: Some(hostname.to_string()),
            service: service.to_string(),
            path: None,
            raw: serde_json::json!({ "hostname": hostname, "service": service }),
        }
    }

    pub fn catch_all(service: &str) -> Self {
        Ingress {
            hostname: None,
            service: service.to_string(),
            path: None,
            raw: serde_json::json!({ "service": service }),
        }
    }

    fn from_value(v: Value) -> Option<Self> {
        let service = v.get("service")?.as_str()?.to_string();
        let hostname = v.get("hostname").and_then(|h| h.as_str()).map(String::from);
        let path = v.get("path").and_then(|h| h.as_str()).map(String::from);
        Some(Ingress { hostname, service, path, raw: v })
    }

    /// The same rule pointing at a different service, everything else kept.
    pub fn with_service(&self, service: &str) -> Self {
        let mut raw = self.raw.clone();
        raw["service"] = Value::String(service.to_string());
        Ingress { service: service.to_string(), raw, ..self.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsRecord {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub rtype: String,
    pub content: String,
    #[serde(default)]
    pub proxied: bool,
    #[serde(default)]
    pub zone_id: String,
}

impl DnsRecord {
    /// The tunnel a CNAME sends traffic to, if it is a tunnel CNAME.
    pub fn tunnel_target(&self) -> Option<String> {
        tunnel_of_target(&self.content)
    }
}

pub fn tunnel_of_target(content: &str) -> Option<String> {
    content
        .strip_suffix(".cfargotunnel.com")
        .map(|s| s.to_ascii_lowercase())
}

pub fn target_for(tunnel_id: &str) -> String {
    format!("{tunnel_id}.cfargotunnel.com")
}

#[derive(Clone)]
pub struct Client {
    token: String,
    base: String,
    http: ureq::Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cf::Client({})", crate::config::hint(&self.token))
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<Value>,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    result_info: Option<ResultInfo>,
}

#[derive(Deserialize)]
struct ResultInfo {
    #[serde(default)]
    total_pages: Option<u32>,
}

pub fn base_url() -> String {
    std::env::var("TUNNELS_CF_API").unwrap_or_else(|_| DEFAULT_BASE.to_string())
}

impl Client {
    pub fn new(token: &str) -> Self {
        crate::scope::assert_cloudflare_allowed();
        let http: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        Client { token: token.to_string(), base: base_url(), http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn send(&self, method: &str, path: &str, body: Option<Value>) -> CfResult<(Value, Option<ResultInfo>)> {
        let url = self.url(path);
        let auth = format!("Bearer {}", self.token);
        let res = match method {
            "GET" => self.http.get(&url).header("Authorization", &auth).call(),
            "DELETE" => self.http.delete(&url).header("Authorization", &auth).call(),
            "PUT" => self
                .http
                .put(&url)
                .header("Authorization", &auth)
                .send_json(body.unwrap_or(Value::Null)),
            "POST" => self
                .http
                .post(&url)
                .header("Authorization", &auth)
                .send_json(body.unwrap_or(Value::Null)),
            "PATCH" => self
                .http
                .patch(&url)
                .header("Authorization", &auth)
                .send_json(body.unwrap_or(Value::Null)),
            _ => unreachable!("method {method}"),
        };
        let mut resp = res.map_err(|e| CfError { status: 0, message: format!("could not reach Cloudflare: {e}") })?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| CfError { status, message: format!("reading response: {e}") })?;
        let env: Envelope = serde_json::from_str(&text).map_err(|_| CfError {
            status,
            message: format!("unexpected response: {}", text.chars().take(200).collect::<String>()),
        })?;
        if !env.success || status >= 400 {
            let message = env
                .errors
                .iter()
                .map(|e| {
                    let code = e.get("code").and_then(|c| c.as_u64());
                    let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("?");
                    match code {
                        Some(c) => format!("[{c}] {msg}"),
                        None => msg.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(CfError {
                status,
                message: if message.is_empty() { "request failed".into() } else { message },
            });
        }
        Ok((env.result, env.result_info))
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> CfResult<T> {
        let (v, _) = self.send("GET", path, None)?;
        serde_json::from_value(v).map_err(|e| CfError { status: 0, message: format!("decoding {path}: {e}") })
    }

    /// Every page of a list endpoint. `path` must already carry a `?`.
    fn get_all<T: serde::de::DeserializeOwned>(&self, path: &str) -> CfResult<Vec<T>> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let (v, info) = self.send("GET", &format!("{path}&per_page=100&page={page}"), None)?;
            let items: Vec<T> = serde_json::from_value(v)
                .map_err(|e| CfError { status: 0, message: format!("decoding {path}: {e}") })?;
            let n = items.len();
            out.extend(items);
            let total = info.and_then(|i| i.total_pages).unwrap_or(1);
            if n == 0 || page >= total {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    pub fn verify(&self) -> CfResult<()> {
        self.send("GET", "/user/tokens/verify", None).map(|_| ())
    }

    pub fn accounts(&self) -> CfResult<Vec<Account>> {
        self.get_all("/accounts?direction=asc")
    }

    pub fn zones(&self) -> CfResult<Vec<Zone>> {
        #[derive(Deserialize)]
        struct Z {
            id: String,
            name: String,
            account: Account,
        }
        let zs: Vec<Z> = self.get_all("/zones?status=active")?;
        Ok(zs
            .into_iter()
            .map(|z| Zone { id: z.id, name: z.name, account_id: z.account.id, account_name: z.account.name })
            .collect())
    }

    pub fn tunnels(&self, account_id: &str) -> CfResult<Vec<Tunnel>> {
        self.get_all(&format!("/accounts/{account_id}/cfd_tunnel?is_deleted=false"))
    }

    pub fn tunnel(&self, account_id: &str, tunnel_id: &str) -> CfResult<Tunnel> {
        self.get(&format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}"))
    }

    /// A tunnel's ingress. A tunnel that has never been given any comes back
    /// as `"config": null` — that is an empty list, not an error.
    pub fn ingress(&self, account_id: &str, tunnel_id: &str) -> CfResult<Vec<Ingress>> {
        let v: Value = self.get(&format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"))?;
        Ok(v.get("config")
            .and_then(|c| c.get("ingress"))
            .and_then(|i| i.as_array())
            .map(|a| a.iter().cloned().filter_map(Ingress::from_value).collect())
            .unwrap_or_default())
    }

    /// Replace a tunnel's ingress. Cloudflare wants a catch-all last; one is
    /// added if the list has none.
    pub fn put_ingress(&self, account_id: &str, tunnel_id: &str, rules: &[Ingress]) -> CfResult<()> {
        let mut ingress: Vec<Value> = rules.iter().map(|r| r.raw.clone()).collect();
        if !rules.last().map(|r| r.hostname.is_none() && r.path.is_none()).unwrap_or(false) {
            ingress.push(serde_json::json!({ "service": "http_status:404" }));
        }
        self.send(
            "PUT",
            &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"),
            Some(serde_json::json!({ "config": { "ingress": ingress } })),
        )
        .map(|_| ())
    }

    pub fn dns_records(&self, zone_id: &str) -> CfResult<Vec<DnsRecord>> {
        let mut rs: Vec<DnsRecord> = self.get_all(&format!("/zones/{zone_id}/dns_records?type=CNAME"))?;
        for r in &mut rs {
            r.zone_id = zone_id.to_string();
        }
        Ok(rs)
    }

    /// Every record at exactly this name, any type.
    pub fn dns_records_named(&self, zone_id: &str, name: &str) -> CfResult<Vec<DnsRecord>> {
        let mut rs: Vec<DnsRecord> = self.get_all(&format!("/zones/{zone_id}/dns_records?name={name}"))?;
        for r in &mut rs {
            r.zone_id = zone_id.to_string();
        }
        Ok(rs)
    }

    pub fn create_cname(&self, zone_id: &str, name: &str, tunnel_id: &str) -> CfResult<DnsRecord> {
        let (v, _) = self.send(
            "POST",
            &format!("/zones/{zone_id}/dns_records"),
            Some(serde_json::json!({
                "type": "CNAME", "name": name, "content": target_for(tunnel_id), "proxied": true,
            })),
        )?;
        let mut r: DnsRecord =
            serde_json::from_value(v).map_err(|e| CfError { status: 0, message: format!("decoding record: {e}") })?;
        r.zone_id = zone_id.to_string();
        Ok(r)
    }

    pub fn update_cname(&self, zone_id: &str, record_id: &str, name: &str, content: &str) -> CfResult<()> {
        self.send(
            "PUT",
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            Some(serde_json::json!({ "type": "CNAME", "name": name, "content": content, "proxied": true })),
        )
        .map(|_| ())
    }

    pub fn delete_record(&self, zone_id: &str, record_id: &str) -> CfResult<()> {
        self.send("DELETE", &format!("/zones/{zone_id}/dns_records/{record_id}"), None).map(|_| ())
    }

    /// The connector token for a tunnel — what `cloudflared tunnel run
    /// --token` takes. A machine holding an API token for the account can
    /// fetch this itself, so connector tokens never have to be copied around.
    pub fn connector_token(&self, account_id: &str, tunnel_id: &str) -> CfResult<String> {
        self.get(&format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/token"))
    }

    pub fn create_tunnel(&self, account_id: &str, name: &str) -> CfResult<Tunnel> {
        let (v, _) = self.send(
            "POST",
            &format!("/accounts/{account_id}/cfd_tunnel"),
            Some(serde_json::json!({
                "name": name, "config_src": "cloudflare", "tunnel_secret": new_secret(),
            })),
        )?;
        serde_json::from_value(v).map_err(|e| CfError { status: 0, message: format!("decoding tunnel: {e}") })
    }

    /// Give the tunnel a new secret. Every connector token issued before this
    /// stops working; connectors already running keep their sessions until
    /// they reconnect.
    pub fn rotate_secret(&self, account_id: &str, tunnel_id: &str) -> CfResult<()> {
        self.send(
            "PATCH",
            &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
            Some(serde_json::json!({ "tunnel_secret": new_secret() })),
        )
        .map(|_| ())
    }

    pub fn delete_connections(&self, account_id: &str, tunnel_id: &str) -> CfResult<()> {
        self.send("DELETE", &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/connections"), None)
            .map(|_| ())
    }

    pub fn delete_tunnel(&self, account_id: &str, tunnel_id: &str) -> CfResult<()> {
        self.send("DELETE", &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}"), None).map(|_| ())
    }
}

/// 32 random bytes, base64 — what Cloudflare wants as a tunnel secret.
fn new_secret() -> String {
    use base64::Engine;
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("reading /dev/urandom");
    base64::engine::general_purpose::STANDARD.encode(buf)
}

/// The zone a hostname lives in: the longest zone name it ends with.
pub fn zone_for<'a>(host: &str, zones: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let host = host.to_ascii_lowercase();
    zones
        .into_iter()
        .filter(|z| {
            let z = z.to_ascii_lowercase();
            host == z || host.ends_with(&format!(".{z}"))
        })
        .max_by_key(|z| z.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_is_the_longest_suffix_on_a_label_boundary() {
        let zones = ["felixflor.es", "everyday.vet", "vet"];
        assert_eq!(zone_for("admin.everyday.vet", zones), Some("everyday.vet"));
        assert_eq!(zone_for("everyday.vet", zones), Some("everyday.vet"));
        // a suffix that is not a label boundary is not the zone
        assert_eq!(zone_for("notfelixflor.es", ["felixflor.es"]), None);
        assert_eq!(zone_for("x.example.com", zones), None);
    }

    #[test]
    fn a_tunnel_cname_names_its_tunnel() {
        assert_eq!(tunnel_of_target("abc-123.cfargotunnel.com"), Some("abc-123".into()));
        assert_eq!(tunnel_of_target("example.com"), None);
    }

    #[test]
    fn rewriting_a_rule_keeps_what_we_do_not_model() {
        let raw = serde_json::json!({
            "hostname": "a.example.com", "service": "http://localhost:1",
            "originRequest": { "noTLSVerify": true }
        });
        let r = Ingress::from_value(raw).unwrap();
        let moved = r.with_service("http://localhost:2");
        assert_eq!(moved.raw["originRequest"]["noTLSVerify"], true);
        assert_eq!(moved.raw["service"], "http://localhost:2");
    }

    #[test]
    fn secrets_are_32_bytes() {
        use base64::Engine;
        let s = new_secret();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(s).unwrap().len(), 32);
        assert_ne!(new_secret(), new_secret());
    }
}
