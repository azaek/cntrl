#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Instant;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Debug)]
pub struct GpuData {
    pub vendor: String,
    pub model: String,
    pub vram_total_mb: u64, // MB
    pub vram_used_mb: i64,  // MB (-1 for unknown)
    pub temp_c: f64,        // (-1.0 for unknown)
    pub load_percent: f64,  // (-1.0 for unknown)
    pub last_updated: Instant,
}

pub fn get_gpu_stats() -> Option<GpuData> {
    // Try NVIDIA
    if let Some(stats) = get_nvidia_stats() {
        return Some(stats);
    }

    #[cfg(any(target_os = "macos", unix))]
    {
        if let Some(stats) = get_macos_gpu_stats() {
            return Some(stats);
        }
    }

    None
}

#[cfg(any(target_os = "macos", unix))]
fn get_macos_gpu_stats() -> Option<GpuData> {
    use serde_json::Value;

    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;

    let json: Value = serde_json::from_slice(&output.stdout).ok()?;
    let display_data = json.get("SPDisplaysDataType")?.as_array()?;

    for gpu in display_data {
        let mut name = gpu
            .get("_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown GPU")
            .to_string();

        // CLEANUP BRAND: "kHW_AppleM1Item" -> "Apple M1"
        if name.starts_with("kHW_") && name.ends_with("Item") {
            name = name
                .trim_start_matches("kHW_")
                .trim_end_matches("Item")
                .to_string();
            // Handle "AppleM1" -> "Apple M1"
            if name.starts_with("AppleM") {
                name = name.replace("AppleM", "Apple M");
            }
        }

        // Skip items that are clearly not GPUs if possible
        if name.contains("Display Connector") {
            continue;
        }

        let mut vendor = gpu
            .get("spdisplays_vendor")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        // CLEANUP VENDOR: "sppci_vendor_Apple" -> "Apple"
        if vendor.to_lowercase().contains("apple") {
            vendor = "Apple".to_string();
        } else if vendor.to_lowercase().contains("intel") {
            vendor = "Intel".to_string();
        } else if vendor.to_lowercase().contains("amd") {
            vendor = "AMD".to_string();
        }

        let mut vram_mb = 0;
        let vram_keys = [
            "spdisplays_vram",
            "spdisplays_vram_shared",
            "spdisplays_vram_dynamic",
        ];
        for key in vram_keys {
            if let Some(v) = gpu.get(key).and_then(|v| v.as_str()) {
                // Parse "8 GB" or "1536 MB"
                let v_up = v.to_uppercase();
                if v_up.contains("GB") {
                    if let Ok(gb) = v_up.split("GB").next().unwrap_or("0").trim().parse::<u64>() {
                        vram_mb = gb * 1024;
                        break;
                    }
                } else if v_up.contains("MB") {
                    if let Ok(mb) = v_up.split("MB").next().unwrap_or("0").trim().parse::<u64>() {
                        vram_mb = mb;
                        break;
                    }
                }
            }
        }

        // REAL-TIME USAGE (Apple Silicon / Intel Integrated)
        let mut load_percent = -1.0;
        let mut vram_used_mb = -1;
        let temp_c = -1.0;

        let ioreg_output = Command::new("ioreg")
            .args(["-rw0", "-c", "IOAccelerator"])
            .output()
            .ok();

        if let Some(out) = ioreg_output {
            let s = String::from_utf8_lossy(&out.stdout);
            // Search for "Device Utilization" = 5
            if let Some(idx) = s.find("\"Device Utilization\"=") {
                let start = idx + "\"Device Utilization\"=".len();
                let end = s[start..]
                    .find(',')
                    .or_else(|| s[start..].find('}'))
                    .unwrap_or(0);
                if end > 0 {
                    if let Ok(util) = s[start..start + end].trim().parse::<f64>() {
                        load_percent = util;
                    }
                }
            }
            // Search for memory usage (varies by chip)
            // Some newer macs show "In use system memory"
            if let Some(idx) = s.find("\"In use system memory\"=") {
                let start = idx + "\"In use system memory\"=".len();
                let end = s[start..]
                    .find(',')
                    .or_else(|| s[start..].find('}'))
                    .unwrap_or(0);
                if end > 0 {
                    if let Ok(bytes) = s[start..start + end].trim().parse::<u64>() {
                        vram_used_mb = (bytes / 1024 / 1024) as i64;
                    }
                }
            }
        }

        return Some(GpuData {
            vendor,
            model: name,
            vram_total_mb: vram_mb,
            vram_used_mb,
            temp_c,
            load_percent,
            last_updated: Instant::now(),
        });
    }

    None
}

fn get_nvidia_stats() -> Option<GpuData> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(&[
        "--query-gpu=utilization.gpu,utilization.memory,temperature.gpu,name,memory.total,memory.used",
        "--format=csv,noheader,nounits"
    ]);

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim().lines().next()?;
    let parts: Vec<&str> = line.split(", ").collect();

    if parts.len() < 6 {
        return None;
    }

    let util_gpu: f64 = parts[0].parse().unwrap_or(-1.0);
    let temp: f64 = parts[2].parse().unwrap_or(-1.0);
    let name = parts[3].to_string();
    let total_mem: u64 = parts[4].parse().unwrap_or(0);
    let used_mem: i64 = parts[5].parse().unwrap_or(0) as i64;

    Some(GpuData {
        vendor: "NVIDIA".to_string(),
        model: name,
        vram_total_mb: total_mem,
        vram_used_mb: used_mem,
        temp_c: temp,
        load_percent: util_gpu,
        last_updated: Instant::now(),
    })
}
