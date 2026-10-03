//! GPUs (angle 09 §5). AMD through amdgpu's sysfs files; NVIDIA through
//! `nvidia-smi` in loop mode, since a static musl binary can't load NVML
//! (angle 06 §1.5); Macs through IOKit's `IOAccelerator` statistics, as
//! `ioreg` prints them. Parsing is plain Rust, so it builds and its tests run on
//! any OS.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cntrl_protocol::stats::GpuStats;

use crate::hwmon::{entries, read};
use crate::stats::round;

/// AMD GPUs from `class/drm/card*/device` under a sysfs root: amdgpu's busy
/// share and VRAM, and its hwmon's temperature and power. A GPU the kernel has
/// powered down shows as idle without its other files being read, since
/// reading them would wake it.
pub(crate) fn amd(sys: &Path) -> Vec<GpuStats> {
    entries(&sys.join("class/drm"))
        .into_iter()
        .filter(|card| {
            card.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card") && !n.contains('-'))
        })
        .map(|card| card.join("device"))
        .filter(|device| device.join("gpu_busy_percent").exists())
        .map(|device| {
            let name = read(&device.join("product_name"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "AMD GPU".to_owned());
            if read(&device.join("power/runtime_status")).as_deref() == Some("suspended") {
                return GpuStats {
                    name,
                    busy: Some(0.0),
                    ..GpuStats::default()
                };
            }
            let number = |file: &str| read(&device.join(file))?.parse::<u64>().ok();
            let hwmon = entries(&device.join("hwmon")).into_iter().next();
            let sensor = |file: &str| {
                let hwmon = hwmon.as_ref()?;
                read(&hwmon.join(file))?.parse::<f64>().ok()
            };
            GpuStats {
                name,
                busy: number("gpu_busy_percent").map(|p| round(p as f64 / 100.0, 4)),
                memory_used: number("mem_info_vram_used"),
                memory_total: number("mem_info_vram_total"),
                temperature: sensor("temp1_input").map(|m| round(m / 1000.0, 1)),
                // Microwatts; newer kernels name the average power1_input.
                power: sensor("power1_average")
                    .or_else(|| sensor("power1_input"))
                    .map(|uw| round(uw / 1_000_000.0, 1)),
            }
        })
        .collect()
}

/// What `nvidia-smi` is asked for, in this order.
const QUERY: &str =
    "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw";
/// How often it prints.
const LOOP_MS: &str = "2000";
/// Stopped once nobody has asked for this long.
const IDLE: Duration = Duration::from_secs(10);
/// After a start that printed nothing, how long until another try.
const RETRY: Duration = Duration::from_secs(300);

/// NVIDIA GPUs through one `nvidia-smi` child in loop mode, run while someone
/// asks and stopped soon after they stop. It can keep the GPU out of its
/// power-saving state, but only while someone watches (Beszel's GPU guide).
#[derive(Debug, Default)]
pub(crate) struct NvidiaSmi {
    shared: Arc<Mutex<NvidiaState>>,
}

#[derive(Debug, Default)]
struct NvidiaState {
    gpus: BTreeMap<u32, GpuStats>,
    asked: Option<Instant>,
    running: bool,
    retry_at: Option<Instant>,
}

impl NvidiaSmi {
    /// The GPUs as of `nvidia-smi`'s last line, starting it if it isn't running.
    pub(crate) fn get(&self) -> Vec<GpuStats> {
        let mut state = lock(&self.shared);
        state.asked = Some(Instant::now());
        if !state.running && state.retry_at.is_none_or(|at| Instant::now() >= at) {
            let shared = Arc::clone(&self.shared);
            state.running = thread::Builder::new()
                .name("nvidia-smi".to_owned())
                .spawn(move || watch(&shared))
                .is_ok();
        }
        state.gpus.values().cloned().collect()
    }
}

fn watch(shared: &Arc<Mutex<NvidiaState>>) {
    let child = Command::new("nvidia-smi")
        .args([QUERY, "--format=csv,noheader,nounits", "-lms", LOOP_MS])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut printed = false;
    if let Ok(mut child) = child {
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Some((index, gpu)) = nvidia_line(&line) else {
                    continue;
                };
                printed = true;
                let mut state = lock(shared);
                state.gpus.insert(index, gpu);
                if state.asked.is_none_or(|at| at.elapsed() > IDLE) {
                    break;
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut state = lock(shared);
    state.running = false;
    state.gpus.clear();
    // Nothing printed: `nvidia-smi` is missing or can't reach the devices.
    // Console just shows no GPU, and the next try waits.
    if !printed {
        state.retry_at = Some(Instant::now() + RETRY);
    }
}

/// One `nvidia-smi` line: `0, NVIDIA GeForce RTX 4090, 37, 1523, 24564, 45,
/// 61.23`. Memory comes in MiB; fields a GPU doesn't support read `[N/A]`.
pub(crate) fn nvidia_line(line: &str) -> Option<(u32, GpuStats)> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [index, name, busy, used, total, temperature, power] = fields.as_slice() else {
        return None;
    };
    let number = |text: &str| text.parse::<f64>().ok().filter(|n| n.is_finite());
    let mib = |text: &str| number(text).map(|n| (n * 1024.0 * 1024.0) as u64);
    Some((
        index.parse().ok()?,
        GpuStats {
            name: (*name).to_owned(),
            busy: number(busy).map(|p| round(p / 100.0, 4)),
            memory_used: mib(used),
            memory_total: mib(total),
            temperature: number(temperature),
            power: number(power).map(|w| round(w, 1)),
        },
    ))
}

/// GPUs from `ioreg -r -c IOAccelerator -d 1 -w0`: each accelerator's
/// `PerformanceStatistics` and `model`. Apple silicon reports `Device
/// Utilization %` and `In use system memory`; AMD GPUs in Intel Macs report
/// `GPU Activity(%)` and `Temperature(C)` (Stats.app's GPU reader).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac(listing: &str) -> Vec<GpuStats> {
    listing
        .split("+-o ")
        .skip(1)
        .filter_map(|entry| {
            let stats = entry
                .lines()
                .find_map(|line| line.trim().strip_prefix("\"PerformanceStatistics\" = {"))?;
            let value = |key: &str| -> Option<f64> {
                stats.split(',').find_map(|pair| {
                    let (k, v) = pair.split_once('=')?;
                    (k.trim().trim_matches('"') == key)
                        .then(|| v.trim().trim_end_matches('}').parse::<f64>().ok())
                        .flatten()
                })
            };
            let quoted = |key: &str| {
                entry.lines().find_map(|line| {
                    let value = line.trim().strip_prefix(&format!("\"{key}\" = "))?;
                    Some(value.trim_matches(['"', '<', '>']).to_owned())
                })
            };
            let class = quoted("IOClass").unwrap_or_default();
            let name = quoted("model")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    if class.contains("AMD") {
                        "AMD GPU".to_owned()
                    } else if class.contains("Intel") {
                        "Intel GPU".to_owned()
                    } else {
                        "GPU".to_owned()
                    }
                });
            Some(GpuStats {
                name,
                busy: value("Device Utilization %")
                    .or_else(|| value("GPU Activity(%)"))
                    .map(|p| round(p / 100.0, 4)),
                memory_used: value("In use system memory").map(|b| b as u64),
                memory_total: None,
                temperature: value("Temperature(C)").filter(|t| *t > 0.0),
                power: None,
            })
        })
        .collect()
}

