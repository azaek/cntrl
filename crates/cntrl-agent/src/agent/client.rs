//! The CLI commands that talk to the running agent or read its files:
//! `cntrl status`, `cntrl enroll`, `cntrl policy show|check|allow` and
//! `cntrl audit verify`.

use std::fs;
use std::future::Future;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use cntrl_protocol::enroll::{EnrollError, EnrollErrorCode, EnrollToken};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode, header};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use super::audit;
use super::config::Config;
use super::enroll::{EnrollCommand, EnrollOutcome};
use super::local_api::{PauseCommand, PauseOutcome, ResumeOutcome, Status};
use super::policy::{self, Policy, PolicyState, Source};
use super::uplink::{UplinkStatus, now_ms};

pub fn print_status(config: &Config, json: bool) -> ExitCode {
    let status = match block_on(get_status(&config.paths.agent_socket)) {
        Ok(status) => status,
        Err(e) => return fail(&e),
    };
    if json {
        return match serde_json::to_string_pretty(&status) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e.to_string()),
        };
    }
    println!(
        "cntrl-agent {}, up {} (pid {})",
        status.version,
        uptime(status.uptime_s),
        status.pid
    );
    match &status.uplink {
        UplinkStatus::NotEnrolled => println!("uplink: not enrolled"),
        UplinkStatus::Connecting { gateway, attempt } => {
            println!("uplink: connecting to {gateway} (attempt {})", attempt + 1);
        }
        UplinkStatus::Online {
            gateway,
            session,
            since_ms,
        } => {
            let up = now_ms().saturating_sub(*since_ms) / 1000;
            println!(
                "uplink: online at {gateway} for {} (session {session})",
                uptime(up)
            );
        }
        UplinkStatus::Retrying {
            reason,
            retry_at_ms,
        } => {
            let wait = retry_at_ms.saturating_sub(now_ms()).div_ceil(1000);
            println!("uplink: retrying in {wait}s; {reason}");
        }
        UplinkStatus::Stopped { reason } => println!("uplink: stopped; {reason}"),
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
            println!(
                "uplink: paused by {by} {} ago{why}; `sudo cntrl resume` reconnects",
                uptime(ago)
            );
        }
    }
    if let Some(device) = &status.device_id {
        println!("device: {device}");
    }
    match (&status.privd.policy, &status.privd.error) {
        (Some(policy), _) => {
            println!("privd: reachable");
            print_policy_state(policy);
        }
        (None, Some(e)) => println!("privd: unreachable ({e})"),
        (None, None) => println!("privd: reachable, but its policy reply was unreadable"),
    }
    println!("config: {}", status.config);
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
                    Some(old) => println!("Enrolled as {}, replacing {old}.", outcome.device_id),
                    None => println!("Enrolled as {}.", outcome.device_id),
                }
                println!("Device key fingerprint: {}", outcome.fingerprint);
                println!("Check that Console shows the same fingerprint.");
                return ExitCode::SUCCESS;
            }
            Ok(Err(refused)) => refused,
            Err(e) => return fail(&e),
        };
        let to = refused.to.as_deref().unwrap_or("the token's account");
        match refused.code {
            EnrollErrorCode::AlreadyEnrolled => {
                println!("This machine is already in {to}; nothing changed.");
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
    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    write!(
        tty,
        "This machine is in {from}. Move it to {to}?\n{from} loses it, and its history stays there. [y/N] "
    )
    .ok()?;
    let mut answer = String::new();
    BufReader::new(tty).read_line(&mut answer).ok()?;
    Some(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// How to confirm a move where nobody can be asked. The installer sets
/// CNTRL_INSTALLER, since its flag goes after `sh -s --`.
fn move_hint() -> &'static str {
    if std::env::var_os("CNTRL_INSTALLER").is_some() {
        "run Add device's command again, ending it with `sudo sh -s -- --move`"
    } else {
        "run `sudo cntrl enroll --move` with the same token"
    }
}

/// `cntrl policy allow` and `cntrl policy deny`: change the policy file, then
/// have the running agent reconnect, so Console sees the new policy in its hello.
pub fn change_capability(config: &Config, capability: &str, allowed: bool) -> ExitCode {
    let path = &config.paths.policy;
    // privd reads the file as root, so root must own it.
    let changed = if allowed {
        policy::allow(path, 0, capability)
    } else {
        policy::deny(path, 0, capability)
    };
    match (changed, allowed) {
        (Ok(false), true) => {
            println!("{capability} is already allowed.");
            return ExitCode::SUCCESS;
        }
        (Ok(false), false) => {
            println!("{capability} isn't allowed.");
            return ExitCode::SUCCESS;
        }
        (Ok(true), true) => println!("Allowed {capability} in {}.", path.display()),
        (Ok(true), false) => println!("Denied {capability} in {}.", path.display()),
        (Err(e), _) => return fail(&e),
    }
    let reload = request(
        &config.paths.agent_socket,
        Method::POST,
        "/v1/policy/reload",
        Vec::new(),
    );
    match block_on(reload) {
        Ok((status, _)) if status.is_success() => {
            println!("The agent is reconnecting with the new policy.");
        }
        Ok((_, bytes)) => eprintln!(
            "The agent didn't reload: {}",
            String::from_utf8_lossy(&bytes).trim()
        ),
        Err(_) => println!("The agent isn't running; it reads the policy when it starts."),
    }
    ExitCode::SUCCESS
}

/// `cntrl pause`: the agent tells Console who paused it and why, then hangs
/// up until `cntrl resume` (D46). Needs root.
pub fn pause(config: &Config, reason: Option<String>) -> ExitCode {
    if !rustix::process::geteuid().is_root() {
        return fail("pausing cuts Console off from this machine; run `sudo cntrl pause`");
    }
    // Who ran sudo, as this machine names them.
    let by = std::env::var("SUDO_USER")
        .ok()
        .filter(|user| !user.is_empty())
        .unwrap_or_else(|| "root".to_owned());
    let body = match serde_json::to_vec(&PauseCommand { by, reason }) {
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
                println!(
                    "Paused. Console shows this machine as paused, and nothing reaches it until `sudo cntrl resume`."
                );
            } else {
                println!(
                    "Paused, but Console couldn't be told, since the link was down: it shows this machine as offline. `sudo cntrl resume` reconnects."
                );
            }
            ExitCode::SUCCESS
        }
        Ok((_, bytes)) => fail(String::from_utf8_lossy(&bytes).trim()),
        Err(_) => fail("the agent isn't running, so there's nothing to pause"),
    }
}

