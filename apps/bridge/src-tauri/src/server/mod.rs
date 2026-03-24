use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use sysinfo::{Disks, Networks, System};
use tokio::task::JoinHandle;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::auth_scopes::{self, AuthContext, SCOPE_ADMIN};
use crate::auth_store::{self, AuthMode};

pub mod gpu;
pub mod handlers;
pub mod hardware;
pub mod media;
pub mod process;
pub mod types;
pub mod ws;

/// Manages lazy-spawned monitoring loops.
/// Loops only run when there are active subscribers.
pub struct LoopManager {
    stats_handle: Mutex<Option<JoinHandle<()>>>,
    media_handle: Mutex<Option<JoinHandle<()>>>,
    processes_handle: Mutex<Option<JoinHandle<()>>>,
}

impl LoopManager {
    pub fn new() -> Self {
        Self {
            stats_handle: Mutex::new(None),
            media_handle: Mutex::new(None),
            processes_handle: Mutex::new(None),
        }
    }

    /// Called when topic subscriber count goes from 0 -> 1
    pub fn ensure_loop_running(&self, topic: &str, state: Arc<handlers::AppState>) {
        // Determine which loop category this topic belongs to
        let is_stats_topic = matches!(
            topic,
            "stats"
                | "stats.cpu"
                | "stats.memory"
                | "stats.gpu"
                | "stats.disks"
                | "stats.network"
                | "cpu"
                | "memory"
                | "gpu"
                | "disks"
                | "network"
                | "system"
        );
        let is_media_topic = matches!(topic, "media" | "stats.media");
        let is_processes_topic = matches!(topic, "processes" | "process");

        if is_stats_topic {
            let mut handle = self.stats_handle.lock().unwrap();
            if handle.as_ref().map_or(true, |h| h.is_finished()) {
                println!("[LoopManager] Starting stats loop");
                *handle = Some(spawn_stats_loop(state));
            }
        } else if is_media_topic {
            let mut handle = self.media_handle.lock().unwrap();
            if handle.as_ref().map_or(true, |h| h.is_finished()) {
                println!("[LoopManager] Starting media loop");
                *handle = Some(spawn_media_loop(state));
            }
        } else if is_processes_topic {
            let mut handle = self.processes_handle.lock().unwrap();
            if handle.as_ref().map_or(true, |h| h.is_finished()) {
                println!("[LoopManager] Starting processes loop");
                *handle = Some(spawn_processes_loop(state));
            }
        }
    }

    /// Called when topic subscriber count goes from 1 -> 0
    pub fn stop_loop_if_idle(&self, topic: &str, state: &handlers::AppState) {
        let topics = state.active_topics.lock().unwrap();

        // Check if ANY stats sub-topic has subscribers
        let is_stats_topic = matches!(
            topic,
            "stats"
                | "stats.cpu"
                | "stats.memory"
                | "stats.gpu"
                | "stats.disks"
                | "stats.network"
                | "cpu"
                | "memory"
                | "gpu"
                | "disks"
                | "network"
                | "system"
        );
        let is_media_topic = matches!(topic, "media" | "stats.media");
        let is_processes_topic = matches!(topic, "processes" | "process");

        if is_stats_topic {
            let has_stats_subs = [
                "stats",
                "stats.cpu",
                "stats.memory",
                "stats.gpu",
                "stats.disks",
                "stats.network",
                "cpu",
                "memory",
                "gpu",
                "disks",
                "network",
                "system",
            ]
            .iter()
            .any(|t| *topics.get(*t).unwrap_or(&0) > 0);

            if !has_stats_subs {
                drop(topics); // Release lock before acquiring another
                let mut handle = self.stats_handle.lock().unwrap();
                if let Some(h) = handle.take() {
                    println!("[LoopManager] Stopping stats loop");
                    h.abort();
                }
            }
        } else if is_media_topic {
            let has_media_subs = ["media", "stats.media"]
                .iter()
                .any(|t| *topics.get(*t).unwrap_or(&0) > 0);

            if !has_media_subs {
                drop(topics);
                let mut handle = self.media_handle.lock().unwrap();
                if let Some(h) = handle.take() {
                    println!("[LoopManager] Stopping media loop");
                    h.abort();
                }
            }
        } else if is_processes_topic {
            let has_process_subs = ["processes", "process"]
                .iter()
                .any(|t| *topics.get(*t).unwrap_or(&0) > 0);

            if !has_process_subs {
                drop(topics);
                let mut handle = self.processes_handle.lock().unwrap();
                if let Some(h) = handle.take() {
                    println!("[LoopManager] Stopping processes loop");
                    h.abort();
                }
            }
        }
    }
}

