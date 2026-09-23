//! Small things with no better home: time, hostnames, atomic writes.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Seconds since the epoch as `2026-09-23T20:15:00Z`.
pub fn rfc3339(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

pub fn now_rfc3339() -> String {
    rfc3339(now_epoch())
}

/// Parse the timestamps Cloudflare hands back (`2026-09-23T20:15:00Z`,
/// with or without fractional seconds or a `+00:00` offset).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut t = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se;
    // an offset, if there is one, after any fraction
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(off) = rest.strip_prefix('+').map(|o| (1, o)).or_else(|| rest.strip_prefix('-').map(|o| (-1, o))) {
        let (sign, o) = off;
        let oh = o.get(0..2)?.parse::<i64>().ok()?;
        let om = o.get(3..5).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
        t -= sign * (oh * 3600 + om * 60);
    }
    Some(t)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// How long ago, for people: `4m`, `3h`, `2d`.
pub fn ago(epoch: i64) -> String {
    let s = (now_epoch() - epoch).max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// `hostname -s`, lowercased.
pub fn short_hostname() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    let name = if ok {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).to_string()
    } else {
        String::new()
    };
    name.split('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Write via a temporary file and rename, so a reader never sees half a file
/// and a crash mid-write leaves the old one intact.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Is `ip` a tailnet address (100.64.0.0/10, or Tailscale's IPv6 range) or loopback?
pub fn is_tailnet_or_loopback(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback() || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_tailnet_or_loopback(&std::net::IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback() || (s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0)
        }
    }
}

/// This machine's tailnet IPv4 address, found on its interfaces — no need
/// for the tailscale CLI, which lives in different places on different Macs.
pub fn tailnet_ipv4() -> Option<std::net::Ipv4Addr> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut cur = ifap;
        let mut found = None;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let o = ip.octets();
                if o[0] == 100 && (o[1] & 0xC0) == 64 {
                    found = Some(ip);
                    break;
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_round_trips() {
        let t = parse_rfc3339("2026-09-23T20:15:07Z").unwrap();
        assert_eq!(rfc3339(t), "2026-09-23T20:15:07Z");
        assert_eq!(parse_rfc3339("2026-09-23T20:15:07.123456Z"), Some(t));
        assert_eq!(parse_rfc3339("2026-09-23T22:15:07+02:00"), Some(t));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn tailnet_addresses() {
        let ok = |s: &str| is_tailnet_or_loopback(&s.parse().unwrap());
        assert!(ok("100.97.227.67"));
        assert!(ok("100.64.0.1"));
        assert!(ok("127.0.0.1"));
        assert!(ok("::1"));
        assert!(ok("fd7a:115c:a1e0::1"));
        assert!(!ok("100.128.0.1"));
        assert!(!ok("192.168.1.10"));
        assert!(!ok("8.8.8.8"));
    }
}
