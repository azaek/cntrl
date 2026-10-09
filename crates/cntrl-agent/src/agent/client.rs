//! The CLI commands that talk to the running agent or read its files:
//! `cntrl status`, `cntrl enroll`, `cntrl policy show|check|allow|deny|modify`,
//! `cntrl audit verify` and `cntrl history show|keep|clear`.

use std::fs;
use std::future::Future;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use cntrl_protocol::enroll::{EnrollError, EnrollErrorCode, EnrollToken};
use cntrl_protocol::history::HistoryStore;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode, header};
use hyper_util::rt::TokioIo;

use super::audit;
use super::config::Config;
use super::enroll::{EnrollCommand, EnrollOutcome};
use super::history::{self, HISTORY_DIR};
use super::local_api::{
    HistoryClearCommand, HistoryKeepCommand, PauseCommand, PauseOutcome, ResumeOutcome, Status,
    UninstallCommand, UninstallOutcome,
};
use super::os;
use super::policy::{self, Policy, PolicyState, Source};
use super::say::{say, say_err};
use super::signers::{self, Pin, Trust};
use super::uplink::{UplinkStatus, now_ms};

pub fn print_status(config: &Config, json: bool) -> ExitCode {
    let status = match block_on(get_status(&config.paths.agent_socket)) {
        Ok(status) => status,
        Err(e) => return fail(&e),
    };
    if json {
        return match serde_json::to_string_pretty(&status) {
            Ok(text) => {
                say!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e.to_string()),
        };
    }
    say!(
        "cntrl-agent {}, up {} (pid {})",
        status.version,
        uptime(status.uptime_s),
        status.pid
    );
    match &status.uplink {
        UplinkStatus::NotEnrolled => say!("uplink: not enrolled"),
        UplinkStatus::Connecting { gateway, attempt } => {
            say!("uplink: connecting to {gateway} (attempt {})", attempt + 1);
        }
        UplinkStatus::Online {
            gateway,
            session,
            since_ms,
        } => {
            let up = now_ms().saturating_sub(*since_ms) / 1000;
            say!(
                "uplink: online at {gateway} for {} (session {session})",
                uptime(up)
            );
        }
        UplinkStatus::Retrying {
            reason,
            retry_at_ms,
        } => {
            let wait = retry_at_ms.saturating_sub(now_ms()).div_ceil(1000);
            say!("uplink: retrying in {wait}s; {reason}");
        }
        UplinkStatus::Stopped { reason } => say!("uplink: stopped; {reason}"),
        UplinkStatus::Paused {
            by,
            reason,
            since_ms,
        } => {
            let ago = now_ms().saturating_sub(*since_ms) / 1000;
            let why = reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default();
            say!(
                "uplink: paused by {by} {} ago{why}; {} reconnects",
                uptime(ago),
                os::elevated("cntrl resume")
            );
        }
        UplinkStatus::Disabled {
            gateway,
            message,
            since_ms,
        } => {
            let ago = now_ms().saturating_sub(*since_ms) / 1000;
            let why = message
                .as_deref()
                .unwrap_or("its organization's plan doesn't cover this device")
                .trim_end_matches('.');
            say!(
                "uplink: disabled in Console for {}, at {gateway}: {why}. It comes back on its own once the plan covers it.",
                uptime(ago)
            );
        }
    }
    if let Some(device) = &status.device_id {
        say!("device: {device}");
    }
    match (&status.privd.policy, &status.privd.error) {
        (Some(policy), _) => {
            say!("privd: reachable");
            print_policy_state(policy);
        }
        (None, Some(e)) => say!("privd: unreachable ({e})"),
        (None, None) => say!("privd: reachable, but its policy reply was unreadable"),
    }
    say!("config: {}", status.config);
    ExitCode::SUCCESS
}

