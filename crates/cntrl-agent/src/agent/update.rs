//! `cntrl update` (research angle 05, D41): updates this agent in place to the
//! release Console names, after checking that release's signed manifest with
//! the release key built into the agent, so Console can only choose among
//! genuine releases and never an older one. It downloads this machine's
//! archive, checks its size and SHA-256 against the manifest, and runs the
//! installer, which keeps the enrollment: the one in the archive when it
//! carries one (from 0.1.8), else the copy built into this binary.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::client::block_on;
use super::config::Config;
use super::os;
use super::say::{say, say_err};

/// The public half of the release key (D21), which signs every release's
/// manifest.
const RELEASE_KEY: &str = include_str!("../../../../packaging/release-key.pub.pem");
/// The installer as this agent was built, for an archive without one.
const INSTALLER: &str = include_str!("../../../../packaging/install.sh");
/// Where releases are published.
const RELEASES: &str = "https://github.com/azaek/cntrl/releases/download";
/// The largest archive the update downloads; releases are about 5 MB.
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
/// How long connecting may take, which reqwest's connector splits across a
/// host's addresses. GitHub's download host has four, and a network that
/// drops one (as the owner's did on 2026-10-05) otherwise holds each request
/// for the system's own connect timeout, 75 s on macOS, before the next.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// What a release's signed manifest says.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    artifacts: BTreeMap<String, Artifact>,
}

/// One target's archive.
#[derive(Debug, Deserialize)]
struct Artifact {
    url: String,
    size: u64,
    sha256: String,
}