/// Spawn the stats monitoring loop. Returns a JoinHandle that can be aborted.
fn spawn_stats_loop(state: Arc<handlers::AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut first_run = true;

        loop {
            // Read interval from config each iteration (allows runtime changes)
            let (interval_ms, enabled) = {
                let config = state.config.lock().unwrap();
                (
                    config.websocket.stats.interval_ms,
                    config.websocket.stats.enabled,
                )
            };

            if !enabled {
                // Loop exits gracefully if disabled
                break;
            }

            // Sleep AFTER first iteration so subscribers get instant data
            if !first_run {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            first_run = false;

            // Check if still have subscribers
            let receiver_count = state.broadcast_tx.receiver_count();
            if receiver_count == 0 {
                continue;
            }

            // Demand-Aware Polling (Granular)
            let (need_cpu, need_mem, need_gpu, need_disks, need_net) = {
                let topics = state.active_topics.lock().unwrap();
                (
                    *topics.get("cpu").unwrap_or(&0) > 0
                        || *topics.get("stats.cpu").unwrap_or(&0) > 0
                        || *topics.get("stats").unwrap_or(&0) > 0,
                    *topics.get("memory").unwrap_or(&0) > 0
                        || *topics.get("stats.memory").unwrap_or(&0) > 0
                        || *topics.get("stats").unwrap_or(&0) > 0,
                    *topics.get("gpu").unwrap_or(&0) > 0
                        || *topics.get("stats.gpu").unwrap_or(&0) > 0
                        || *topics.get("stats").unwrap_or(&0) > 0,
                    *topics.get("disks").unwrap_or(&0) > 0
                        || *topics.get("stats.disks").unwrap_or(&0) > 0
                        || *topics.get("stats").unwrap_or(&0) > 0,
                    *topics.get("network").unwrap_or(&0) > 0
                        || *topics.get("net").unwrap_or(&0) > 0
                        || *topics.get("stats.network").unwrap_or(&0) > 0
                        || *topics.get("stats").unwrap_or(&0) > 0,
                )
            };

            // If nothing is needed, skip
            if !need_cpu && !need_mem && !need_gpu && !need_disks && !need_net {
                continue;
            }

            let payload = {
                let mut sys = state.system.lock().unwrap();
                if need_cpu {
                    sys.refresh_cpu();
                }
                if need_mem {
                    sys.refresh_memory();
                }

                let mut networks = state.networks.lock().unwrap();
                if need_net {
                    networks.refresh();
                }

                types::StreamPayload {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64,
                    uptime: System::uptime(),
                    cpu: if need_cpu {
                        Some(types::CpuUsage {
                            current_load: sys.global_cpu_info().cpu_usage() as f64,
                            current_temp: 0.0,
                            current_speed: crate::server::hardware::get_cpu_speed_ghz(),
                        })
                    } else {
                        None
                    },
                    memory: if need_mem {
                        Some(types::MemoryUsage {
                            used: sys.used_memory(),
                            free: sys.free_memory(),
                            used_percent: (sys.used_memory() as f64 / sys.total_memory() as f64)
                                * 100.0,
                        })
                    } else {
                        None
                    },
                    gpu: if need_gpu {
                        handlers::get_or_update_gpu_stats(&state).map(|g| types::GpuUsage {
                            current_load: g.load_percent,
                            current_temp: g.temp_c,
                            current_memory: g.vram_used_mb,
                        })
                    } else {
                        None
                    },
                    disks: if need_disks {
                        let mut disks_lock = state.disks.lock().unwrap();
                        disks_lock.refresh_list();
                        Some(
                            disks_lock
                                .iter()
                                .map(|d| types::DiskUsage {
                                    fs: d
                                        .mount_point()
                                        .to_string_lossy()
                                        .trim_end_matches('\\')
                                        .into(),
                                    used: d.total_space() - d.available_space(),
                                    available: d.available_space(),
                                    used_percent: ((d.total_space() - d.available_space()) as f64
                                        / d.total_space() as f64)
                                        * 100.0,
                                })
                                .collect(),
                        )
                    } else {
                        None
                    },
                    network: if need_net {
                        let mut sent: u64 = 0;
                        let mut recv: u64 = 0;
                        for (_, network) in networks.iter() {
                            sent += network.total_transmitted();
                            recv += network.total_received();
                        }
                        Some(types::NetworkUsage {
                            bytes_sent: sent,
                            bytes_recv: recv,
                        })
                    } else {
                        None
                    },
                    media: None,
                }
            };

            let _ = state
                .broadcast_tx
                .send(types::BroadcastEvent::SystemStats(payload));
        }
    })
}

