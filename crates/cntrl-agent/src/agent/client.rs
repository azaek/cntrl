//! `cntrl status`: asks the running agent over its local socket.

use std::path::Path;
use std::process::ExitCode;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::{Request, header};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use super::config::Config;
use super::local_api::{Status, Uplink};

pub fn print_status(config_path: &Path, json: bool) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
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
    match runtime.block_on(get_status(&config.paths.agent_socket)) {
        Ok(status) if json => match serde_json::to_string_pretty(&status) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        Ok(status) => {
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
            println!("config: {}", status.config);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
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
