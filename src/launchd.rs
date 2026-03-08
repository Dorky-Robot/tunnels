use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const CLOUDFLARED: &str = "/opt/homebrew/bin/cloudflared";
const LABEL_PREFIX: &str = "com.cloudflare.cloudflared";

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
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{CLOUDFLARED}</string>
		<string>tunnel</string>
		<string>run</string>
		<string>--token</string>
		<string>{token}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
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

pub fn start(name: &str, token: &str) -> Result<()> {
    let path = plist_path(name);
    let plist = generate_plist(name, token);

    // Ensure directories exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(log_dir())?;

    // Write plist directly — no sudo needed for ~/Library/LaunchAgents
    std::fs::write(&path, plist)?;

    let out = Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .output()
        .context("launchctl load")?;

    if !out.status.success() {
        anyhow::bail!(
            "launchctl load failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn stop(name: &str) -> Result<()> {
    let path = plist_path(name);
    if !path.exists() {
        return Ok(());
    }

    let _ = Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .output();

    let _ = std::fs::remove_file(&path);

    Ok(())
}

pub fn restart(name: &str, token: &str) -> Result<()> {
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
