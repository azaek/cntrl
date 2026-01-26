#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn get_cpu_speed_ghz() -> f64 {
    #[cfg(target_os = "windows")]
    {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(key) = hklm.open_subkey("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0") {
            if let Ok(mhz) = key.get_value::<u32, _>("~MHz") {
                return mhz as f64 / 1000.0;
            }
        }
    }

    #[cfg(any(target_os = "macos", unix))]
    {
        // Try hw.cpufrequency
        let output = Command::new("sysctl")
            .args(["-n", "hw.cpufrequency"])
            .output()
            .ok();

        if let Some(out) = output {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(hz) = s.parse::<u64>() {
                if hz > 0 {
                    return hz as f64 / 1_000_000_000.0;
                }
            }
        }

        // Fallback for Apple Silicon (sysctl often returns 0 or fails for cpufrequency)
        let brand_out = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok();

        if let Some(out) = brand_out {
            let brand = String::from_utf8_lossy(&out.stdout).to_lowercase();
            if brand.contains("m1") {
                return 3.2;
            }
            if brand.contains("m2") {
                return 3.5;
            }
            if brand.contains("m3") {
                return 4.0;
            }
            if brand.contains("m4") {
                return 4.4;
            }
        }
    }

    0.0
}

pub fn get_memory_slots() -> usize {
    #[cfg(target_os = "windows")]
    {
        // wmic memorychip get Capacity
        let mut cmd = Command::new("wmic");
        cmd.args(&["memorychip", "get", "Capacity"]);

        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd.output().ok();

        if let Some(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = stdout.trim().lines().collect();

            let mut count = 0;
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    continue;
                } // Skip header
                if !line.trim().is_empty() {
                    count += 1;
                }
            }
            if count > 0 {
                return count;
            }
        }
    }

    #[cfg(any(target_os = "macos", unix))]
    {
        return 1;
    }

    0
}