/// Spawn the media monitoring loop. Returns a JoinHandle that can be aborted.
fn spawn_media_loop(state: Arc<handlers::AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_status: Option<String> = None;
        let mut first_run = true;

        loop {
            // Read interval from config each iteration
            let (interval_ms, enabled) = {
                let config = state.config.lock().unwrap();
                (
                    config.websocket.media.interval_ms,
                    config.websocket.media.enabled,
                )
            };

            if !enabled {
                break;
            }

            // Sleep AFTER first iteration so subscribers get instant data
            if !first_run {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            first_run = false;

            if state.broadcast_tx.receiver_count() == 0 {
                continue;
            }

            // Check demand
            let need_media = {
                let topics = state.active_topics.lock().unwrap();
                *topics.get("media").unwrap_or(&0) > 0
                    || *topics.get("stats.media").unwrap_or(&0) > 0
            };

            if !need_media {
                continue;
            }

            // Check feature flag
            if !state.config.lock().unwrap().features.enable_media {
                continue;
            }

            if let Some(status) = crate::server::media::get_media_status().await {
                // Include volume in signature so volume changes trigger updates
                let current_sig = format!(
                    "{:?}-{:?}-{:?}-{:?}",
                    status.title, status.playing, status.muted, status.volume
                );
                let changed = match &last_status {
                    Some(s) => s != &current_sig,
                    None => true,
                };

                if changed {
                    last_status = Some(current_sig);
                    let _ = state
                        .broadcast_tx
                        .send(types::BroadcastEvent::MediaUpdate(status));
                }
            }
        }
    })
}

/// Spawn the processes monitoring loop. Returns a JoinHandle that can be aborted.
fn spawn_processes_loop(state: Arc<handlers::AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut first_run = true;

        loop {
            // Read interval from config each iteration
            let (interval_ms, enabled) = {
                let config = state.config.lock().unwrap();
                (
                    config.websocket.processes.interval_ms,
                    config.websocket.processes.enabled,
                )
            };

            if !enabled {
                break;
            }

            // Sleep AFTER first iteration so subscribers get instant data
            if !first_run {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            first_run = false;

            if state.broadcast_tx.receiver_count() == 0 {
                continue;
            }

            // Check demand
            let need_processes = {
                let topics = state.active_topics.lock().unwrap();
                *topics.get("processes").unwrap_or(&0) > 0
                    || *topics.get("process").unwrap_or(&0) > 0
            };

            if !need_processes {
                continue;
            }

            // Check feature flag
            if !state.config.lock().unwrap().features.enable_processes {
                continue;
            }

            let processes = crate::server::process::get_processes_list(&state);
            let total_count = processes.len();

            let payload = types::ProcessListPayload {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                processes,
                total_count,
            };

            let _ = state
                .broadcast_tx
                .send(types::BroadcastEvent::ProcessList(payload));
        }
    })
}

use handlers::*;
use types::ServerStatus;
use ws::ws_handler;

