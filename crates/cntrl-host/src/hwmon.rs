//! Temperatures on Linux from hwmon: each chip's `name`, and its
//! `temp<N>_input` in millidegrees Celsius with an optional `temp<N>_label`
//! (kernel `hwmon/sysfs-interface`). Where no hwmon chip is the CPU's, as on
//! some ARM boards, the CPU's thermal zone stands in. Disks are NVMe only:
//! SATA drives report through `drivetemp`, which is left alone so that reading
//! it can't wake a drive that has spun down.

use std::fs;
use std::path::{Path, PathBuf};

use cntrl_protocol::stats::{SensorKind, Temperature};

use crate::stats::round;

/// Chips that measure the CPU, by the `name` their driver gives them.
const CPU_CHIPS: &[&str] = &[
    "coretemp",
    "k10temp",
    "zenpower",
    "cpu_thermal",
    "cpu-thermal",
    "soc_thermal",
    "soc-thermal",
];

/// Thermal zone types that measure the CPU.
const CPU_ZONES: &[&str] = &[
    "x86_pkg_temp",
    "cpu-thermal",
    "cpu_thermal",
    "cpu0-thermal",
    "soc-thermal",
    "soc_thermal",
];

/// The CPU's temperature first, then each NVMe drive's, under a sysfs root.
pub(crate) fn temperatures(sys: &Path) -> Vec<Temperature> {
    let mut cpu: Option<f64> = None;
    let mut disks = Vec::new();
    for chip in entries(&sys.join("class/hwmon")) {
        let name = read(&chip.join("name")).unwrap_or_default();
        let inputs = inputs(&chip);
        if CPU_CHIPS.contains(&name.as_str()) {
            if let Some(reading) = cpu_reading(&name, &inputs) {
                cpu = Some(cpu.map_or(reading, |other| other.max(reading)));
            }
        } else if name == "nvme"
            && let Some(reading) = labelled(&inputs, "Composite").or_else(|| first(&inputs))
        {
            let device = fs::read_link(chip.join("device"))
                .ok()
                .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "NVMe".to_owned());
            disks.push(reading_for(SensorKind::Disk, device, reading));
        }
    }
    if cpu.is_none() {
        cpu = entries(&sys.join("class/thermal"))
            .into_iter()
            .filter(|zone| {
                read(&zone.join("type")).is_some_and(|kind| CPU_ZONES.contains(&kind.as_str()))
            })
            .filter_map(|zone| celsius(&zone.join("temp")))
            .reduce(f64::max);
    }
    disks.sort_by(|a, b| a.label.cmp(&b.label));
    cpu.map(|reading| reading_for(SensorKind::Cpu, "CPU".to_owned(), reading))
        .into_iter()
        .chain(disks)
        .collect()
}

/// The CPU's reading from its chip: the hottest package on Intel, the die
/// (Tdie, else Tctl, which some Ryzens offset upward) on AMD, else the
/// hottest input.
fn cpu_reading(chip: &str, inputs: &[(Option<String>, f64)]) -> Option<f64> {
    match chip {
        "coretemp" => inputs
            .iter()
            .filter(|(label, _)| {
                label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Package id"))
            })
            .map(|(_, value)| *value)
            .reduce(f64::max)
            .or_else(|| hottest(inputs)),
        "k10temp" | "zenpower" => labelled(inputs, "Tdie")
            .or_else(|| labelled(inputs, "Tctl"))
            .or_else(|| hottest(inputs)),
        _ => hottest(inputs),
    }
}

fn reading_for(sensor: SensorKind, label: String, celsius: f64) -> Temperature {
    Temperature {
        sensor,
        label,
        celsius: round(celsius, 1),
    }
}

/// A chip's readings with their labels, in input order.
fn inputs(chip: &Path) -> Vec<(Option<String>, f64)> {
    let mut found: Vec<(u32, Option<String>, f64)> = entries(chip)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let index: u32 = name
                .strip_prefix("temp")?
                .strip_suffix("_input")?
                .parse()
                .ok()?;
            let value = celsius(&path)?;
            let label = read(&chip.join(format!("temp{index}_label")));
            Some((index, label, value))
        })
        .collect();
    found.sort_by_key(|(index, _, _)| *index);
    found
        .into_iter()
        .map(|(_, label, value)| (label, value))
        .collect()
}