/// Runs `cntrl update`: with `check`, only says whether a newer release is
/// out; with `force`, reinstalls the release this agent already is.
pub fn run(config: &Config, check: bool, force: bool) -> ExitCode {
    let current = env!("CARGO_PKG_VERSION");
    if !check && !os::is_root() {
        say_err!(
            "cntrl update: updating replaces the agent's files; run {}",
            os::elevated("cntrl update")
        );
        return ExitCode::FAILURE;
    }
    match block_on(update(config, current, check, force)) {
        Ok(code) => code,
        Err(e) => {
            say_err!("cntrl update: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn update(
    config: &Config,
    current: &str,
    check: bool,
    force: bool,
) -> Result<ExitCode, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("cntrl-agent/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let latest = latest_version(&client, &config.console.url).await?;
    let reinstall = force && latest == current;
    if !newer(&latest, current) && !reinstall {
        if newer(current, &latest) {
            say!("cntrl agent {current} is newer than the latest release, {latest}.");
        } else {
            say!("cntrl agent {current} is the latest release.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    if check {
        say!(
            "cntrl agent {latest} is out; this one is {current}. Update with {}.",
            os::elevated("cntrl update")
        );
        return Ok(ExitCode::SUCCESS);
    }

    // Say what's happening before each wait on the network, never after.
    if reinstall {
        say!("Installing cntrl agent {latest} again. Checking the release's signature…");
    } else {
        say!(
            "cntrl agent {latest} is out; this one is {current}. Checking the release's signature…"
        );
    }
    let target = release_target()?;
    let base = format!("{RELEASES}/agent-v{latest}");
    let manifest = fetch(&client, &format!("{base}/manifest.json"), MAX_ARCHIVE, None).await?;
    let signature = fetch(&client, &format!("{base}/manifest.json.sig"), 1024, None).await?;
    verify(&manifest, &signature)?;
    let manifest: Manifest = serde_json::from_slice(&manifest)
        .map_err(|e| format!("the release's manifest doesn't read: {e}"))?;
    if manifest.version != latest {
        return Err(format!(
            "the manifest for {latest} says it's {}",
            manifest.version
        ));
    }
    let artifact = manifest
        .artifacts
        .get(&target)
        .ok_or_else(|| format!("release {latest} has no build for {target}"))?;
    say!(
        "The signature checks. Downloading the build for {target} ({:.1} MB)…",
        megabytes(artifact.size)
    );
    let archive = fetch(
        &client,
        &artifact.url,
        MAX_ARCHIVE.min(artifact.size),
        Progress::new(artifact.size),
    )
    .await?;
    matches(&archive, artifact)?;

    // A directory only root can reach, so nothing changes the files between
    // checking them and running them.
    let work = tempfile::Builder::new()
        .prefix("cntrl-update-")
        .tempdir()
        .map_err(|e| format!("can't make a working directory: {e}"))?;
    let archive_path = work.path().join("agent.tar.gz");
    std::fs::write(&archive_path, &archive).map_err(|e| format!("can't save the download: {e}"))?;
    let installer = installer(work.path(), &archive_path)?;
    let mut install = Command::new("/bin/sh");
    install.arg(&installer).arg("--archive").arg(&archive_path);
    if reinstall {
        install.arg("--force");
    }
    let status = install
        .status()
        .map_err(|e| format!("can't run the installer: {e}"))?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The release Console offers: the version its install script names, so
/// Console's imports and yanks decide, as they do for the install command.
async fn latest_version(client: &reqwest::Client, console: &str) -> Result<String, String> {
    let url = format!("{}/install.sh", console.trim_end_matches('/'));
    let script = fetch(client, &url, 1024 * 1024, None).await?;
    installer_version(&String::from_utf8_lossy(&script))
        .ok_or_else(|| format!("{url} doesn't say which release is current"))
}

/// The `CNTRL_VERSION` Console filled in at the top of its install script.
fn installer_version(script: &str) -> Option<String> {
    script.lines().find_map(|line| {
        let value = line
            .strip_prefix("CNTRL_VERSION=")?
            .trim()
            .trim_matches('\'');
        (!value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)))
        .then(|| value.to_owned())
    })
}

/// Checks the manifest's signature with the release key.
fn verify(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    let signature = STANDARD
        .decode(String::from_utf8_lossy(signature).trim())
        .map_err(|_| "the release's signature isn't base64".to_owned())?;
    UnparsedPublicKey::new(&ED25519, release_key()?)
        .verify(manifest, &signature)
        .map_err(|_| "the release's manifest isn't signed by cntrl's release key".to_owned())
}

/// The release key's 32 bytes, from its PEM: an Ed25519 key's DER is a fixed
/// 12-byte prefix and the key.
fn release_key() -> Result<[u8; 32], String> {
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let body: String = RELEASE_KEY
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = STANDARD
        .decode(body.trim())
        .map_err(|_| "the built-in release key isn't base64".to_owned())?;
    match der.split_at_checked(PREFIX.len()) {
        Some((prefix, key)) if prefix == PREFIX => key
            .try_into()
            .map_err(|_| "the built-in release key isn't 32 bytes".to_owned()),
        _ => Err("the built-in release key isn't an Ed25519 key".to_owned()),
    }
}

/// Whether the download is what the manifest describes.
fn matches(archive: &[u8], artifact: &Artifact) -> Result<(), String> {
    if archive.len() as u64 != artifact.size {
        return Err(format!(
            "the download is {} bytes; the release says {}",
            archive.len(),
            artifact.size
        ));
    }
    let digest = Sha256::digest(archive);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if !hex.eq_ignore_ascii_case(&artifact.sha256) {
        return Err("the download's SHA-256 doesn't match the release's".to_owned());
    }
    Ok(())
}

/// The installer to run: the archive's own, from 0.1.8, which knows that
/// release's steps; else the one built into this agent.
fn installer(work: &Path, archive: &Path) -> Result<std::path::PathBuf, String> {
    let unpacked = work.join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&unpacked)
        .arg("--strip-components")
        .arg("1")
        .status()
        .map_err(|e| format!("can't run tar: {e}"))?;
    if !status.success() {
        return Err("the archive doesn't unpack".to_owned());
    }
    let own = unpacked.join("packaging").join("install.sh");
    if own.is_file() {
        return Ok(own);
    }
    let built_in = work.join("install.sh");
    std::fs::write(&built_in, INSTALLER).map_err(|e| e.to_string())?;
    Ok(built_in)
}

/// Downloads `url`, refusing more than `limit` bytes, and shows `progress`
/// when there is one.
async fn fetch(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
    mut progress: Option<Progress>,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("can't reach {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("lost {url}: {e}"))?
    {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > limit {
            return Err(format!("{url} sent more than expected"));
        }
        if let Some(progress) = &mut progress {
            progress.show(body.len() as u64);
        }
    }
    Ok(body)
}

/// A download's progress on a terminal, one line redrawn the way the
/// installer draws its own: a bar, the share done, and how much of how much.
struct Progress {
    total: u64,
    glyphs: (char, char),
    drawn: Option<Instant>,
}

impl Progress {
    /// Progress toward `total` bytes, when stderr is a terminal to draw on.
    fn new(total: u64) -> Option<Self> {
        std::io::stderr().is_terminal().then(|| Self {
            total,
            glyphs: if utf8_locale() {
                ('█', '░')
            } else {
                ('#', '.')
            },
            drawn: None,
        })
    }

    /// Draws `have` bytes done: at most ten times a second, and always the
    /// last.
    fn show(&mut self, have: u64) {
        let now = Instant::now();
        if have < self.total
            && self
                .drawn
                .is_some_and(|at| now.duration_since(at) < Duration::from_millis(100))
        {
            return;
        }
        self.drawn = Some(now);
        let line = format!("\r  {}\x1b[K", progress_line(have, self.total, self.glyphs));
        let _ = std::io::stderr().write_all(line.as_bytes());
    }
}

impl Drop for Progress {
    /// Ends the line however the download ends, so what follows starts on
    /// its own.
    fn drop(&mut self) {
        if self.drawn.is_some() {
            say_err!();
        }
    }
}

/// Whether the locale is UTF-8, the installer's test, for the bar's blocks.
fn utf8_locale() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .is_some_and(|locale| {
            let locale = locale.to_ascii_lowercase();
            locale.contains("utf-8") || locale.contains("utf8")
        })
}

/// The progress line for `have` of `total` bytes.
fn progress_line(have: u64, total: u64, (full, empty): (char, char)) -> String {
    const WIDTH: usize = 28;
    let share = if total == 0 {
        0.0
    } else {
        (have as f64 / total as f64).min(1.0)
    };
    let filled = (share * WIDTH as f64).round() as usize;
    let bar: String = (0..WIDTH)
        .map(|at| if at < filled { full } else { empty })
        .collect();
    format!(
        "{bar} {:>3}%  {:.1} of {:.1} MB",
        (share * 100.0) as u32,
        megabytes(have),
        megabytes(total)
    )
}

/// The release build for this machine: static musl on Linux, whatever this
/// binary was built against, as the installer picks.
fn release_target() -> Result<String, String> {
    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        other => return Err(format!("there are no releases for {other}")),
    };
    match std::env::consts::OS {
        "linux" => Ok(format!("{arch}-unknown-linux-musl")),
        "macos" => Ok(format!("{arch}-apple-darwin")),
        other => Err(format!("there are no releases for {other}")),
    }
}

/// Whether version `a` is newer than `b`: `1.2.10` over `1.2.9`, and a
/// release over its own prereleases.
fn newer(a: &str, b: &str) -> bool {
    fn parts(version: &str) -> (Vec<u64>, Option<&str>) {
        let (core, pre) = match version.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (version.split('+').next().unwrap_or(version), None),
        };
        (
            core.split('.').map(|n| n.parse().unwrap_or(0)).collect(),
            pre,
        )
    }
    let (a_core, a_pre) = parts(a);
    let (b_core, b_pre) = parts(b);
    match a_core.cmp(&b_core) {
        std::cmp::Ordering::Equal => match (a_pre, b_pre) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => x > y,
            _ => false,
        },
        order => order.is_gt(),
    }
}