pub async fn start_server(
    port: u16,
    config: Arc<Mutex<AppConfig>>,
    auth_state: Arc<Mutex<auth_store::AuthState>>,
    status_tx: Arc<tokio::sync::watch::Sender<ServerStatus>>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let loop_manager = Arc::new(LoopManager::new());

    let state = Arc::new(AppState {
        system: Arc::new(Mutex::new(System::new_all())),
        networks: Arc::new(Mutex::new({
            let mut n = Networks::new();
            n.refresh_list();
            n
        })),
        disks: Arc::new(Mutex::new({
            let mut d = Disks::new();
            d.refresh_list();
            d
        })),
        gpu_cache: Arc::new(Mutex::new(None)),
        broadcast_tx: {
            let (tx, _rx) = tokio::sync::broadcast::channel(100);
            tx
        },
        active_topics: Arc::new(Mutex::new(std::collections::HashMap::new())),
        config: config,
        auth_state: auth_state,
        loop_manager: loop_manager,
    });

    // No always-running loops! Loops are now lazy-spawned via LoopManager
    // when clients subscribe to topics.
    println!("Server initialized with lazy loop spawning (zero CPU when idle)");

    let app = Router::new()
        .route("/api/status", get(status_handler))
        .route("/api/system", get(get_system_info))
        .route("/api/usage", get(get_system_usage))
        .route("/api/processes", get(list_processes))
        .route("/api/processes/:name", get(get_process_details))
        .route("/api/processes/kill", post(kill_process))
        .route("/api/processes/focus", post(focus_process))
        .route("/api/processes/launch", post(launch_process))
        .route("/api/pw/:action", post(power_action))
        .route("/api/media/control", post(media_control))
        .route("/api/media/status", get(get_media_status))
        .route("/api/stream", get(handle_stream))
        .route("/api/ws", get(ws_handler))
        .route("/api/clients", get(get_client_count))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let host = {
        let c = state.config.lock().unwrap();
        c.server.host.clone()
    };

    let ip_addr: std::net::IpAddr = host.parse().unwrap_or_else(|_| {
        println!("Invalid host '{}', defaulting to 0.0.0.0", host);
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))
    });

    // On Windows, :: is IPv6-only (IPV6_V6ONLY=true by default), so binding 0.0.0.0
    // won't accept IPv6 clients. We bind :: as a second listener to cover both.
    // On Linux/macOS, :: is dual-stack by default, so 0.0.0.0 + :: would conflict on
    // the same port — no second listener needed there.
    let dual_stack = cfg!(target_os = "windows")
        && ip_addr == std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0));

    let addr_v4 = SocketAddr::from((ip_addr, port));
    let listener = match tokio::net::TcpListener::bind(addr_v4).await {
        Ok(l) => l,
        Err(e) => {
            println!("Failed to bind port: {}", e);
            status_tx.send_modify(|s| *s = ServerStatus::Error(format!("Failed to bind port {}: {}", port, e)));
            return;
        }
    };

    let ipv6_listener = if dual_stack {
        let addr_v6 = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));
        match tokio::net::TcpListener::bind(addr_v6).await {
            Ok(l) => {
                println!("Server listening on {} (dual-stack)", addr_v4);
                Some(l)
            }
            Err(e) => {
                // IPv6 not available on this system — continue with IPv4 only
                println!("IPv6 bind failed ({}), continuing with IPv4 only", e);
                println!("Server listening on {}", addr_v4);
                None
            }
        }
    } else {
        println!("Server listening on {}", addr_v4);
        None
    };

    status_tx.send_modify(|s| *s = ServerStatus::Running);

    let app = app.into_make_service_with_connect_info::<SocketAddr>();

    // Broadcast shutdown to both servers
    let (inner_tx, inner_rx1) = tokio::sync::broadcast::channel::<()>(1);
    let inner_rx2 = inner_tx.subscribe();

    // Forward the external shutdown signal to the inner broadcast
    tauri::async_runtime::spawn(async move {
        shutdown_rx.recv().await.ok();
        println!("Server received shutdown signal");
        let _ = inner_tx.send(());
    });

    let status_tx_clone = status_tx.clone();
    if let Some(v6_listener) = ipv6_listener {
        let app_v6 = app.clone();
        let v4_task = tauri::async_runtime::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { inner_rx1.resubscribe().recv().await.ok(); })
                .await
        });
        let v6_task = tauri::async_runtime::spawn(async move {
            axum::serve(v6_listener, app_v6)
                .with_graceful_shutdown(async move { inner_rx2.resubscribe().recv().await.ok(); })
                .await
        });

        let (r4, r6) = tokio::join!(v4_task, v6_task);
        let err = r4.err().map(|e| e.to_string())
            .or_else(|| r6.err().map(|e| e.to_string()));
        if let Some(e) = err {
            status_tx_clone.send_modify(|s| *s = ServerStatus::Error(format!("Server exited: {}", e)));
        } else {
            status_tx_clone.send_modify(|s| *s = ServerStatus::Stopped);
        }
    } else {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move { inner_rx1.resubscribe().recv().await.ok(); })
            .await;
        if let Err(e) = result {
            status_tx_clone.send_modify(|s| *s = ServerStatus::Error(format!("Server exited: {}", e)));
        } else {
            status_tx_clone.send_modify(|s| *s = ServerStatus::Stopped);
        }
    }
}

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (allowed_ips, blocked_ips) = {
        let config = state.config.lock().unwrap();
        (
            config.auth.allowed_ips.clone(),
            config.auth.blocked_ips.clone(),
        )
    };
    let auth_state = { state.auth_state.lock().unwrap().clone() };

    let client_ip = addr.ip().to_string();

    // Always check blocked IPs first (even if auth is disabled)
    if !blocked_ips.is_empty() && is_ip_in_list(&client_ip, &blocked_ips) {
        return StatusCode::FORBIDDEN.into_response();
    }

    // If auth is disabled/public, allow the request (local-only guard)
    if matches!(auth_state.mode, AuthMode::Public) {
        if !is_local_network(addr.ip()) {
            return StatusCode::FORBIDDEN.into_response();
        }
        req.extensions_mut().insert(AuthContext {
            mode: AuthMode::Public,
            scopes: vec![SCOPE_ADMIN.to_string()],
        });
        return next.run(req).await;
    }

    // Check IP in allowlist, if found bypass auth
    if is_ip_in_list(&client_ip, &allowed_ips) {
        req.extensions_mut().insert(AuthContext {
            mode: AuthMode::Protected,
            scopes: vec![SCOPE_ADMIN.to_string()],
        });
        return next.run(req).await;
    }

    let required_scope =
        auth_scopes::required_scope_for_request(req.method().as_str(), req.uri().path());
    let token = extract_token(&req);

    if let Some(token) = token {
        if let Some(record) = auth_store::find_active_record(&auth_state, &token) {
            if let Some(scope) = required_scope {
                if !auth_store::has_scope(&record, scope) {
                    return StatusCode::FORBIDDEN.into_response();
                }
            }
            req.extensions_mut().insert(AuthContext {
                mode: AuthMode::Protected,
                scopes: record.scopes.clone(),
            });
            return next.run(req).await;
        }
        return StatusCode::UNAUTHORIZED.into_response();
    }

    StatusCode::UNAUTHORIZED.into_response()
}

