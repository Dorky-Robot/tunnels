//! Signing in to the web UI when it is reached from the internet.
//!
//! `tunnels.felixflor.es` is a plain tunnel route, like the other admin sites
//! on the mesh: the agent signs people in itself, with pocket-id, the way
//! admin.homesforsalebymonica.com does. No Cloudflare Access, nothing to set
//! up in the Cloudflare dashboard — only an OIDC client in pocket-id.
//!
//! The flow is the standard one for a public client: authorization code with
//! PKCE (so no client secret lives on any Mac), the ID token checked against
//! pocket-id's published keys (signature, audience, issuer, expiry), the
//! email read from it. Admin rights come from `[policy.web] admins`; anyone
//! else pocket-id signs in may look. Sessions live in the agent's memory:
//! restarting the agent signs everyone out, which is the safe direction.
//!
//! On the tailnet none of this applies: the tailnet is the boundary, and it
//! is the way in when pocket-id is down.

use crate::fleet::WebPolicy;
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

pub const COOKIE: &str = "tunnels_session";
const SESSION_SECS: i64 = 12 * 3600;
const PENDING_SECS: i64 = 600;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Identity {
    /// an email for someone signed in from the internet; "tailnet" otherwise
    pub who: String,
    pub admin: bool,
    /// reached through the public hostname rather than the tailnet
    pub via_cloudflare: bool,
}

impl Identity {
    pub fn tailnet() -> Self {
        Identity { who: "tailnet".into(), admin: true, via_cloudflare: false }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    #[serde(default)]
    email: String,
}

static DISCOVERY: Mutex<Option<(i64, String, Discovery)>> = Mutex::new(None);
static KEYS: Mutex<Option<(i64, String, JwkSet)>> = Mutex::new(None);
/// state → (PKCE verifier, when it was issued)
static PENDING: Mutex<Option<HashMap<String, (String, i64)>>> = Mutex::new(None);
/// session id → (email, expires)
static SESSIONS: Mutex<Option<HashMap<String, (String, i64)>>> = Mutex::new(None);

fn http() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into()
}

fn random(n: usize) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("reading /dev/urandom");
    URL_SAFE_NO_PAD.encode(b)
}