/// `cntrl resume`: the agent reconnects to Console. Needs root.
pub fn resume(config: &Config) -> ExitCode {
    if !rustix::process::geteuid().is_root() {
        return fail("only root resumes the agent; run `sudo cntrl resume`");
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
            println!(
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
    let state = policy::load(&config.paths.policy, 0);
    let valid = matches!(state, PolicyState::Valid { .. });
    if check_only && valid {
        println!("{}: OK", config.paths.policy.display());
    } else {
        print_policy_state(&state);
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
            println!(
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
            println!("policy: INVALID, so every remote action is denied");
            println!("  {reason}");
        }
    }
}

fn print_valid_policy(policy: &Policy) {
    let source = match policy.source {
        Source::File => "from file",
        Source::Default => "built-in monitor-only (no policy file)",
    };
    let hash = policy.hash.get(..12).unwrap_or(&policy.hash);
    println!("policy: {source}, hash {hash}");
    let allow: Vec<&str> = policy.allow.iter().map(String::as_str).collect();
    println!("  allow: {}", allow.join(", "));
    if !policy.protect.is_empty() {
        let protect: Vec<&str> = policy.protect.iter().map(String::as_str).collect();
        println!("  protected units: {}", protect.join(", "));
    }
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
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| format!("can't reach the agent at {}: {e}", socket.display()))?;
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
    eprintln!("{message}");
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