/// `cntrl enroll`: hands a token from stdin or a file to the running agent. On a
/// machine in another organization already, it asks before moving it, unless
/// `--move` answered ahead of time (D23).
pub fn run_enroll(config: &Config, token_file: Option<&Path>, replace: bool) -> ExitCode {
    let token = match read_token(token_file) {
        Ok(token) => token,
        Err(e) => return fail(&e),
    };
    // The checksum catches a mistyped token before anything changes.
    if let Err(e) = EnrollToken::parse(&token) {
        return fail(&e.to_string());
    }
    let mut force = replace;
    loop {
        let refused = match send_enroll(config, &token, force) {
            Ok(Ok(outcome)) => {
                match &outcome.replaced {
                    Some(old) => say!("Enrolled as {}, replacing {old}.", outcome.device_id),
                    None => say!("Enrolled as {}.", outcome.device_id),
                }
                say!("Device key fingerprint: {}", outcome.fingerprint);
                say!("Check that Console shows the same fingerprint.");
                // Another organization's, or another device's, signers aren't
                // this one's to trust (D108).
                if outcome.replaced.is_some() {
                    let pinned = signers::path_in(&config.paths.privd_state_dir);
                    match signers::unpin(&pinned) {
                        Ok(true) => say!(
                            "Signed commands aren't required here any more. Once it's online, run `{}` to require its organization's.",
                            os::elevated("cntrl policy require-signatures")
                        ),
                        Ok(false) => {}
                        Err(e) => say_err!("{e}"),
                    }
                }
                return ExitCode::SUCCESS;
            }
            Ok(Err(refused)) => refused,
            Err(e) => return fail(&e),
        };
        let to = refused.to.as_deref().unwrap_or("the token's account");
        match refused.code {
            EnrollErrorCode::AlreadyEnrolled => {
                say!("This machine is already in {to}; nothing changed.");
                return ExitCode::SUCCESS;
            }
            EnrollErrorCode::ConfirmMove if !force => {
                let from = refused.from.as_deref().unwrap_or("another account");
                match confirm_move(from, to) {
                    Some(true) => force = true,
                    Some(false) => return fail("Nothing changed."),
                    None => {
                        return fail(&format!(
                            "This machine is in {from}. To move it to {to}, {}.",
                            move_hint()
                        ));
                    }
                }
            }
            _ => return fail(&refused.msg),
        }
    }
}

/// One enrollment through the agent: its outcome, or Console's answer when it
/// needs the user (`confirm_move`) or there's nothing to do (`already_enrolled`).
fn send_enroll(
    config: &Config,
    token: &str,
    force: bool,
) -> Result<Result<EnrollOutcome, EnrollError>, String> {
    let command = EnrollCommand {
        token: token.to_owned(),
        force,
    };
    let body = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
    let (status, bytes) = block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/enroll",
        body,
    ))?;
    if status.is_success() {
        return serde_json::from_slice(&bytes)
            .map(Ok)
            .map_err(|e| format!("unexpected reply from the agent: {e}"));
    }
    if status == StatusCode::CONFLICT
        && let Ok(refused) = serde_json::from_slice::<EnrollError>(&bytes)
    {
        return Ok(Err(refused));
    }
    Err(String::from_utf8_lossy(&bytes).trim().to_owned())
}

