//! Who is asking, when the web UI is reached through Cloudflare.
//!
//! `tunnels.felixflor.es` sits behind Cloudflare Access, which signs every
//! request it lets through with a JWT in `Cf-Access-Jwt-Assertion`. The
//! agent does not take Access's word for it: it checks the signature against
//! the team's published keys, the audience, the issuer and the expiry, and
//! reads the email from the token itself. Admin rights come from
//! `[policy.web] admins`, not from the Access policy — so a policy loosened
//! by mistake lets more people look, never more people change things.
//!
//! On the tailnet nothing here applies: the tailnet is already the boundary,
//! and it is the way in when Cloudflare or pocket-id is down.

use crate::fleet::WebPolicy;
use anyhow::{Result, anyhow, bail};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Identity {
    /// an email for someone signed in through Access; "tailnet" otherwise
    pub who: String,
    pub admin: bool,
    pub via_cloudflare: bool,
}

impl Identity {
    pub fn tailnet() -> Self {
        Identity { who: "tailnet".into(), admin: true, via_cloudflare: false }
    }
}

#[derive(Debug, Deserialize)]
struct Claims {
    #[serde(default)]
    email: String,
}

/// The team's signing keys, cached; fetched again when a token names a key
/// we have not seen (Cloudflare rotates them), at most once a minute.
static KEYS: Mutex<Option<(i64, String, JwkSet)>> = Mutex::new(None);

fn certs_url(team_domain: &str) -> String {
    std::env::var("TUNNELS_ACCESS_CERTS_URL").unwrap_or_else(|_| format!("https://{team_domain}/cdn-cgi/access/certs"))
}

fn fetch_keys(team_domain: &str) -> Result<JwkSet> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
    let set: JwkSet = agent
        .get(&certs_url(team_domain))
        .call()
        .map_err(|e| anyhow!("fetching Access keys: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| anyhow!("reading Access keys: {e}"))?;
    Ok(set)
}

fn key_for(team_domain: &str, kid: &str) -> Result<DecodingKey> {
    let now = crate::util::now_epoch();
    let mut cache = KEYS.lock().unwrap();
    let stale = match cache.as_ref() {
        Some((_, d, set)) if d == team_domain => set.find(kid).is_none(),
        _ => true,
    };
    if stale && cache.as_ref().map(|(at, _, _)| now - at >= 60).unwrap_or(true) {
        let set = fetch_keys(team_domain)?;
        *cache = Some((now, team_domain.to_string(), set));
    }
    let (_, _, set) = cache.as_ref().ok_or_else(|| anyhow!("no Access keys"))?;
    let jwk = set.find(kid).ok_or_else(|| anyhow!("the token was signed with a key Access does not publish"))?;
    DecodingKey::from_jwk(jwk).map_err(|e| anyhow!("Access key: {e}"))
}

/// Check an Access token and say who it is for.
pub fn verify(token: &str, web: &WebPolicy) -> Result<Identity> {
    if web.team_domain.is_empty() || web.aud.is_empty() {
        bail!("the fleet file's [policy.web] has no team_domain/aud yet, so no Access token can be checked");
    }
    let header = decode_header(token).map_err(|e| anyhow!("not an Access token: {e}"))?;
    if header.alg != Algorithm::RS256 {
        bail!("unexpected signing algorithm {:?}", header.alg);
    }
    let kid = header.kid.ok_or_else(|| anyhow!("the token names no key"))?;
    let key = key_for(&web.team_domain, &kid)?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_audience(&[web.aud.as_str()]);
    v.set_issuer(&[format!("https://{}", web.team_domain)]);
    v.leeway = 30;
    let data = decode::<Claims>(token, &key, &v).map_err(|e| anyhow!("Access token rejected: {e}"))?;
    let email = data.claims.email.to_ascii_lowercase();
    if email.is_empty() {
        bail!("the Access token carries no email");
    }
    let admin = web.admins.iter().any(|a| a.eq_ignore_ascii_case(&email));
    Ok(Identity { who: email, admin, via_cloudflare: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    const KEY: &str = include_str!("../tests/fixtures/access-test-key.pem");
    const CERTS: &str = include_str!("../tests/fixtures/access-test-certs.json");

    fn web() -> WebPolicy {
        WebPolicy {
            public_host: "tunnels.example.com".into(),
            team_domain: "team.example.com".into(),
            aud: "aud-1".into(),
            admins: vec!["Admin@Example.com".into()],
        }
    }

    fn token(email: &str, aud: &str, iss: &str, exp_in: i64) -> String {
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some("test-kid".into());
        let claims = serde_json::json!({
            "email": email, "aud": [aud], "iss": iss,
            "exp": crate::util::now_epoch() + exp_in, "iat": crate::util::now_epoch(),
        });
        encode(&h, &claims, &EncodingKey::from_rsa_pem(KEY.as_bytes()).unwrap()).unwrap()
    }

    fn with_keys() {
        let set: JwkSet = serde_json::from_str(CERTS).unwrap();
        *KEYS.lock().unwrap() = Some((crate::util::now_epoch(), "team.example.com".into(), set));
    }

    #[test]
    fn an_admin_is_named_in_the_fleet_file_and_matched_without_case() {
        with_keys();
        let id = verify(&token("admin@example.com", "aud-1", "https://team.example.com", 300), &web()).unwrap();
        assert_eq!(id, Identity { who: "admin@example.com".into(), admin: true, via_cloudflare: true });
    }

    #[test]
    fn anyone_else_signed_in_can_only_look() {
        with_keys();
        let id = verify(&token("guest@example.com", "aud-1", "https://team.example.com", 300), &web()).unwrap();
        assert!(!id.admin);
    }

    #[test]
    fn tokens_for_another_app_another_team_or_out_of_date_are_refused() {
        with_keys();
        assert!(verify(&token("admin@example.com", "other-app", "https://team.example.com", 300), &web()).is_err());
        assert!(verify(&token("admin@example.com", "aud-1", "https://evil.example.com", 300), &web()).is_err());
        assert!(verify(&token("admin@example.com", "aud-1", "https://team.example.com", -3600), &web()).is_err());
        assert!(verify("not.a.token", &web()).is_err());
    }

    #[test]
    fn a_token_signed_by_some_other_key_is_refused() {
        with_keys();
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some("test-kid".into());
        let forged = encode(
            &h,
            &serde_json::json!({ "email": "admin@example.com", "aud": ["aud-1"], "iss": "https://team.example.com", "exp": crate::util::now_epoch() + 300 }),
            &EncodingKey::from_secret(b"guess"),
        )
        .unwrap();
        assert!(verify(&forged, &web()).is_err());
    }
}
