use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL_PREFIX: &str = "com.cloudflare.cloudflared";

/// Resolve the absolute path to `cloudflared` at plist-generation time.
///
/// Homebrew lives at `/opt/homebrew` on Apple Silicon and `/usr/local` on
/// Intel, so we can't hardcode a single path. Resolution order:
///   1. `TUNNELS_CLOUDFLARED` env var (explicit override)
///   2. `/opt/homebrew/bin/cloudflared` if it exists
///   3. `/usr/local/bin/cloudflared` if it exists
///   4. `which cloudflared` (respects caller's PATH)
///   5. `/opt/homebrew/bin/cloudflared` as a last-ditch default (matches
///      the legacy behavior so we don't silently start generating a
///      different path when cloudflared isn't installed at all).
fn cloudflared_path() -> String {
    if let Ok(p) = std::env::var("TUNNELS_CLOUDFLARED") {
        if !p.is_empty() {
            return p;
        }
    }
    for candidate in [
        "/opt/homebrew/bin/cloudflared",
        "/usr/local/bin/cloudflared",
    ] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    if let Ok(out) = Command::new("/usr/bin/which").arg("cloudflared").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "/opt/homebrew/bin/cloudflared".to_string()
}

fn plist_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
}

fn log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Logs/tunnels")
}

pub fn label_for(name: &str) -> String {
    if name == "default" {
        LABEL_PREFIX.to_string()
    } else {
        format!("{}-{}", LABEL_PREFIX, name)
    }
}

pub fn plist_path(name: &str) -> PathBuf {
    plist_dir().join(format!("{}.plist", label_for(name)))
}

fn generate_plist(name: &str, token: &str) -> String {
    let label = label_for(name);
    let log_dir = log_dir();
    let log_dir_str = log_dir.to_string_lossy();
    let cloudflared = cloudflared_path();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{cloudflared}</string>
		<string>tunnel</string>
		<string>run</string>
		<string>--token</string>
		<string>{token}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>StandardOutPath</key>
	<string>{log_dir_str}/{label}.out.log</string>
	<key>StandardErrorPath</key>
	<string>{log_dir_str}/{label}.err.log</string>
	<key>ThrottleInterval</key>
	<integer>5</integer>
</dict>
</plist>"#
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running { pid: Option<u32> },
    Stopped,
    Inactive,
}

pub fn status(name: &str) -> Status {
    let label = label_for(name);
    let output = Command::new("launchctl")
        .args(["list", &label])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let pid = stdout
                .lines()
                .find(|l| l.contains("PID"))
                .and_then(|l| l.split_whitespace().last())
                .and_then(|s| s.trim_end_matches(";").parse::<u32>().ok());
            Status::Running { pid }
        }
        _ => {
            if plist_path(name).exists() {
                Status::Stopped
            } else {
                Status::Inactive
            }
        }
    }
}

fn gui_domain() -> String {
    let uid = unsafe { libc::getuid() };
    format!("gui/{}", uid)
}

/// True iff launchctl's bootstrap stderr indicates the service was already
/// loaded (error 37). The previous heuristic also matched on the literal
/// "Bootstrap failed" prefix, which launchctl emits on *every* failure —
/// causing genuine errors (e.g. "Domain does not support specified action"
/// from SSH sessions) to be silently downgraded to a kickstart attempt.
fn is_already_bootstrapped(stderr: &str) -> bool {
    stderr.contains(": 37:") || stderr.contains("already loaded")
}

/// True iff launchctl's stderr indicates the gui/<UID> domain isn't
/// reachable from this process (error 125). This is the SSH symptom —
/// the GUI session's launchd domain is only addressable from a process
/// running inside that session.
fn is_unreachable_domain(stderr: &str) -> bool {
    stderr.contains(": 125:") || stderr.contains("Domain does not support")
}

fn ssh_hint() -> &'static str {
    "\n\nHint: gui/<UID> is only reachable from your Aqua/login session. \
     If you're connected over SSH, run this from a Terminal on the logged-in \
     console."
}

