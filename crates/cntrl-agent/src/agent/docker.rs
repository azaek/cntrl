//! Docker and Podman, through their API on a Unix socket, or a named pipe on
//! Windows (D54, angle 19, D58).
//! The socket is root on the host, so only privd talks to it, and only for
//! its own calls: the containers with their counters, one container's log,
//! and start, stop and restart. Paths carry no version prefix, so the engine
//! answers in its own, and Podman's compatibility layer understands them.
//! The system's engine is tried first, then a user's: on a Mac, Docker
//! Desktop and its likes keep their socket in the user's home, and on Linux a
//! rootless engine keeps it under `/run/user`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use cntrl_host::journal;
use cntrl_protocol::containers::{
    Container, ContainerHealth, ContainerPort, ContainerState, Engine, EngineKind,
};
use cntrl_protocol::logs::{LogEntry, LogsBatch, LogsParams};
use futures_util::StreamExt;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, StatusCode, header};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::logs::{Batcher, flush_timer};
use super::os;
use super::uplink::now_ms;

/// Where the system's engines listen, in the order they're tried: Docker's,
/// then rootful Podman's. On a Mac the first is there only when Docker
/// Desktop is set to make it.
#[cfg(unix)]
const SYSTEM_SOCKETS: &[&str] = &["/var/run/docker.sock", "/run/podman/podman.sock"];
/// Docker Desktop's pipe, which Docker Engine on Windows Server also uses,
/// then Podman's machine's.
#[cfg(windows)]
const SYSTEM_SOCKETS: &[&str] = &[
    r"\\.\pipe\docker_engine",
    r"\\.\pipe\podman-machine-default",
];
/// Where a Mac's engines keep their socket, in a user's home: Docker Desktop,
/// OrbStack, Colima and Rancher Desktop.
#[cfg(target_os = "macos")]
const HOME_SOCKETS: &[&str] = &[
    ".docker/run/docker.sock",
    ".orbstack/run/docker.sock",
    ".colima/default/docker.sock",
    ".rd/docker.sock",
];
/// Where a rootless engine keeps its socket, in a user's runtime directory.
#[cfg(target_os = "linux")]
const RUNTIME_SOCKETS: &[&str] = &["docker.sock", "podman/podman.sock"];
/// How long a call may take, but for an action, which waits for the
/// container.
const CALL_LIMIT: Duration = Duration::from_secs(10);
/// How long a start, stop or restart may take: a stop gives the container its
/// grace period, 10 s by default, before it's killed.
pub const ACTION_LIMIT: Duration = Duration::from_secs(60);
/// The most of an answer that's read; hundreds of containers fit well under.
const BODY_LIMIT: usize = 16 * 1024 * 1024;
/// Stats asked for at once. Docker before 25 answers one-shot stats right only
/// when they're asked together, as Beszel found (angle 19).
const STATS_AT_ONCE: usize = 16;
/// A log line longer than this without an end is cut there.
const LINE_LIMIT: usize = 64 * 1024;

/// The containers, for the agent: privd's answer to `Call::Containers`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Listing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    pub containers: Vec<Listed>,
    /// Why there's no list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A container, with its counters when it's running and they were asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listed {
    pub container: Container,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counters: Option<Counters>,
}

/// A running container's counters, which mean something only against an
/// earlier reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// When they were read, in Unix milliseconds.
    pub at: u64,
    /// The container's CPU time, in nanoseconds.
    pub cpu: u64,
    /// The machine's CPU time across all its cores, in nanoseconds, where the
    /// engine gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<u64>,
    pub cpus: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<u64>,
    /// Bytes received and sent over all its networks; none for a container
    /// without its own network, such as one on the host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<(u64, u64)>,
}