/// Whether the NVIDIA driver is loaded, under a procfs root.
pub(crate) fn nvidia_driver(proc: &Path) -> bool {
    proc.join("driver/nvidia/version").exists()
}

fn lock(shared: &Mutex<NvidiaState>) -> MutexGuard<'_, NvidiaState> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn reads_nvidia_smi_lines() {
        let (index, gpu) =
            nvidia_line("1, NVIDIA GeForce RTX 4090, 37, 1523, 24564, 45, 61.23").expect("a line");
        assert_eq!(index, 1);
        assert_eq!(
            gpu,
            GpuStats {
                name: "NVIDIA GeForce RTX 4090".to_owned(),
                busy: Some(0.37),
                memory_used: Some(1523 * 1024 * 1024),
                memory_total: Some(24564 * 1024 * 1024),
                temperature: Some(45.0),
                power: Some(61.2),
            }
        );
        let (_, older) = nvidia_line("0, Tesla K80, 0, 0, 11441, 31, [N/A]").expect("a line");
        assert_eq!(older.power, None);
        assert_eq!(
            nvidia_line("NVIDIA-SMI has failed because it couldn't communicate"),
            None
        );
    }

    #[test]
    fn reads_amdgpu_sysfs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sys = dir.path();
        let write = |path: &str, text: &str| {
            let path = sys.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        };
        write("class/drm/card1/device/gpu_busy_percent", "23\n");
        write("class/drm/card1/device/mem_info_vram_used", "1073741824\n");
        write(
            "class/drm/card1/device/mem_info_vram_total",
            "17163091968\n",
        );
        write("class/drm/card1/device/product_name", "\n");
        write("class/drm/card1/device/power/runtime_status", "active\n");
        write("class/drm/card1/device/hwmon/hwmon4/temp1_input", "52000\n");
        write(
            "class/drm/card1/device/hwmon/hwmon4/power1_average",
            "35000000\n",
        );
        // A connector, and a sleeping second GPU whose files mustn't be read.
        write("class/drm/card1-DP-1/status", "connected\n");
        write("class/drm/card2/device/gpu_busy_percent", "99\n");
        write("class/drm/card2/device/power/runtime_status", "suspended\n");
        let gpus = amd(sys);
        assert_eq!(gpus.len(), 2);
        assert_eq!(
            gpus[0],
            GpuStats {
                name: "AMD GPU".to_owned(),
                busy: Some(0.23),
                memory_used: Some(1_073_741_824),
                memory_total: Some(17_163_091_968),
                temperature: Some(52.0),
                power: Some(35.0),
            }
        );
        assert_eq!((gpus[1].busy, gpus[1].memory_used), (Some(0.0), None));
        assert_eq!(amd(Path::new("/nonexistent")), Vec::new());
    }

    #[test]
    fn reads_ioreg_on_apple_silicon_and_intel() {
        let apple = r#"+-o AGXAcceleratorG16G  <class AGXAcceleratorG16G, id 0x100000460>
    {
      "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=1047658496,"Tiler Utilization %"=7,"Renderer Utilization %"=6,"Device Utilization %"=7,"In use system memory"=330776576}
      "model" = "Apple M4"
      "IOClass" = "AGXAcceleratorG16G"
    }
"#;
        assert_eq!(
            mac(apple),
            vec![GpuStats {
                name: "Apple M4".to_owned(),
                busy: Some(0.07),
                memory_used: Some(330_776_576),
                ..GpuStats::default()
            }]
        );
        let intel_mac = r#"+-o AMDRadeonX6000_AMDNavi14GraphicsAccelerator  <class AMDRadeonX6000_AMDNavi14GraphicsAccelerator>
    {
      "IOClass" = "AMDRadeonX6000_AMDNavi14GraphicsAccelerator"
      "PerformanceStatistics" = {"GPU Activity(%)"=41,"Temperature(C)"=61,"Fan Speed(%)"=30}
    }
+-o IntelAccelerator  <class IntelAccelerator>
    {
      "IOClass" = "IntelAccelerator"
      "PerformanceStatistics" = {"Device Utilization %"=4}
    }
"#;
        let gpus = mac(intel_mac);
        assert_eq!(
            gpus.iter()
                .map(|g| (g.name.as_str(), g.busy, g.temperature))
                .collect::<Vec<_>>(),
            [
                ("AMD GPU", Some(0.41), Some(61.0)),
                ("Intel GPU", Some(0.04), None)
            ]
        );
    }
}