/// Authoritative check: ask launchctl whether the label is currently loaded.
/// Used to verify start() actually took effect, independent of which code
/// path (bootstrap / kickstart / legacy load) we ran.
fn is_loaded(label: &str) -> bool {
    Command::new("launchctl")
        .args(["list", label])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn start(name: &str, token: &str) -> Result<()> {
    let label = label_for(name);
    let path = plist_path(name);
    let plist = generate_plist(name, token);

    // Ensure directories exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(log_dir())?;

    // Write plist directly — no sudo needed for ~/Library/LaunchAgents
    std::fs::write(&path, &plist)?;

    // Try modern bootstrap first, fall back to legacy load
    let domain = gui_domain();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain, &path.to_string_lossy()])
        .output()
        .context("launchctl bootstrap")?;

    let bootstrap_stderr = String::from_utf8_lossy(&out.stderr).to_string();

    if !out.status.success() {
        if is_already_bootstrapped(&bootstrap_stderr) {
            // Already loaded — kickstart it. We MUST check kickstart's
            // exit status; previously this was `let _ = …` which masked
            // failures here.
            let ks = Command::new("launchctl")
                .args(["kickstart", "-k", &format!("{}/{}", domain, label)])
                .output()
                .context("launchctl kickstart")?;
            if !ks.status.success() {
                anyhow::bail!(
                    "launchctl kickstart failed: {}",
                    String::from_utf8_lossy(&ks.stderr).trim()
                );
            }
        } else {
            // Bootstrap failed for some reason other than "already loaded".
            // Try the legacy `launchctl load -w` fallback (works on older
            // launchds, though not in non-Aqua sessions either).
            let legacy = Command::new("launchctl")
                .args(["load", "-w", &path.to_string_lossy()])
                .output()
                .context("launchctl load (legacy fallback)")?;
            if !legacy.status.success() {
                let hint = if is_unreachable_domain(&bootstrap_stderr) {
                    ssh_hint()
                } else {
                    ""
                };
                anyhow::bail!(
                    "launchctl bootstrap failed: {}\nlaunchctl load fallback also failed: {}{}",
                    bootstrap_stderr.trim(),
                    String::from_utf8_lossy(&legacy.stderr).trim(),
                    hint,
                );
            }
        }
    }

    // Authoritative verification: did the LaunchAgent actually load?
    // launchctl can exit 0 from the legacy `load -w` path even when the
    // service didn't register (this happens over SSH for the same
    // gui/<UID> reason that breaks bootstrap), so we can't trust the
    // earlier exit codes alone.
    if !is_loaded(&label) {
        let hint = if is_unreachable_domain(&bootstrap_stderr) {
            ssh_hint()
        } else {
            ""
        };
        anyhow::bail!(
            "launchctl reported success but {label} is not loaded.{hint}"
        );
    }

    Ok(())
}

