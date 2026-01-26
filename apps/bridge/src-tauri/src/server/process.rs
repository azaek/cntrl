use crate::server::handlers::AppState;
use crate::server::types::ProcessInfo;
use std::sync::Arc;

#[cfg(target_os = "macos")]
pub fn get_processes_list(_state: &Arc<AppState>) -> Vec<ProcessInfo> {
    use std::collections::HashMap;
    use std::process::Command;

    // Execute ps command
    let output = Command::new("ps")
        .args(&["-A", "-o", "pid,rss,time,comm"])
        .output();

    // Suffixes to strip for grouping (ported from procutil_darwin.go)
    let suffixes = [
        " Helper (Renderer)",
        " Helper (GPU)",
        " Helper (Plugin)",
        " Helper (Networking)",
        " Helper",
        " Renderer",
        " GPU Process",
        " GPU",
        " Networking",
        " Plugin Host",
        " Plugin",
        " Agent",
        " Extension",
        " Web Content",
        " Utility",
        "Helper",
    ];

    if let Ok(output) = output {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            let mut agg: HashMap<String, ProcessInfo> = HashMap::new();

            // Skip header (first line)
            for line in stdout.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 4 {
                    continue;
                }

                // Parse RSS (KB)
                let memory_kb = parts[1].parse::<u64>().unwrap_or(0);
                let memory_bytes = memory_kb * 1024;

                // Parse CPU Time
                let time_str = parts[2];
                let cpu_time = parse_cpu_time(time_str);

                // Parse Command/Name
                // The command might contain spaces, so join the rest
                let raw_comm = parts[3..].join(" ");
                // Extract base name logic
                let mut name = raw_comm.split('/').last().unwrap_or(&raw_comm).to_string();

                // Remove parenthetical suffixes first like "(Renderer)"
                if let Some(idx) = name.rfind(" (") {
                    name = name[..idx].trim().to_string();
                }

                // Strip common suffixes
                for suffix in suffixes.iter() {
                    if name.ends_with(suffix) {
                        let stripped = name.trim_end_matches(suffix).trim();
                        if !stripped.is_empty() {
                            name = stripped.to_string();
                        }
                        break;
                    }
                }

                let entry = agg.entry(name.clone()).or_insert(ProcessInfo {
                    name,
                    count: 0,
                    memory: 0,
                    memory_mb: 0.0,
                    cpu_time: 0.0,
                });

                entry.count += 1;
                entry.memory += memory_bytes;
                entry.cpu_time += cpu_time;
            }

            let mut result: Vec<ProcessInfo> = agg.into_values().collect();

            // Calculate MB
            for p in &mut result {
                p.memory_mb = p.memory as f64 / (1024.0 * 1024.0);
            }

            result.sort_by(|a, b| b.memory.cmp(&a.memory));
            return result;
        }
    }

    // Fallback if ps fails (shouldn't happen on macOS)
    Vec::new()
}

#[cfg(target_os = "macos")]
fn parse_cpu_time(time_str: &str) -> f64 {
    // Format is usually MM:SS.cc or HH:MM:SS
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() == 2 {
        let min = parts[0].parse::<f64>().unwrap_or(0.0);
        let sec = parts[1].parse::<f64>().unwrap_or(0.0);
        return min * 60.0 + sec;
    } else if parts.len() == 3 {
        let hour = parts[0].parse::<f64>().unwrap_or(0.0);
        let min = parts[1].parse::<f64>().unwrap_or(0.0);
        let sec = parts[2].parse::<f64>().unwrap_or(0.0);
        return hour * 3600.0 + min * 60.0 + sec;
    }
    0.0
}

#[cfg(target_os = "macos")]
pub fn get_macos_window_map() -> std::collections::HashSet<u32> {
    use regex::Regex;
    use std::collections::HashSet;
    use std::process::Command;

    let mut pids = HashSet::new();

    // Execute lsappinfo list
    let output = Command::new("lsappinfo").arg("list").output();

    if let Ok(output) = output {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            // Output format usually "pid=1234" or "PID=1234", allowing for spaces
            // Using case-insensitive (?i) and word boundary \b
            if let Ok(re) = Regex::new(r"(?i)\bpid\s*=\s*(\d+)") {
                for cap in re.captures_iter(&stdout) {
                    if let Some(pid_match) = cap.get(1) {
                        if let Ok(pid) = pid_match.as_str().parse::<u32>() {
                            pids.insert(pid);
                        }
                    }
                }
            }
        }
    }

    pids
}

#[cfg(not(target_os = "macos"))]
pub fn get_processes_list(state: &Arc<AppState>) -> Vec<ProcessInfo> {
    let mut sys = state.system.lock().unwrap();
    sys.refresh_processes();

    use std::collections::HashMap;
    let mut agg: HashMap<String, ProcessInfo> = HashMap::new();

    for proc in sys.processes().values() {
        let name = proc.name().to_string();
        let entry = agg.entry(name.clone()).or_insert(ProcessInfo {
            name,
            count: 0,
            memory: 0,
            memory_mb: 0.0,
            cpu_time: 0.0,
        });
        entry.count += 1;
        entry.memory += proc.memory();
        // For non-macOS, we keep using cpu_usage() as "cpu_time" for now,
        // or we could try to get run time if sysinfo supports it, but sticking to existing logic is safer
        entry.cpu_time += proc.cpu_usage() as f64;
    }

    let mut result: Vec<ProcessInfo> = agg.into_values().collect();

    // Calculate MB
    for p in &mut result {
        p.memory_mb = p.memory as f64 / (1024.0 * 1024.0);
    }

    result.sort_by(|a, b| b.memory.cmp(&a.memory));
    result
}