/// Asks on the terminal, which works under `curl | sudo sh` too, since only
/// stdin is the pipe. `None` when there's no terminal to ask on.
fn confirm_move(from: &str, to: &str) -> Option<bool> {
    let (input, mut output) = terminal()?;
    write!(
        output,
        "This machine is in {from}. Move it to {to}?\n{from} loses it, and its history stays there. [y/N] "
    )
    .ok()?;
    let mut answer = String::new();
    BufReader::new(input).read_line(&mut answer).ok()?;
    Some(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// The terminal to read and write: `/dev/tty`, or on Windows the console's
/// own input and output.
fn terminal() -> Option<(fs::File, fs::File)> {
    #[cfg(unix)]
    {
        let tty = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        Some((tty.try_clone().ok()?, tty))
    }
    #[cfg(windows)]
    {
        let input = fs::OpenOptions::new().read(true).open("CONIN$").ok()?;
        let output = fs::OpenOptions::new().write(true).open("CONOUT$").ok()?;
        Some((input, output))
    }
}

/// How to confirm a move where nobody can be asked. The installer sets
/// CNTRL_INSTALLER, since its flag goes after `sh -s --`; on Windows, where
/// `irm | iex` passes no flags, CNTRL_MOVE says it.
fn move_hint() -> String {
    if std::env::var_os("CNTRL_INSTALLER").is_none() {
        format!(
            "run {} with the same token",
            os::elevated("cntrl enroll --move")
        )
    } else if cfg!(windows) {
        "set `$env:CNTRL_MOVE = 1`, then run Add device's command again".to_owned()
    } else {
        "run Add device's command again, ending it with `sudo sh -s -- --move`".to_owned()
    }
}

/// `cntrl policy allow`, `deny` and `modify` (D53): every name is checked
/// first, then the policy file is rewritten once and the agent reconnects
/// once, however many change. Writing the file needs root.
pub fn change_capabilities(config: &Config, allow: &[String], deny: &[String]) -> ExitCode {
    let clean = |names: &[String]| -> Vec<String> {
        names
            .iter()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    };
    let (allow, deny) = (clean(allow), clean(deny));
    if allow.is_empty() && deny.is_empty() {
        return fail(
            "name what to change, such as `--allow services.manage` or `--deny power.poweroff`",
        );
    }
    let path = &config.paths.policy;
    // privd reads the file as root, so root must own it.
    let changed = match policy::modify(path, os::ROOT, &allow, &deny) {
        Ok(changed) => changed,
        Err(e) => return fail(&e),
    };
    for name in allow.iter().filter(|name| !changed.allowed.contains(name)) {
        say!("{name} is already allowed.");
    }
    for name in deny.iter().filter(|name| !changed.denied.contains(name)) {
        say!("{name} isn't allowed.");
    }
    if changed.is_empty() {
        return ExitCode::SUCCESS;
    }
    let mut done = Vec::new();
    if !changed.allowed.is_empty() {
        done.push(format!("Allowed {}", listed(&changed.allowed)));
    }
    if !changed.denied.is_empty() {
        let verb = if done.is_empty() { "Denied" } else { "denied" };
        done.push(format!("{verb} {}", listed(&changed.denied)));
    }
    say!("{} in {}.", done.join("; "), path.display());
    let reload = request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/policy/reload",
        Vec::new(),
    );
    match block_on(reload) {
        Ok((status, _)) if status.is_success() => {
            say!("The agent is reconnecting with the new policy.");
        }
        Ok((_, bytes)) => say_err!(
            "The agent didn't reload: {}",
            String::from_utf8_lossy(&bytes).trim()
        ),
        Err(_) => say!("The agent isn't running; it reads the policy when it starts."),
    }
    ExitCode::SUCCESS
}

/// `a`, `a and b`, `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `cntrl pause`: the agent tells Console who paused it and why, then hangs
/// up until `cntrl resume` (D46). Needs root.
pub fn pause(config: &Config, reason: Option<String>) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "pausing cuts Console off from this machine; run {}",
            os::elevated("cntrl pause")
        ));
    }
    let body = match serde_json::to_vec(&PauseCommand {
        by: os::invoker(),
        reason,
    }) {
        Ok(body) => body,
        Err(e) => return fail(&e.to_string()),
    };
    match block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/pause",
        body,
    )) {
        Ok((status, bytes)) if status.is_success() => {
            let told =
                serde_json::from_slice::<PauseOutcome>(&bytes).is_ok_and(|outcome| outcome.told);
            if told {
                say!(
                    "Paused. Console shows this machine as paused, and nothing reaches it until {}.",
                    os::elevated("cntrl resume")
                );
            } else {
                say!(
                    "Paused, but Console couldn't be told, since the link was down: it shows this machine as offline. {} reconnects.",
                    os::elevated("cntrl resume")
                );
            }
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running, so there's nothing to pause"),
    }
}

