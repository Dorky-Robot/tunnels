use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const PLIST_DIR: &str = "/Library/LaunchDaemons";
const LOG_DIR: &str = "/Library/Logs";
const CLOUDFLARED: &str = "/opt/homebrew/bin/cloudflared";
const LABEL_PREFIX: &str = "com.cloudflare.cloudflared";

pub fn label_for(name: &str) -> String {
    if name == "default" {
        LABEL_PREFIX.to_string()
    } else {
        format!("{}-{}", LABEL_PREFIX, name)
    }
}

pub fn plist_path(name: &str) -> PathBuf {
    Path::new(PLIST_DIR).join(format!("{}.plist", label_for(name)))
}

fn generate_plist(name: &str, token: &str) -> String {
    let label = label_for(name);
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
	<string>{LOG_DIR}/{label}.out.log</string>
	<key>StandardErrorPath</key>
	<string>{LOG_DIR}/{label}.err.log</string>
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
    let output = Command::new("sudo")
        .args(["launchctl", "list", &label])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let pid = stdout
                .lines()
                .find(|l| l.contains("PID"))
                .and_then(|l| l.split_whitespace().last())
                .and_then(|s| s.parse::<u32>().ok());
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

    // Write plist via sudo tee
    let mut child = Command::new("sudo")
        .args(["tee", &path.to_string_lossy()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .context("sudo tee")?;

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        stdin.write_all(plist.as_bytes())?;
    }
    child.wait()?;

    let out = Command::new("sudo")
        .args(["launchctl", "load", &path.to_string_lossy()])
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

    let _ = Command::new("sudo")
        .args(["launchctl", "unload", &path.to_string_lossy()])
        .output();

    let _ = Command::new("sudo")
        .args(["rm", "-f", &path.to_string_lossy()])
        .output();

    Ok(())
}

pub fn restart(name: &str, token: &str) -> Result<()> {
    stop(name)?;
    start(name, token)
}

/// Read recent log lines for a tunnel
pub fn read_logs(name: &str, lines: usize) -> Result<String> {
    let label = label_for(name);
    let err_log = format!("{}/{}.err.log", LOG_DIR, label);
    let out_log = format!("{}/{}.out.log", LOG_DIR, label);

    let mut result = String::new();

    for (tag, path) in [("stderr", &err_log), ("stdout", &out_log)] {
        let output = Command::new("sudo")
            .args(["tail", &format!("-{}", lines), path])
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                if !text.trim().is_empty() {
                    result.push_str(&format!("--- {} ---\n{}\n", tag, text));
                }
            }
        }
    }

    Ok(result)
}

/// Import existing plists from /Library/LaunchDaemons
pub fn discover_existing() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let dir = Path::new(PLIST_DIR);

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
                        found.push((name, token));
                    }
                }
            }
        }
    }

    found
}