/// What an action does to a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    pub fn op(self) -> &'static str {
        match self {
            Self::Start => "container.start",
            Self::Stop => "container.stop",
            Self::Restart => "container.restart",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// Checks a container's ID or name before it goes into a path: Docker's
/// names take letters, digits, `_`, `.` and `-`.
pub fn container_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    let valid = (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && !name.starts_with(['.', '-']);
    if valid {
        Ok(name)
    } else {
        Err(format!("`{name}` isn't a container's ID or name"))
    }
}

/// The first engine socket there is: the system's, then a user's.
async fn socket() -> Option<PathBuf> {
    tokio::task::spawn_blocking(|| candidates().into_iter().find(|path| os::is_endpoint(path)))
        .await
        .ok()?
}

/// Every place an engine's socket may be, in the order they're tried.
fn candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let users = candidates_in(Path::new("/Users"), HOME_SOCKETS);
    #[cfg(target_os = "linux")]
    let users = candidates_in(Path::new("/run/user"), RUNTIME_SOCKETS);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let users = Vec::new();
    SYSTEM_SOCKETS
        .iter()
        .map(PathBuf::from)
        .chain(users)
        .collect()
}

/// Each user's sockets, as `sockets` names them under each of `parent`'s
/// directories: a Mac's homes, or Linux's runtime directories.
#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
fn candidates_in(parent: &Path, sockets: &[&str]) -> Vec<PathBuf> {
    subdirectories(parent)
        .into_iter()
        .flat_map(|user| sockets.iter().map(move |socket| user.join(socket)))
        .collect()
}