/// Before `cntrl uninstall` removes anything (D87): the running agent tells
/// Console who's uninstalling it. Whether Console was told; the uninstall goes
/// on either way.
pub fn goodbye(config: &Config) -> bool {
    let Ok(body) = serde_json::to_vec(&UninstallCommand { by: os::invoker() }) else {
        return false;
    };
    let offline =
        "if this machine is in Console, it shows there as offline until someone removes it";
    match block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/uninstall",
        body,
    )) {
        Ok((status, bytes)) if status.is_success() => {
            let told = serde_json::from_slice::<UninstallOutcome>(&bytes)
                .is_ok_and(|outcome| outcome.told);
            if told {
                say!("Told Console this machine is being uninstalled.");
            } else {
                say!("Console couldn't be told; {offline}.");
            }
            told
        }
        Ok((_, bytes)) => {
            say!(
                "Console couldn't be told ({}); {offline}.",
                String::from_utf8_lossy(&bytes).trim()
            );
            false
        }
        Err(_) => {
            say!("The agent isn't running, so Console couldn't be told; {offline}.");
            false
        }
    }
}

/// `cntrl resume`: the agent reconnects to Console. Needs root.
pub fn resume(config: &Config) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "only {} resumes the agent; run {}",
            os::SUPERUSER,
            os::elevated("cntrl resume")
        ));
    }
    match block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/resume",
        Vec::new(),
    )) {
        Ok((status, bytes)) if status.is_success() => {
            let was = serde_json::from_slice::<ResumeOutcome>(&bytes)
                .is_ok_and(|outcome| outcome.was_paused);
            say!(
                "{}",
                if was {
                    "Resumed; the agent is reconnecting to Console."
                } else {
                    "The agent wasn't paused."
                }
            );
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running; start its service to bring it back"),
    }
}

/// `cntrl history show`: how many days the machine keeps, since when, and the
/// room it takes (D52).
pub fn print_history(config: &Config) -> ExitCode {
    let reply = block_on(request(
        &config.paths.agent_socket,
        Method::GET,
        "/v1/history",
        Vec::new(),
    ));
    match reply {
        Ok((status, bytes)) if status.is_success() => {
            let Ok(store) = serde_json::from_slice::<HistoryStore>(&bytes) else {
                return fail("the agent's answer didn't read");
            };
            say!(
                "Keeps {} of 15-minute points, and 48 hours of 1-minute points, for Console's charts.",
                days(store.keep_days)
            );
            match store.oldest {
                Some(oldest) => say!(
                    "Since {}: {} in {}.",
                    history::utc(oldest),
                    size(store.bytes),
                    config.paths.state_dir.join(HISTORY_DIR).display()
                ),
                None => say!("Nothing recorded yet; the first point comes within a minute."),
            }
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running, so it isn't recording a history"),
    }
}

/// `cntrl history keep <days>`. Needs root.
pub fn keep_history(config: &Config, keep: u32) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "only {} changes how much history is kept; run {}",
            os::SUPERUSER,
            os::elevated("cntrl history keep")
        ));
    }
    let body = match serde_json::to_vec(&HistoryKeepCommand {
        by: os::invoker(),
        days: keep,
    }) {
        Ok(body) => body,
        Err(e) => return fail(&e.to_string()),
    };
    let reply = block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/history/keep",
        body,
    ));
    match reply {
        Ok((status, bytes)) if status.is_success() => {
            let used = serde_json::from_slice::<HistoryStore>(&bytes)
                .map(|store| size(store.bytes))
                .unwrap_or_else(|_| "?".to_owned());
            say!("Keeping {} of history; it takes {used} now.", days(keep));
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running; start its service first"),
    }
}

/// `cntrl history clear`: asks first at a terminal, unless `--yes`. Needs
/// root.
pub fn clear_history(config: &Config, yes: bool) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "only {} clears the history; run {}",
            os::SUPERUSER,
            os::elevated("cntrl history clear")
        ));
    }
    if !yes && io::stdin().is_terminal() {
        eprint!("Delete all of the history this machine keeps for Console's charts? [y/N] ");
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err()
            || !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
        {
            say!("Kept it.");
            return ExitCode::SUCCESS;
        }
    }
    let body = match serde_json::to_vec(&HistoryClearCommand { by: os::invoker() }) {
        Ok(body) => body,
        Err(e) => return fail(&e.to_string()),
    };
    let reply = block_on(request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/history/clear",
        body,
    ));
    match reply {
        Ok((status, _)) if status.is_success() => {
            say!("Cleared. The history starts again with the next minute.");
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running; start its service first"),
    }
}

fn days(count: u32) -> String {
    if count == 1 {
        "1 day".to_owned()
    } else {
        format!("{count} days")
    }
}