fn challenge(verifier: &str) -> String {
    use sha2::Digest;
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn discovery(issuer: &str) -> Result<Discovery> {
    let now = crate::util::now_epoch();
    let mut cache = DISCOVERY.lock().unwrap();
    if let Some((at, i, d)) = cache.as_ref() {
        if i == issuer && now - at < 3600 {
            return Ok(d.clone());
        }
    }
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let d: Discovery = http()
        .get(&url)
        .call()
        .map_err(|e| anyhow!("asking {issuer} how to sign in: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| anyhow!("reading {issuer}'s OIDC configuration: {e}"))?;
    *cache = Some((now, issuer.to_string(), d.clone()));
    Ok(d)
}

fn key_for(jwks_uri: &str, kid: Option<&str>) -> Result<DecodingKey> {
    let now = crate::util::now_epoch();
    let mut cache = KEYS.lock().unwrap();
    let known = |set: &JwkSet| match kid {
        Some(k) => set.find(k).is_some(),
        None => !set.keys.is_empty(),
    };
    let stale = match cache.as_ref() {
        Some((_, u, set)) if u == jwks_uri => !known(set),
        _ => true,
    };
    if stale && cache.as_ref().map(|(at, _, _)| now - at >= 60).unwrap_or(true) {
        let set: JwkSet = http()
            .get(jwks_uri)
            .call()
            .map_err(|e| anyhow!("fetching the sign-in keys: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| anyhow!("reading the sign-in keys: {e}"))?;
        *cache = Some((now, jwks_uri.to_string(), set));
    }
    let (_, _, set) = cache.as_ref().ok_or_else(|| anyhow!("no sign-in keys"))?;
    let jwk = match kid {
        Some(k) => set.find(k),
        None => set.keys.first(),
    }
    .ok_or_else(|| anyhow!("the ID token was signed with a key pocket-id does not publish"))?;
    DecodingKey::from_jwk(jwk).map_err(|e| anyhow!("sign-in key: {e}"))
}

fn ready(web: &WebPolicy) -> Result<()> {
    if web.issuer.is_empty() || web.client_id.is_empty() {
        bail!(
            "sign-in is not set up yet: [policy.web] needs issuer and client_id \
             (a public OIDC client in pocket-id with callback https://{}/auth/callback)",
            web.public_host
        );
    }
    Ok(())
}

fn redirect_uri(web: &WebPolicy) -> String {
    format!("https://{}/auth/callback", web.public_host)
}

/// Where to send a browser to sign in. Remembers the PKCE verifier for the
/// state it hands out; the callback must bring that state back.
pub fn login_url(web: &WebPolicy) -> Result<String> {
    ready(web)?;
    let d = discovery(&web.issuer)?;
    let (state, verifier) = (random(24), random(48));
    let now = crate::util::now_epoch();
    {
        let mut p = PENDING.lock().unwrap();
        let map = p.get_or_insert_with(HashMap::new);
        map.retain(|_, (_, at)| now - *at < PENDING_SECS);
        map.insert(state.clone(), (verifier.clone(), now));
    }
    Ok(format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        d.authorization_endpoint,
        enc(&web.client_id),
        enc(&redirect_uri(web)),
        enc("openid email profile"),
        enc(&state),
        challenge(&verifier),
    ))
}

/// Finish a sign-in: trade the code for an ID token, check it, start a
/// session. Returns the session id and who it is for.
pub fn finish(web: &WebPolicy, code: &str, state: &str) -> Result<(String, Identity)> {
    ready(web)?;
    let verifier = {
        let mut p = PENDING.lock().unwrap();
        let map = p.get_or_insert_with(HashMap::new);
        let (v, at) = map.remove(state).ok_or_else(|| anyhow!("this sign-in link is unknown or already used — start again"))?;
        if crate::util::now_epoch() - at > PENDING_SECS {
            bail!("this sign-in took too long — start again");
        }
        v
    };
    let d = discovery(&web.issuer)?;
    let redirect = redirect_uri(web);
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("client_id", web.client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    let resp: serde_json::Value = http()
        .post(&d.token_endpoint)
        .send_form(form)
        .map_err(|e| anyhow!("pocket-id refused the sign-in: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| anyhow!("reading pocket-id's answer: {e}"))?;
    let id_token = resp.get("id_token").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("pocket-id sent no ID token"))?;
    let email = verify_id_token(id_token, web, &d.jwks_uri)?;
    let sid = random(32);
    SESSIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(sid.clone(), (email.clone(), crate::util::now_epoch() + SESSION_SECS));
    let admin = web.admins.iter().any(|a| a.eq_ignore_ascii_case(&email));
    Ok((sid, Identity { who: email, admin, via_cloudflare: true }))
}

fn verify_id_token(token: &str, web: &WebPolicy, jwks_uri: &str) -> Result<String> {
    let header = decode_header(token).map_err(|e| anyhow!("not an ID token: {e}"))?;
    if header.alg != Algorithm::RS256 {
        bail!("unexpected signing algorithm {:?}", header.alg);
    }
    let key = key_for(jwks_uri, header.kid.as_deref())?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_audience(&[web.client_id.as_str()]);
    v.set_issuer(&[web.issuer.trim_end_matches('/'), web.issuer.as_str()]);
    v.leeway = 30;
    let data = decode::<Claims>(token, &key, &v).map_err(|e| anyhow!("ID token rejected: {e}"))?;
    let email = data.claims.email.to_ascii_lowercase();
    if email.is_empty() {
        bail!("the ID token carries no email — the pocket-id client needs the email scope");
    }
    Ok(email)
}

/// Who a session cookie belongs to, if it is a live session.
pub fn session(web: &WebPolicy, cookie_header: Option<&str>) -> Option<Identity> {
    let sid = cookie_header?
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())?;
    let now = crate::util::now_epoch();
    let mut s = SESSIONS.lock().unwrap();
    let map = s.get_or_insert_with(HashMap::new);
    map.retain(|_, (_, exp)| *exp > now);
    let (email, _) = map.get(&sid)?;
    let admin = web.admins.iter().any(|a| a.eq_ignore_ascii_case(email));
    Some(Identity { who: email.clone(), admin, via_cloudflare: true })
}

pub fn logout(cookie_header: Option<&str>) {
    let Some(h) = cookie_header else { return };
    if let Some((_, sid)) = h.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == COOKIE) {
        SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).remove(sid);
    }
}

pub fn set_cookie(sid: &str) -> String {
    format!("{COOKIE}={sid}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={SESSION_SECS}")
}

pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
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
            issuer: "https://id.example.com".into(),
            client_id: "tunnels".into(),
            admins: vec!["Admin@Example.com".into()],
        }
    }

    fn token(email: &str, aud: &str, iss: &str, exp_in: i64) -> String {
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some("test-kid".into());
        let claims = serde_json::json!({
            "email": email, "aud": aud, "iss": iss,
            "exp": crate::util::now_epoch() + exp_in, "iat": crate::util::now_epoch(),
        });
        encode(&h, &claims, &EncodingKey::from_rsa_pem(KEY.as_bytes()).unwrap()).unwrap()
    }

    fn with_keys() {
        let set: JwkSet = serde_json::from_str(CERTS).unwrap();
        *KEYS.lock().unwrap() = Some((crate::util::now_epoch(), "jwks".into(), set));
    }

    #[test]
    fn a_good_id_token_names_its_email() {
        with_keys();
        assert_eq!(verify_id_token(&token("Admin@Example.com", "tunnels", "https://id.example.com", 300), &web(), "jwks").unwrap(), "admin@example.com");
    }

    #[test]
    fn id_tokens_for_another_client_another_issuer_or_out_of_date_are_refused() {
        with_keys();
        assert!(verify_id_token(&token("a@example.com", "other", "https://id.example.com", 300), &web(), "jwks").is_err());
        assert!(verify_id_token(&token("a@example.com", "tunnels", "https://evil.example.com", 300), &web(), "jwks").is_err());
        assert!(verify_id_token(&token("a@example.com", "tunnels", "https://id.example.com", -3600), &web(), "jwks").is_err());
        assert!(verify_id_token("not.a.token", &web(), "jwks").is_err());
    }

    #[test]
    fn a_token_signed_by_some_other_key_is_refused() {
        with_keys();
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some("test-kid".into());
        let forged = encode(
            &h,
            &serde_json::json!({ "email": "admin@example.com", "aud": "tunnels", "iss": "https://id.example.com", "exp": crate::util::now_epoch() + 300 }),
            &EncodingKey::from_secret(b"guess"),
        )
        .unwrap();
        assert!(verify_id_token(&forged, &web(), "jwks").is_err());
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        // RFC 7636, appendix B
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn a_session_is_found_by_its_cookie_and_gone_after_logout() {
        let sid = "test-session-id";
        SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(sid.into(), ("guest@example.com".into(), crate::util::now_epoch() + 60));
        let cookie = format!("a=b; {COOKIE}={sid}");
        let who = session(&web(), Some(&cookie)).unwrap();
        assert_eq!((who.who.as_str(), who.admin), ("guest@example.com", false));
        logout(Some(&cookie));
        assert!(session(&web(), Some(&cookie)).is_none());
    }

    #[test]
    fn signing_in_before_it_is_set_up_says_what_is_missing() {
        let w = WebPolicy { public_host: "tunnels.example.com".into(), ..Default::default() };
        let e = login_url(&w).unwrap_err().to_string();
        assert!(e.contains("client_id") && e.contains("https://tunnels.example.com/auth/callback"), "{e}");
    }
}