fn labelled(inputs: &[(Option<String>, f64)], wanted: &str) -> Option<f64> {
    inputs
        .iter()
        .find(|(label, _)| label.as_deref() == Some(wanted))
        .map(|(_, value)| *value)
}

fn first(inputs: &[(Option<String>, f64)]) -> Option<f64> {
    inputs.first().map(|(_, value)| *value)
}

fn hottest(inputs: &[(Option<String>, f64)]) -> Option<f64> {
    inputs.iter().map(|(_, value)| *value).reduce(f64::max)
}

/// A millidegree reading in degrees, if it's believable: sensors that aren't
/// wired up report values like −128 or 127.
fn celsius(path: &Path) -> Option<f64> {
    let millidegrees: i64 = read(path)?.parse().ok()?;
    let degrees = millidegrees as f64 / 1000.0;
    (-40.0 < degrees && degrees < 125.0).then_some(degrees)
}

pub(crate) fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

/// A directory's entries, sorted; none if it can't be read.
pub(crate) fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(sys: &Path, dir: &str, name: &str, inputs: &[(u32, Option<&str>, &str)]) -> PathBuf {
        let path = sys.join("class/hwmon").join(dir);
        fs::create_dir_all(&path).expect("mkdir");
        fs::write(path.join("name"), format!("{name}\n")).expect("write");
        for (index, label, value) in inputs {
            fs::write(
                path.join(format!("temp{index}_input")),
                format!("{value}\n"),
            )
            .expect("write");
            if let Some(label) = label {
                fs::write(
                    path.join(format!("temp{index}_label")),
                    format!("{label}\n"),
                )
                .expect("write");
            }
        }
        path
    }

    #[test]
    fn an_intel_machine_with_an_nvme_drive() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sys = dir.path();
        chip(
            sys,
            "hwmon2",
            "coretemp",
            &[
                (1, Some("Package id 0"), "48000"),
                (2, Some("Core 0"), "51000"),
                (3, Some("Core 1"), "45000"),
            ],
        );
        let nvme = chip(
            sys,
            "hwmon1",
            "nvme",
            &[
                (1, Some("Composite"), "38900"),
                (2, Some("Sensor 1"), "41850"),
            ],
        );
        std::os::unix::fs::symlink("../../nvme0", nvme.join("device")).expect("symlink");
        chip(sys, "hwmon0", "acpitz", &[(1, None, "27800")]);
        let readings = temperatures(sys);
        assert_eq!(
            readings,
            vec![
                reading_for(SensorKind::Cpu, "CPU".to_owned(), 48.0),
                reading_for(SensorKind::Disk, "nvme0".to_owned(), 38.9),
            ]
        );
    }

    #[test]
    fn ryzen_prefers_the_die_and_skips_nonsense() {
        let dir = tempfile::tempdir().expect("temp dir");
        chip(
            dir.path(),
            "hwmon3",
            "k10temp",
            &[
                (1, Some("Tctl"), "71250"),
                (2, Some("Tdie"), "61250"),
                (3, Some("Tccd1"), "-128000"),
            ],
        );
        assert_eq!(
            temperatures(dir.path()),
            vec![reading_for(SensorKind::Cpu, "CPU".to_owned(), 61.3)]
        );
    }

    #[test]
    fn a_board_without_hwmon_uses_its_thermal_zone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let zone = dir.path().join("class/thermal/thermal_zone0");
        fs::create_dir_all(&zone).expect("mkdir");
        fs::write(zone.join("type"), "cpu-thermal\n").expect("write");
        fs::write(zone.join("temp"), "52582\n").expect("write");
        let other = dir.path().join("class/thermal/thermal_zone1");
        fs::create_dir_all(&other).expect("mkdir");
        fs::write(other.join("type"), "gpu-thermal\n").expect("write");
        fs::write(other.join("temp"), "60000\n").expect("write");
        assert_eq!(
            temperatures(dir.path()),
            vec![reading_for(SensorKind::Cpu, "CPU".to_owned(), 52.6)]
        );
    }

    #[test]
    fn nothing_to_read_reads_nothing() {
        assert_eq!(temperatures(Path::new("/nonexistent")), Vec::new());
    }
}