/// Bytes in decimal units, as disks are sold.
#[allow(clippy::cast_precision_loss)]
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{:.0} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

fn read_token(file: Option<&Path>) -> Result<String, String> {
    let text = match file {
        Some(path) => {
            fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?
        }
        None => {
            if io::stdin().is_terminal() {
                eprint!("Paste the enrollment token: ");
            }
            let mut line = String::new();
            io::stdin()
                .read_line(&mut line)
                .map_err(|e| format!("can't read the token: {e}"))?;
            line
        }
    };
    Ok(text.trim().to_owned())
}

/// `cntrl policy show` and `cntrl policy check`. The file must belong to root.
pub fn print_policy(config: &Config, check_only: bool) -> ExitCode {
    let state = policy::load(&config.paths.policy, os::ROOT);
    // The pinned signers are in privd's own directory, which only root reads.
    let pin = os::is_root()
        .then(|| signers::load(&signers::path_in(&config.paths.privd_state_dir), os::ROOT));
    let valid = matches!(state, PolicyState::Valid { .. }) && !matches!(pin, Some(Pin::Broken(_)));
    if check_only && valid {
        say!("{}: OK", config.paths.policy.display());
    } else {
        print_policy_state(&state);
        match &pin {
            Some(pin) => print_pin(pin),
            None => say!(
                "signatures: run it {} to see whom this machine trusts",
                os::AS_ROOT
            ),
        }
    }
    if valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `cntrl audit verify`: checks the local audit log's hash chain.
pub fn print_audit_verify(config: &Config) -> ExitCode {
    let path = config.paths.audit_dir.join(audit::FILE_NAME);
    match audit::verify(&path) {
        Ok((records, head)) => {
            let head = head.get(..12).unwrap_or(&head);
            say!(
                "{}: {records} records, chain intact, head {head}",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn print_policy_state(state: &PolicyState) {
    match state {
        PolicyState::Valid { policy } => print_valid_policy(policy),
        PolicyState::Invalid { reason } => {
            say!("policy: INVALID, so every remote action is denied");
            say!("  {reason}");
        }
    }
}

fn print_valid_policy(policy: &Policy) {
    let source = match policy.source {
        Source::File => "from file",
        Source::Default => "built-in monitor-only (no policy file)",
    };
    let hash = policy.hash.get(..12).unwrap_or(&policy.hash);
    say!("policy: {source}, hash {hash}");
    let allow: Vec<&str> = policy.allow.iter().map(String::as_str).collect();
    say!("  allow: {}", allow.join(", "));
    if !policy.protect.is_empty() {
        let protect: Vec<&str> = policy.protect.iter().map(String::as_str).collect();
        say!("  protected units: {}", protect.join(", "));
    }
}

/// Where the machine stands on signed commands (D108).
fn print_pin(pin: &Pin) {
    match pin {
        Pin::Off => say!("signatures: not required"),
        Pin::On(trust) => {
            let head = trust.head.get(..12).unwrap_or(&trust.head);
            say!(
                "signatures: required, from {}'s signers (log entry {}, {head})",
                trust.org,
                trust.seq()
            );
            print_signers(trust);
        }
        Pin::Broken(reason) => {
            say!(
                "signatures: required, but the trusted signers can't be read, so every changing operation is refused"
            );
            say!("  {reason}");
        }
    }
}

/// Each signer's name and fingerprint, to compare with Console's.
fn print_signers(trust: &Trust) {
    let width = trust
        .signers
        .values()
        .map(|signer| signer.name.chars().count())
        .max()
        .unwrap_or(0);
    for signer in trust.signers.values() {
        say!("  {:width$}  {}", signer.name, signer.fingerprint);
    }
}

/// `cntrl policy require-signatures` (D108): shows the signers the gateway
/// relayed last and, with a yes, pins them, so the machine acts on changing
/// operations only when one of them signed. Needs root.
pub fn require_signatures(config: &Config, yes: bool) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "only {} requires signatures; run it {}",
            os::SUPERUSER,
            os::AS_ROOT
        ));
    }
    let latest = signers::latest_path(&config.paths.state_dir);
    let log: cntrl_protocol::signers::SignerLog = match fs::read_to_string(&latest) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(log) => log,
            Err(e) => return fail(&format!("{} is unreadable: {e}", latest.display())),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return fail(
                "This machine hasn't heard of its organization's signers yet. Turn on signed commands in Console, under Organization, wait until the machine is online, then run this again.",
            );
        }
        Err(e) => return fail(&format!("can't read {}: {e}", latest.display())),
    };
    let trust = match Trust::from_log(&log.entries) {
        Ok(trust) => trust,
        Err(e) => {
            return fail(&format!(
                "The signer log doesn't check out, so nothing changed: {e}"
            ));
        }
    };
    let pinned = signers::path_in(&config.paths.privd_state_dir);
    if let Pin::On(current) = signers::load(&pinned, os::ROOT)
        && current.head == trust.head
    {
        say!("Signed commands are already required, from these signers:");
        print_signers(&current);
        return ExitCode::SUCCESS;
    }
    say!(
        "These keys may sign commands for this machine, for {}:",
        trust.org
    );
    print_signers(&trust);
    say!("Compare each fingerprint with Console's, under Organization, Signed commands.");
    if !yes {
        match confirm(
            "Act on commands that change this machine only when one of them signed it? [y/N] ",
        ) {
            Some(true) => {}
            Some(false) => return fail("Nothing changed."),
            None => return fail("There's no terminal to ask on: run it again with --yes."),
        }
    }
    if let Err(e) = signers::save(&pinned, &trust) {
        return fail(&e);
    }
    say!("Signed commands are required now: {}.", pinned.display());
    reload_agent(config);
    ExitCode::SUCCESS
}