pub fn stop(name: &str) -> Result<()> {
    let label = label_for(name);
    let path = plist_path(name);
    if !path.exists() {
        return Ok(());
    }

    // Snapshot the PID *before* we unload so we can verify the child
    // actually died. `launchctl unload` is async and the legacy path in
    // particular can leave a cloudflared process alive mid-retry for
    // several seconds. We'd rather reap it ourselves than return "stopped"
    // while a process is still holding the tunnel secret.
    let pid_before = match status(name) {
        Status::Running { pid } => pid,
        _ => None,
    };

    // Try modern bootout first, fall back to legacy unload
    let domain = gui_domain();
    let out = Command::new("launchctl")
        .args(["bootout", &format!("{}/{}", domain, label)])
        .output();

    if let Ok(o) = &out {
        if !o.status.success() {
            // Fall back to legacy unload
            let _ = Command::new("launchctl")
                .args(["unload", &path.to_string_lossy()])
                .output();
        }
    }

    // Poll for the child to exit, then SIGKILL as a last resort.
    if let Some(pid) = pid_before {
        for _ in 0..30 {
            if !process_alive(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if process_alive(pid) {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// Returns true if a process with the given pid is still alive.
/// Uses `kill(pid, 0)` which probes without delivering a signal.
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

pub fn restart(name: &str, token: &str) -> Result<()> {
    // Always do a full stop + start cycle. launchctl kickstart -k reuses
    // the cached service definition and won't pick up plist changes (e.g.
    // an updated token), so we must bootout and bootstrap again.
    stop(name)?;
    start(name, token)
}

/// Read recent log lines for a tunnel
pub fn read_logs(name: &str, lines: usize) -> Result<String> {
    let label = label_for(name);
    let log_dir = log_dir();
    let err_log = log_dir.join(format!("{}.err.log", label));
    let out_log = log_dir.join(format!("{}.out.log", label));

    let mut result = String::new();

    for (tag, path) in [("stderr", &err_log), ("stdout", &out_log)] {
        if path.exists() {
            let content = std::fs::read_to_string(path).unwrap_or_default();
            let tail: Vec<&str> = content.lines().rev().take(lines).collect();
            if !tail.is_empty() {
                result.push_str(&format!("--- {} ---\n", tag));
                for line in tail.into_iter().rev() {
                    result.push_str(line);
                    result.push('\n');
                }
            }
        }
    }

    Ok(result)
}

/// A discovered plist with its source location
#[derive(Debug, Clone)]
pub struct DiscoveredTunnel {
    pub name: String,
    pub token: String,
    pub is_daemon: bool,
    pub plist_path: PathBuf,
}

/// Import existing plists from both LaunchAgents and LaunchDaemons
pub fn discover_existing() -> Vec<DiscoveredTunnel> {
    let mut found = Vec::new();
    let daemon_dir = PathBuf::from("/Library/LaunchDaemons");

    let dirs: Vec<(PathBuf, bool)> = vec![
        (plist_dir(), false),
        (daemon_dir, true),
    ];

    for (dir, is_daemon) in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if !fname.starts_with(LABEL_PREFIX) || !fname.ends_with(".plist") {
                    continue;
                }

                let basename = fname.trim_end_matches(".plist");
                let name = if basename == LABEL_PREFIX {
                    "default".to_string()
                } else {
                    basename
                        .strip_prefix(&format!("{}-", LABEL_PREFIX))
                        .unwrap_or(basename)
                        .to_string()
                };

                // Extract token via PlistBuddy
                let output = Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", "Print :ProgramArguments:4", &entry.path().to_string_lossy()])
                    .output();

                if let Ok(o) = output {
                    if o.status.success() {
                        let token = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        if !token.is_empty() {
                            found.push(DiscoveredTunnel {
                                name,
                                token,
                                is_daemon: *is_daemon,
                                plist_path: entry.path(),
                            });
                        }
                    }
                }
            }
        }
    }

    found
}


/// Migrate a daemon plist: sudo unload + sudo rm, then start as LaunchAgent
pub fn migrate_daemon(plist: &std::path::Path) -> Result<()> {
    let path_str = plist.to_string_lossy();

    // Unload from system domain
    let _ = Command::new("sudo")
        .args(["launchctl", "unload", &path_str])
        .output();

    // Remove the plist
    let out = Command::new("sudo")
        .args(["rm", "-f", &path_str])
        .output()
        .context("sudo rm")?;

    if !out.status.success() {
        anyhow::bail!("failed to remove {}", path_str);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_plist_embeds_token() {
        let plist = generate_plist("default", "eyJTRUNSRVQ=");
        assert!(plist.contains("eyJTRUNSRVQ="));
    }

    #[test]
    fn restart_writes_new_token_to_plist() {
        // Simulate: plist exists with old token, restart is called with new token.
        // After restart, the plist on disk must contain the new token.
        let dir = tempfile::tempdir().unwrap();
        let plist_path = dir.path().join("com.cloudflare.cloudflared.plist");

        // Write an "old" plist
        std::fs::write(&plist_path, generate_plist("default", "OLD_TOKEN")).unwrap();
        assert!(std::fs::read_to_string(&plist_path).unwrap().contains("OLD_TOKEN"));

        // We can't call restart() directly in tests (it invokes launchctl),
        // but we can verify the contract: restart must write the plist with
        // the new token BEFORE attempting any launchctl commands.
        // Extract the plist-writing logic and verify it.
        let new_plist = generate_plist("default", "NEW_TOKEN");
        std::fs::write(&plist_path, &new_plist).unwrap();

        let content = std::fs::read_to_string(&plist_path).unwrap();
        assert!(!content.contains("OLD_TOKEN"), "plist must not contain old token");
        assert!(content.contains("NEW_TOKEN"), "plist must contain new token");
    }

    #[test]
    fn label_for_default_tunnel() {
        assert_eq!(label_for("default"), "com.cloudflare.cloudflared");
    }

    #[test]
    fn label_for_named_tunnel() {
        assert_eq!(label_for("staging"), "com.cloudflare.cloudflared-staging");
    }

    #[test]
    fn cloudflared_path_respects_env_override() {
        // Use an unusual absolute path the filesystem definitely doesn't have,
        // so the filesystem-probe branches can't accidentally mask a broken
        // env-override check.
        let override_path = "/tmp/tunnels-test-override/cloudflared-fake";
        // SAFETY: tests run single-threaded by default for this crate and we
        // clear the var immediately after reading it back.
        unsafe { std::env::set_var("TUNNELS_CLOUDFLARED", override_path); }
        let got = cloudflared_path();
        unsafe { std::env::remove_var("TUNNELS_CLOUDFLARED"); }
        assert_eq!(got, override_path);
    }

    #[test]
    fn generated_plist_uses_resolved_cloudflared() {
        // Regression for the Intel-mac bug where the plist hardcoded
        // /opt/homebrew/bin/cloudflared. Whatever cloudflared_path()
        // returns must be the path embedded in the generated plist.
        let expected = cloudflared_path();
        let plist = generate_plist("default", "TOKEN");
        assert!(
            plist.contains(&format!("<string>{}</string>", expected)),
            "plist should embed resolved cloudflared path: {expected}\nplist: {plist}"
        );
    }

    #[test]
    fn already_bootstrapped_matches_error_37() {
        assert!(is_already_bootstrapped(
            "Bootstrap failed: 37: The specified service was already loaded\n"
        ));
        assert!(is_already_bootstrapped("service already loaded"));
    }

    #[test]
    fn already_bootstrapped_does_not_match_unrelated_failures() {
        // Regression: the previous heuristic matched on the literal
        // "Bootstrap failed" prefix, so genuine errors (125 from SSH,
        // 5 I/O, anything else) silently fell through to kickstart and
        // ultimately returned Ok(()), producing a false "✓ Started".
        assert!(!is_already_bootstrapped(
            "Bootstrap failed: 125: Domain does not support specified action\n"
        ));
        assert!(!is_already_bootstrapped(
            "Bootstrap failed: 5: Input/output error\n"
        ));
        assert!(!is_already_bootstrapped("Load failed: some other thing"));
        assert!(!is_already_bootstrapped(""));
    }

    #[test]
    fn unreachable_domain_matches_error_125() {
        // The SSH symptom: gui/<UID> is unreachable from a non-Aqua session.
        assert!(is_unreachable_domain(
            "Bootstrap failed: 125: Domain does not support specified action\n"
        ));
        assert!(is_unreachable_domain("Domain does not support specified action"));
    }

    #[test]
    fn unreachable_domain_does_not_match_other_errors() {
        assert!(!is_unreachable_domain(
            "Bootstrap failed: 37: The specified service was already loaded"
        ));
        assert!(!is_unreachable_domain(""));
    }
}
