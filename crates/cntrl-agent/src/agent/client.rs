//! The CLI's read-only commands: `cntrl status` asks the running agent over its
//! local socket, and `cntrl policy show|check` reads the policy file.

use std::path::Path;
use std::process::ExitCode;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::{Request, header};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use super::config::Config;
use super::local_api::{Status, Uplink};
use super::policy::{self, Policy, PolicyState, Source};

pub fn print_status(config: &Config, json: bool) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("can't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let status = match runtime.block_on(get_status(&config.paths.agent_socket)) {
        Ok(status) => status,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        return match serde_json::to_string_pretty(&status) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }
    println!(
        "cntrl-agent {}, up {} (pid {})",
        status.version,
        uptime(status.uptime_s),
        status.pid
    );
    let uplink = match status.uplink {
        Uplink::NotEnrolled => "not enrolled",
    };
    println!("uplink: {uplink}");
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
    let request = Request::get("/v1/status")
        .header(header::HOST, "cntrl")
        .body(Empty::<Bytes>::new())
        .map_err(|e| e.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the agent answered {}", response.status()));
    }
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    serde_json::from_slice(&body).map_err(|e| format!("unexpected status reply: {e}"))
}

fn uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// `cntrl audit verify`: checks the local audit log's hash chain.
pub fn print_audit_verify(config: &Config) -> ExitCode {
    let path = config.paths.audit_dir.join(super::audit::FILE_NAME);
    match super::audit::verify(&path) {
        Ok((records, head)) => {
            let head = head.get(..12).unwrap_or(&head);
            println!(
                "{}: {records} records, chain intact, head {head}",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