/// A directory's subdirectories, by name, without hidden ones.
#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
fn subdirectories(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

#[cfg(target_os = "macos")]
const NO_ENGINE: &str = "No Docker or Podman found: no socket at /var/run/docker.sock, or in a user's Docker Desktop, OrbStack, Colima or Rancher Desktop folder.";
#[cfg(windows)]
const NO_ENGINE: &str = r"No Docker or Podman found: no pipe at \\.\pipe\docker_engine or \\.\pipe\podman-machine-default.";
#[cfg(not(any(target_os = "macos", windows)))]
const NO_ENGINE: &str = "No Docker or Podman found: no socket at /var/run/docker.sock or /run/podman/podman.sock, or a rootless one under /run/user.";

/// Every container, and with `counters`, each running one's counters.
pub async fn list(counters: bool) -> Listing {
    let Some(socket) = socket().await else {
        return Listing {
            note: Some(NO_ENGINE.to_owned()),
            ..Listing::default()
        };
    };
    let failed = |e: String| Listing {
        note: Some(e),
        ..Listing::default()
    };
    let version: Version = match get(&socket, "/version").await {
        Ok(version) => version,
        Err(e) => return failed(e),
    };
    let engine = Engine {
        kind: if version
            .components
            .iter()
            .flatten()
            .any(|component| component.name.to_lowercase().contains("podman"))
        {
            EngineKind::Podman
        } else {
            EngineKind::Docker
        },
        version: version.version,
    };
    let summaries: Vec<Summary> = match get(&socket, "/containers/json?all=1").await {
        Ok(summaries) => summaries,
        Err(e) => {
            return Listing {
                engine: Some(engine),
                ..failed(e)
            };
        }
    };
    let mut containers: Vec<Listed> = summaries
        .into_iter()
        .map(|summary| Listed {
            container: container(summary),
            counters: None,
        })
        .collect();
    if counters {
        let wanted: Vec<(usize, String)> = containers
            .iter()
            .enumerate()
            .filter(|(_, listed)| listed.container.state == ContainerState::Running)
            .map(|(index, listed)| (index, listed.container.id.clone()))
            .collect();
        let socket = &socket;
        let read: Vec<(usize, Option<Counters>)> = futures_util::stream::iter(wanted)
            .map(|(index, id)| async move {
                let path = format!("/containers/{id}/stats?stream=0&one-shot=1");
                let stats = get::<Stats>(socket, &path).await.ok();
                (index, stats.map(|stats| stats.counters(now_ms())))
            })
            .buffer_unordered(STATS_AT_ONCE)
            .collect()
            .await;
        for (index, counters) in read {
            if let Some(listed) = containers.get_mut(index) {
                listed.counters = counters;
            }
        }
    }
    containers.sort_by(|a, b| {
        let key = |listed: &Listed| {
            (
                listed.container.project.is_none(),
                listed.container.project.clone().unwrap_or_default(),
                listed.container.name.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    Listing {
        engine: Some(engine),
        containers,
        note: None,
    }
}

/// Starts, stops or restarts a container, and answers its state after.
pub async fn act(id: &str, action: Action) -> Result<ContainerState, String> {
    let id = container_name(id)?;
    let socket = socket().await.ok_or_else(|| NO_ENGINE.to_owned())?;
    let path = format!("/containers/{id}/{}", action.verb());
    let response = tokio::time::timeout(ACTION_LIMIT, call(&socket, Method::POST, &path))
        .await
        .map_err(|_| "the engine gave no result in time".to_owned())??;
    let status = response.status();
    let body = read_body(response.into_body()).await.unwrap_or_default();
    // 304: it was started or stopped already, which is what was asked.
    if !status.is_success() && status != StatusCode::NOT_MODIFIED {
        return Err(engine_error(status, &body));
    }
    let inspected: Inspect = get(&socket, &format!("/containers/{id}/json")).await?;
    Ok(state(&inspected.state.status))
}

/// Reads one container's log until it ends or the caller drops this: the
/// earlier lines `params.lines` asks for, then new ones. Returns why it
/// stopped.
pub async fn read_logs(params: &LogsParams, out: &mpsc::Sender<LogsBatch>) -> String {
    let Some(id) = params.container.as_deref() else {
        return "no container named".to_owned();
    };
    let id = match container_name(id) {
        Ok(id) => id.to_owned(),
        Err(e) => return e,
    };
    let Some(socket) = socket().await else {
        return NO_ENGINE.to_owned();
    };
    let tail = params
        .lines
        .unwrap_or(journal::DEFAULT_LINES)
        .min(journal::MAX_LINES);
    let path = format!("/containers/{id}/logs?follow=1&stdout=1&stderr=1&timestamps=1&tail={tail}");
    let response = match tokio::time::timeout(CALL_LIMIT, call(&socket, Method::GET, &path)).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => return e,
        Err(_) => return "the engine didn't answer in time".to_owned(),
    };
    let status = response.status();
    if !status.is_success() {
        let body = read_body(response.into_body()).await.unwrap_or_default();
        return engine_error(status, &body);
    }
    let multiplexed = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("multiplexed"));
    let started = now_ms();
    let mut body = response.into_body();
    let mut frames = Frames::new(multiplexed);
    let mut batcher = Batcher::new(params);
    let mut flush = flush_timer();
    let closed = || "the subscription closed".to_owned();
    loop {
        tokio::select! {
            frame = body.frame() => match frame {
                Some(Ok(frame)) => {
                    let Ok(data) = frame.into_data() else { continue };
                    for (stream, line) in frames.push(&data) {
                        let entry = entry(stream, &line);
                        let live = entry.ts >= started;
                        if let Some(full) = batcher.push(entry, live)
                            && out.send(full).await.is_err()
                        {
                            return closed();
                        }
                    }
                }
                Some(Err(e)) => return format!("lost the container's log: {e}"),
                None => break,
            },
            _ = flush.tick() => {
                if let Some(batch) = batcher.take()
                    && out.send(batch).await.is_err()
                {
                    return closed();
                }
            }
        }
    }
    if let Some(batch) = batcher.take() {
        let _ = out.send(batch).await;
    }
    // Following a log ends when the container stops.
    "the container stopped".to_owned()
}

/// A log line as the Logs viewer takes it: its time, from the engine's
/// timestamp, and stdout or stderr as its source.
fn entry(stream: Stream, line: &str) -> LogEntry {
    let (ts, message) = match line.split_once(' ') {
        Some((stamp, message)) => match rfc3339_ms(stamp) {
            Some(ts) => (ts, message),
            None => (0, line),
        },
        None => (rfc3339_ms(line).unwrap_or(0), ""),
    };
    LogEntry {
        ts,
        priority: None,
        source: Some(stream.name().to_owned()),
        pid: None,
        message: journal::cut(message.trim_end_matches('\r').to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Splits a log's body into lines. Without a TTY the engine sends frames, an
/// 8-byte header (the stream, then the payload's size, big-endian) and the
/// payload; with one, plain text. Unknown, the first bytes tell.
struct Frames {
    multiplexed: Option<bool>,
    pending: Vec<u8>,
    lines: [Vec<u8>; 2],
}

impl Frames {
    fn new(multiplexed: Option<bool>) -> Self {
        Self {
            multiplexed,
            pending: Vec::new(),
            lines: [Vec::new(), Vec::new()],
        }
    }

    /// Takes the next bytes of the body; answers the lines they finish.
    fn push(&mut self, data: &[u8]) -> Vec<(Stream, String)> {
        self.pending.extend_from_slice(data);
        let multiplexed = match self.multiplexed {
            Some(known) => known,
            // Too little yet to tell.
            None if self.pending.len() < 8 => return Vec::new(),
            None => {
                let framed = self.pending[0] <= 2 && self.pending[1..4] == [0, 0, 0];
                self.multiplexed = Some(framed);
                framed
            }
        };
        let mut done = Vec::new();
        if multiplexed {
            while self.pending.len() >= 8 {
                let size = u32::from_be_bytes([
                    self.pending[4],
                    self.pending[5],
                    self.pending[6],
                    self.pending[7],
                ]) as usize;
                if self.pending.len() < 8 + size {
                    break;
                }
                let stream = if self.pending[0] == 2 {
                    Stream::Stderr
                } else {
                    Stream::Stdout
                };
                let payload: Vec<u8> = self.pending.drain(..8 + size).skip(8).collect();
                self.lines[usize::from(stream == Stream::Stderr)].extend_from_slice(&payload);
                done.extend(Self::split(
                    &mut self.lines[usize::from(stream == Stream::Stderr)],
                    stream,
                ));
            }
        } else {
            let payload = std::mem::take(&mut self.pending);
            self.lines[0].extend_from_slice(&payload);
            done.extend(Self::split(&mut self.lines[0], Stream::Stdout));
        }
        done
    }

    fn split(buffer: &mut Vec<u8>, stream: Stream) -> Vec<(Stream, String)> {
        let mut lines = Vec::new();
        while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            lines.push((stream, String::from_utf8_lossy(&line[..end]).into_owned()));
        }
        if buffer.len() > LINE_LIMIT {
            let line = std::mem::take(buffer);
            lines.push((stream, String::from_utf8_lossy(&line).into_owned()));
        }
        lines
    }
}

/// An RFC 3339 time, as the engine stamps a log line
/// (`2026-10-06T07:31:09.123456789Z`), in Unix milliseconds.
fn rfc3339_ms(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, offset_minutes) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else {
        let at = time.rfind(['+', '-'])?;
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = zone[1..].split_once(':')?;
        (
            clock,
            sign * (hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?),
        )
    };
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = whole.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let millis: i64 = format!("{fraction:0<3}")
        .get(..3)
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}

/// Days since the Unix epoch of a date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One request to the engine.
async fn call(
    socket: &Path,
    method: Method,
    path: &str,
) -> Result<hyper::Response<Incoming>, String> {
    let stream = os::connect(socket)
        .await
        .map_err(|e| format!("can't reach the engine at {}: {e}", socket.display()))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("can't talk to the engine: {e}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "engine")
        .body(Empty::<Bytes>::new())
        .map_err(|e| e.to_string())?;
    sender
        .send_request(request)
        .await
        .map_err(|e| format!("the engine didn't answer: {e}"))
}

async fn get<T: DeserializeOwned>(socket: &Path, path: &str) -> Result<T, String> {
    let response = tokio::time::timeout(CALL_LIMIT, call(socket, Method::GET, path))
        .await
        .map_err(|_| "the engine didn't answer in time".to_owned())??;
    let status = response.status();
    let body = read_body(response.into_body()).await?;
    if !status.is_success() {
        return Err(engine_error(status, &body));
    }
    serde_json::from_slice(&body).map_err(|e| format!("the engine's answer didn't read: {e}"))
}

async fn read_body(body: Incoming) -> Result<Bytes, String> {
    let collected = tokio::time::timeout(CALL_LIMIT, Limited::new(body, BODY_LIMIT).collect())
        .await
        .map_err(|_| "the engine's answer didn't finish in time".to_owned())?
        .map_err(|e| format!("can't read the engine's answer: {e}"))?;
    Ok(collected.to_bytes())
}

/// The engine's own message for a failed call, which it sends as
/// `{"message": …}`.
fn engine_error(status: StatusCode, body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Message {
        message: String,
    }
    match serde_json::from_slice::<Message>(body) {
        Ok(Message { message }) if status == StatusCode::NOT_FOUND => message,
        Ok(Message { message }) => format!("the engine said: {message}"),
        Err(_) => format!("the engine answered {status}"),
    }
}

fn state(text: &str) -> ContainerState {
    match text {
        "created" => ContainerState::Created,
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "restarting" => ContainerState::Restarting,
        "removing" => ContainerState::Removing,
        "exited" | "stopped" => ContainerState::Exited,
        "dead" => ContainerState::Dead,
        _ => ContainerState::Unknown,
    }
}

fn health(text: &str) -> Option<ContainerHealth> {
    match text {
        "starting" => Some(ContainerHealth::Starting),
        "healthy" => Some(ContainerHealth::Healthy),
        "unhealthy" => Some(ContainerHealth::Unhealthy),
        _ => None,
    }
}

/// The list's entry as the protocol gives it. Health comes from the API from
/// 1.52, and from the status's words before that, and from Podman.
fn container(summary: Summary) -> Container {
    let labels = summary.labels.unwrap_or_default();
    let name = summary
        .names
        .as_deref()
        .and_then(<[String]>::first)
        .map(|name| name.trim_start_matches('/').to_owned())
        .unwrap_or_else(|| summary.id.chars().take(12).collect());
    let health = summary
        .health
        .as_ref()
        .and_then(|health| self::health(&health.status))
        .or_else(|| {
            let status = summary.status.as_str();
            if status.contains("(healthy)") {
                Some(ContainerHealth::Healthy)
            } else if status.contains("(unhealthy)") {
                Some(ContainerHealth::Unhealthy)
            } else if status.contains("(health: starting)") {
                Some(ContainerHealth::Starting)
            } else {
                None
            }
        });
    let mut ports: Vec<ContainerPort> = summary
        .ports
        .unwrap_or_default()
        .into_iter()
        .map(|port| ContainerPort {
            ip: port
                .ip
                .filter(|ip| !ip.is_empty() && ip != "0.0.0.0" && ip != "::"),
            private: port.private_port,
            public: port.public_port.filter(|public| *public > 0),
            protocol: if port.kind.is_empty() {
                "tcp".to_owned()
            } else {
                port.kind
            },
        })
        .collect();
    // Docker lists a port published on both IPv4 and IPv6 twice.
    ports.sort_by(|a, b| {
        (a.private, a.public, &a.protocol).cmp(&(b.private, b.public, &b.protocol))
    });
    ports.dedup_by(|a, b| {
        a.private == b.private && a.public == b.public && a.protocol == b.protocol
    });
    Container {
        id: summary.id.chars().take(12).collect(),
        name,
        image: summary.image,
        project: labels.get("com.docker.compose.project").cloned(),
        service: labels.get("com.docker.compose.service").cloned(),
        state: state(&summary.state),
        health,
        status: summary.status,
        created: u64::try_from(summary.created)
            .unwrap_or(0)
            .saturating_mul(1_000),
        ports,
        cpu: None,
        memory: None,
        memory_limit: None,
        network: None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Version {
    #[serde(default)]
    version: String,
    #[serde(default)]
    components: Option<Vec<Component>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Component {
    #[serde(default)]
    name: String,
}

/// An entry of `/containers/json`, as much of it as is used. Docker 29 sends
/// `null` for a container's empty ports, so lists may be absent or null.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Summary {
    id: String,
    #[serde(default)]
    names: Option<Vec<String>>,
    #[serde(default)]
    image: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    ports: Option<Vec<Port>>,
    #[serde(default)]
    labels: Option<HashMap<String, String>>,
    #[serde(default)]
    health: Option<HealthSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Port {
    #[serde(rename = "IP", default)]
    ip: Option<String>,
    #[serde(default)]
    private_port: u16,
    #[serde(default)]
    public_port: Option<u16>,
    #[serde(rename = "Type", default)]
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HealthSummary {
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspect {
    state: InspectState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectState {
    #[serde(default)]
    status: String,
}

/// `/containers/{id}/stats`, as much of it as is used.
#[derive(Debug, Default, Deserialize)]
struct Stats {
    #[serde(default)]
    cpu_stats: CpuStats,
    #[serde(default)]
    memory_stats: MemoryStats,
    #[serde(default)]
    networks: Option<HashMap<String, NetworkStats>>,
}

#[derive(Debug, Default, Deserialize)]
struct CpuStats {
    #[serde(default)]
    cpu_usage: CpuUsage,
    #[serde(default)]
    system_cpu_usage: Option<u64>,
    #[serde(default)]
    online_cpus: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct CpuUsage {
    #[serde(default)]
    total_usage: u64,
    #[serde(default)]
    percpu_usage: Option<Vec<u64>>,
}

#[derive(Debug, Default, Deserialize)]
struct MemoryStats {
    #[serde(default)]
    usage: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    stats: Option<HashMap<String, u64>>,
}

#[derive(Debug, Deserialize)]
struct NetworkStats {
    #[serde(default)]
    rx_bytes: u64,
    #[serde(default)]
    tx_bytes: u64,
}

impl Stats {
    /// The counters, with memory as the docker CLI counts it: without the
    /// inactive file cache (angle 19).
    fn counters(&self, at: u64) -> Counters {
        let cpu = &self.cpu_stats;
        let cpus = cpu.online_cpus.filter(|n| *n > 0).unwrap_or_else(|| {
            cpu.cpu_usage
                .percpu_usage
                .as_ref()
                .map_or(1, |per| u32::try_from(per.len()).unwrap_or(1).max(1))
        });
        let memory = self.memory_stats.usage.map(|usage| {
            let stats = self.memory_stats.stats.as_ref();
            let cache = stats
                .and_then(|stats| {
                    stats
                        .get("total_inactive_file")
                        .or_else(|| stats.get("inactive_file"))
                })
                .copied()
                .filter(|cache| *cache < usage)
                .unwrap_or(0);
            usage - cache
        });
        let network = self.networks.as_ref().map(|networks| {
            networks.values().fold((0u64, 0u64), |(rx, tx), network| {
                (
                    rx.saturating_add(network.rx_bytes),
                    tx.saturating_add(network.tx_bytes),
                )
            })
        });
        Counters {
            at,
            cpu: cpu.cpu_usage.total_usage,
            system: cpu.system_cpu_usage.filter(|system| *system > 0),
            cpus,
            memory,
            memory_limit: self.memory_stats.limit.filter(|limit| *limit > 0),
            network,
        }
    }
}

/// A container's share of the whole machine's CPU between two readings: its
/// CPU time over the machine's, as the docker CLI counts it, or over the
/// time between them across every core where the engine gives no machine
/// time, as Podman's compatibility layer may not.
#[allow(clippy::cast_precision_loss)]
pub fn cpu_share(earlier: &Counters, later: &Counters) -> Option<f64> {
    let used = later.cpu.checked_sub(earlier.cpu)? as f64;
    let whole = match (earlier.system, later.system) {
        (Some(before), Some(after)) if after > before => (after - before) as f64,
        _ => {
            let elapsed_ns = later.at.checked_sub(earlier.at)? as f64 * 1e6;
            elapsed_ns * f64::from(later.cpus.max(1))
        }
    };
    (whole > 0.0).then(|| (used / whole).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(stream: u8, payload: &str) -> Vec<u8> {
        let mut bytes = vec![stream, 0, 0, 0];
        bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(payload.as_bytes());
        bytes
    }

    #[test]
    fn reads_engine_timestamps() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            rfc3339_ms("2026-10-06T07:31:09.123456789Z"),
            Some(1_791_271_869_123)
        );
        assert_eq!(
            rfc3339_ms("2026-10-06T09:31:09+02:00"),
            Some(1_791_271_869_000)
        );
        assert_eq!(rfc3339_ms("not a time"), None);
    }

    #[test]
    fn splits_frames_into_lines_by_stream() {
        let mut frames = Frames::new(Some(true));
        let mut body = frame(1, "2026-10-06T07:31:09Z first\n2026-10-06T07:31:10Z sec");
        body.extend(frame(2, "2026-10-06T07:31:11Z oops\n"));
        // A frame cut across two reads still makes whole lines.
        let (head, tail) = body.split_at(5);
        assert!(frames.push(head).is_empty());
        let lines = frames.push(tail);
        assert_eq!(
            lines,
            vec![
                (Stream::Stdout, "2026-10-06T07:31:09Z first".to_owned()),
                (Stream::Stderr, "2026-10-06T07:31:11Z oops".to_owned()),
            ]
        );
        let lines = frames.push(&frame(1, "ond\n"));
        assert_eq!(
            lines,
            vec![(Stream::Stdout, "2026-10-06T07:31:10Z second".to_owned())]
        );
    }

    #[test]
    fn a_tty_sends_plain_text() {
        let mut frames = Frames::new(None);
        let lines = frames.push(b"2026-10-06T07:31:09Z hello\r\n");
        assert_eq!(
            lines,
            vec![(Stream::Stdout, "2026-10-06T07:31:09Z hello\r".to_owned())]
        );
        let entry = entry(Stream::Stdout, &lines[0].1);
        assert_eq!(entry.message, "hello");
        assert_eq!(entry.ts, 1_791_271_869_000);
        assert_eq!(entry.source.as_deref(), Some("stdout"));
    }

    #[test]
    #[cfg(unix)]
    fn looks_in_each_users_folder_after_the_systems() {
        let dir = tempfile::tempdir().expect("temp dir");
        for user in ["bob", "alice", ".hidden"] {
            std::fs::create_dir(dir.path().join(user)).expect("a home");
        }
        std::fs::write(dir.path().join("notes.txt"), "").expect("a file");
        let found = candidates_in(dir.path(), &[".docker/run/docker.sock", ".rd/docker.sock"]);
        let names: Vec<String> = found
            .iter()
            .map(|path| {
                path.strip_prefix(dir.path())
                    .expect("under it")
                    .display()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            [
                "alice/.docker/run/docker.sock",
                "alice/.rd/docker.sock",
                "bob/.docker/run/docker.sock",
                "bob/.rd/docker.sock"
            ]
        );
        assert_eq!(candidates()[0], PathBuf::from("/var/run/docker.sock"));
    }

    /// Asks this machine's engine, wherever it listens:
    /// `cargo test -p cntrl-agent lists_this_machines_engine -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a running Docker or Podman"]
    async fn lists_this_machines_engine() {
        let listing = list(true).await;
        println!("socket: {:?}", socket().await);
        println!("engine: {:?}, note: {:?}", listing.engine, listing.note);
        for listed in &listing.containers {
            println!(
                "  {} {:?} {} counters: {}",
                listed.container.name,
                listed.container.state,
                listed.container.image,
                listed.counters.is_some()
            );
        }
        assert!(listing.engine.is_some(), "{:?}", listing.note);
    }

    #[test]
    fn checks_names_before_they_go_in_a_path() {
        assert_eq!(container_name("web-1"), Ok("web-1"));
        assert_eq!(container_name("3f2a9c1b7d4e"), Ok("3f2a9c1b7d4e"));
        assert!(container_name("../etc").is_err());
        assert!(container_name("web?all=1").is_err());
        assert!(container_name("").is_err());
        assert!(container_name("-x").is_err());
    }

    #[test]
    fn makes_a_container_from_the_list() {
        let summary: Summary = serde_json::from_value(serde_json::json!({
            "Id": "3f2a9c1b7d4e5f60718293a4b5c6d7e8f9",
            "Names": ["/app-web-1"],
            "Image": "nginx:1.27",
            "Created": 1_791_000_000,
            "State": "running",
            "Status": "Up 3 hours (healthy)",
            "Ports": [
                {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"IP": "::", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"PrivatePort": 443, "Type": "tcp"}
            ],
            "Labels": {"com.docker.compose.project": "app", "com.docker.compose.service": "web"}
        }))
        .expect("a summary");
        let container = container(summary);
        assert_eq!(container.id, "3f2a9c1b7d4e");
        assert_eq!(container.name, "app-web-1");
        assert_eq!(container.project.as_deref(), Some("app"));
        assert_eq!(container.service.as_deref(), Some("web"));
        assert_eq!(container.state, ContainerState::Running);
        assert_eq!(container.health, Some(ContainerHealth::Healthy));
        assert_eq!(container.created, 1_791_000_000_000);
        assert_eq!(container.ports.len(), 2);
        assert_eq!(container.ports[0].public, Some(8080));
        assert_eq!(container.ports[0].ip, None);
    }

    #[test]
    fn takes_null_lists_as_docker_29_sends_them() {
        let summary: Summary = serde_json::from_value(serde_json::json!({
            "Id": "d568690dea0731bf1fb0fd0fc91dc90014f7a361f90303083d1cac59cc99faec",
            "Names": ["/cntrl-linux-dev"],
            "Image": "cntrl-linux-box",
            "Created": 1_791_022_215,
            "Ports": null,
            "Labels": null,
            "State": "running",
            "Status": "Up 2 days",
            "Health": {"Status": "none", "FailingStreak": 0}
        }))
        .expect("a summary");
        let container = container(summary);
        assert!(container.ports.is_empty());
        assert_eq!(container.health, None);
        assert_eq!(container.name, "cntrl-linux-dev");
    }

    #[test]
    fn counts_cpu_and_memory_as_docker_does() {
        let stats: Stats = serde_json::from_value(serde_json::json!({
            "cpu_stats": {"cpu_usage": {"total_usage": 2_000_000_000u64}, "system_cpu_usage": 100_000_000_000u64, "online_cpus": 4},
            "memory_stats": {"usage": 300, "limit": 1000, "stats": {"inactive_file": 100}},
            "networks": {"eth0": {"rx_bytes": 10, "tx_bytes": 20}, "eth1": {"rx_bytes": 5, "tx_bytes": 5}}
        }))
        .expect("stats");
        let earlier = stats.counters(1_000);
        assert_eq!(earlier.memory, Some(200));
        assert_eq!(earlier.network, Some((15, 25)));
        let later = Counters {
            at: 2_000,
            cpu: 3_000_000_000,
            system: Some(104_000_000_000),
            ..earlier
        };
        // A second of CPU over four seconds of the machine's: a quarter.
        assert_eq!(cpu_share(&earlier, &later), Some(0.25));
        // Without the machine's time: over a second on four cores.
        let podman = Counters {
            system: None,
            ..later
        };
        let before = Counters {
            system: None,
            ..earlier
        };
        assert_eq!(cpu_share(&before, &podman), Some(0.25));
    }
}
