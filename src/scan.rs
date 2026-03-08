use std::collections::HashSet;
use std::process::Command;

/// A service discovered via lsof
#[derive(Debug, Clone)]
pub struct DiscoveredService {
    pub name: String,
    pub port: u16,
}

/// System processes to ignore when scanning
const IGNORE_COMMANDS: &[&str] = &[
    "ControlCe",
    "rapportd",
    "figma_age",
    "redis-ser",
    "com.docke",
    "cloudflar",
    "stable",
    "Ollama",
    "ollama",
    "mDNSRespo",
    "launchd",
    "SystemUIS",
    "WindowSer",
    "loginwindo",
    "sharingd",
    "WiFiAgent",
    "AirPlayXPC",
    "remoted",
    "identitys",
    "tunnels",
];

/// Scan for TCP services listening on this machine using lsof.
/// Tries to resolve the project name from the process's working directory.
pub fn scan_services() -> Vec<DiscoveredService> {
    let output = Command::new("lsof")
        .args(["-iTCP", "-sTCP:LISTEN", "-nP", "-F", "pcn"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    // lsof -F pcn outputs fields like:
    // p<pid>
    // c<command>
    // n<address>:<port>
    let mut results = Vec::new();
    let mut seen_ports: HashSet<u16> = HashSet::new();
    let mut current_pid: Option<u32> = None;
    let mut current_cmd: Option<String> = None;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            current_pid = rest.parse().ok();
            current_cmd = None;
        } else if let Some(rest) = line.strip_prefix('c') {
            current_cmd = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('n') {
            let Some(pid) = current_pid else { continue };
            let Some(cmd) = &current_cmd else { continue };

            // Skip ignored system commands
            if IGNORE_COMMANDS.iter().any(|ic| cmd.starts_with(ic)) {
                continue;
            }

            // Parse port from address like "*:7070" or "127.0.0.1:3001" or "[::]:3333"
            let port = rest.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            let Some(port) = port else { continue };

            // Skip ephemeral/high ports that are likely internal
            if port > 49152 {
                continue;
            }

            // Dedup by port
            if !seen_ports.insert(port) {
                continue;
            }

            // Try to get project name from the process's working directory
            let name = project_name_from_pid(pid).unwrap_or_else(|| cmd.clone());

            results.push(DiscoveredService { name, port });
        }
    }

    results.sort_by_key(|s| s.port);
    results
}

/// Quick check: return the set of ports currently listening on this machine.
pub fn listening_ports() -> HashSet<u16> {
    scan_services().into_iter().map(|s| s.port).collect()
}

/// Use lsof to find the cwd for a PID, then extract the last path component as project name.
/// The `-a` flag is critical — it ANDs the -p and -d filters together.
fn project_name_from_pid(pid: u32) -> Option<String> {
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix('n') {
            // Skip root directory — not useful as a project name
            if path == "/" {
                return None;
            }
            if path.starts_with('/') {
                return std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string());
            }
        }
    }
    None
}