/// Check if an IP matches any entry in a list.
/// Supports exact IP match, CIDR notation, and hostname entries (resolved via DNS).
fn is_ip_in_list(client_ip: &str, list: &[String]) -> bool {
    let client: std::net::IpAddr = match client_ip.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    for entry in list {
        // Exact IP match
        if entry == client_ip {
            return true;
        }

        // CIDR match
        if entry.contains('/') {
            if let Some((network, prefix_len)) = entry.split_once('/') {
                if let (Ok(net_ip), Ok(prefix)) = (
                    network.parse::<std::net::IpAddr>(),
                    prefix_len.parse::<u8>(),
                ) {
                    if ip_matches_cidr(&client, &net_ip, prefix) {
                        return true;
                    }
                }
            }
            continue;
        }

        // If entry is not a valid IP, treat it as a hostname and resolve
        if entry.parse::<std::net::IpAddr>().is_err() {
            use std::net::ToSocketAddrs;
            if let Ok(addrs) = (entry.as_str(), 0u16).to_socket_addrs() {
                for addr in addrs {
                    if addr.ip() == client {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Simple CIDR matching
fn ip_matches_cidr(client: &std::net::IpAddr, network: &std::net::IpAddr, prefix_len: u8) -> bool {
    match (client, network) {
        (std::net::IpAddr::V4(c), std::net::IpAddr::V4(n)) => {
            if prefix_len > 32 {
                return false;
            }
            let mask = if prefix_len == 0 {
                0
            } else {
                !0u32 << (32 - prefix_len)
            };
            let c_bits = u32::from_be_bytes(c.octets());
            let n_bits = u32::from_be_bytes(n.octets());
            (c_bits & mask) == (n_bits & mask)
        }
        (std::net::IpAddr::V6(c), std::net::IpAddr::V6(n)) => {
            if prefix_len > 128 {
                return false;
            }
            let mask = if prefix_len == 0 {
                0
            } else {
                !0u128 << (128 - prefix_len)
            };
            let c_bits = u128::from_be_bytes(c.octets());
            let n_bits = u128::from_be_bytes(n.octets());
            (c_bits & mask) == (n_bits & mask)
        }
        _ => false, // IPv4/IPv6 mismatch
    }
}

fn extract_token(req: &axum::extract::Request) -> Option<String> {
    if let Some(token) = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        return Some(token.to_string());
    }

    // WebSocket fallback for compatibility (deprecated)
    if req.uri().path() == "/api/ws" {
        if let Some(query) = req.uri().query() {
            for param in query.split('&') {
                if let Some(key) = param.strip_prefix("api_key=") {
                    println!("[auth] Deprecated api_key query param used for /api/ws");
                    return Some(key.to_string());
                }
            }
        }
    }

    None
}

fn is_local_network(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local()
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local() {
                return true;
            }
            // IPv4-mapped IPv6 addresses (e.g. ::ffff:192.168.1.x) — check the inner IPv4
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_loopback() || v4.is_private() || v4.is_link_local();
            }
            false
        }
    }
}