/// Bytes as megabytes, for saying how big a download is, counted as the
/// installer and GitHub's release page count them.
#[allow(clippy::cast_precision_loss)]
fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] =
        include_bytes!("../../../../testdata/release/agent-v0.1.7/manifest.json");
    const SIGNATURE: &[u8] =
        include_bytes!("../../../../testdata/release/agent-v0.1.7/manifest.json.sig");

    #[test]
    fn the_built_in_key_checks_a_real_release_and_nothing_else() {
        verify(MANIFEST, SIGNATURE).expect("0.1.7's manifest checks");
        let mut changed = MANIFEST.to_vec();
        let at = changed.iter().position(|byte| *byte == b'7').expect("a 7");
        changed[at] = b'8';
        assert!(verify(&changed, SIGNATURE).is_err());
        assert!(verify(MANIFEST, b"not base64!").is_err());
    }

    #[test]
    fn reads_the_release_console_names() {
        let script = "#!/bin/sh\nCNTRL_CONSOLE='https://gw.cntrl.pw'\nCNTRL_VERSION='0.1.7'\nCNTRL_ARTIFACTS='...'\n";
        assert_eq!(installer_version(script).as_deref(), Some("0.1.7"));
        assert_eq!(installer_version("CNTRL_VERSION=''\n"), None);
        assert_eq!(installer_version("CNTRL_VERSION='$(reboot)'\n"), None);
        assert_eq!(installer_version("#!/bin/sh\n"), None);
    }

    #[test]
    fn a_download_must_be_what_the_manifest_says() {
        let manifest: Manifest = serde_json::from_slice(MANIFEST).expect("a manifest");
        let artifact = manifest.artifacts.values().next().expect("a build");
        assert!(matches(b"not the archive", artifact).is_err());
        let archive = b"archive";
        let fitting = Artifact {
            url: String::new(),
            size: archive.len() as u64,
            sha256: "e4e4a4d9e1ea8fdc16d4b1dd2ff2bdbc2a0e8d5c6bc5a9d0d9bea2cc8f0c3a59".to_owned(),
        };
        // The right size with the wrong hash still fails.
        assert!(matches(archive, &fitting).is_err());
        let hex: String = Sha256::digest(archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(
            matches(
                archive,
                &Artifact {
                    sha256: hex,
                    ..fitting
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn versions_compare_as_releases_do() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(newer("0.1.8", "0.1.8-beta.1"));
        assert!(!newer("0.1.7", "0.1.7"));
        assert!(!newer("0.1.6", "0.1.7"));
    }

    #[test]
    fn progress_reads_as_the_installers_does() {
        let ascii = ('#', '.');
        assert_eq!(
            progress_line(0, 4_086_046, ascii),
            "............................   0%  0.0 of 3.9 MB"
        );
        assert_eq!(
            progress_line(2_043_023, 4_086_046, ascii),
            "##############..............  50%  1.9 of 3.9 MB"
        );
        assert_eq!(
            progress_line(4_086_046, 4_086_046, ('█', '░')),
            "████████████████████████████ 100%  3.9 of 3.9 MB"
        );
        // More than promised stays full, and an unknown total stays empty.
        assert!(progress_line(5_000_000, 4_086_046, ascii).contains(" 100%  4.8 of 3.9 MB"));
        assert!(progress_line(10, 0, ascii).starts_with("............................   0%"));
    }

    #[test]
    fn linux_gets_the_static_build() {
        let target = release_target().expect("a target");
        if cfg!(target_os = "linux") {
            assert!(target.ends_with("-unknown-linux-musl"));
        } else {
            assert!(target.ends_with("-apple-darwin"));
        }
    }
}