/// `cntrl policy allow-unsigned` (D108): the pinned signers go, and changing
/// operations need no signature again. Needs root.
pub fn allow_unsigned(config: &Config) -> ExitCode {
    if !os::is_root() {
        return fail(&format!(
            "only {} stops requiring signatures; run it {}",
            os::SUPERUSER,
            os::AS_ROOT
        ));
    }
    let pinned = signers::path_in(&config.paths.privd_state_dir);
    match signers::unpin(&pinned) {
        Ok(true) => say!("Signed commands aren't required any more."),
        Ok(false) => {
            say!("Signed commands weren't required; nothing changed.");
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(&e),
    }
    reload_agent(config);
    ExitCode::SUCCESS
}

/// Asks the running agent to reconnect, so its hello says what changed.
fn reload_agent(config: &Config) {
    let reload = request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/policy/reload",
        Vec::new(),
    );
    match block_on(reload) {
        Ok((status, _)) if status.is_success() => say!("The agent is reconnecting."),
        Ok((_, bytes)) => say_err!(
            "The agent didn't reload: {}",
            String::from_utf8_lossy(&bytes).trim()
        ),
        Err(_) => say!("The agent isn't running; it reads this when it starts."),
    }
}

/// A yes or no on the terminal; none when there's no terminal.
fn confirm(question: &str) -> Option<bool> {
    let (input, mut output) = terminal()?;
    write!(output, "{question}").ok()?;
    let mut answer = String::new();
    BufReader::new(input).read_line(&mut answer).ok()?;
    Some(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub async fn get_status(socket: &Path) -> Result<Status, String> {
    let (status, bytes) = request(socket, Method::GET, "/v1/status", Vec::new()).await?;
    if !status.is_success() {
        return Err(format!("the agent answered {status}"));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("unexpected status reply: {e}"))
}

/// One HTTP request to the agent's local API.
pub(super) async fn request(
    socket: &Path,
    method: Method,
    path: &str,
    body: Vec<u8>,
) -> Result<(StatusCode, Bytes), String> {
    let stream = os::connect(socket)
        .await
        .map_err(|e| format!("can't reach the agent at {}: {e}", socket.display()))?;
    os::check_agent(&stream)?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!("local API connection ended: {e}");
        }
    });
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "cntrl")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((status, bytes))
}

pub(super) fn block_on<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("can't start the async runtime: {e}"))?
        .block_on(future)
}

fn fail(message: &str) -> ExitCode {
    say_err!("{message}");
    ExitCode::FAILURE
}

fn uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}
