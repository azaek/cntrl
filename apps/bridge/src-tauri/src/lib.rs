// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

mod auth_scopes;
mod auth_store;
mod config;
mod mac_rounded_corners;
mod server;
mod tray;

use auth_store::{ApiKeyRecord, ApiKeySource, AuthMode, AuthState};
use config::AppConfig;
use serde::Serialize;
use server::types::{ServerState, ServerStatus};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;

// Server Control State
struct ServerControl {
    shutdown_tx: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
    status_tx: Arc<tokio::sync::watch::Sender<ServerStatus>>,
}

fn spawn_server(
    port: u16,
    config: Arc<Mutex<AppConfig>>,
    auth_state: Arc<Mutex<AuthState>>,
    status_tx: Arc<tokio::sync::watch::Sender<ServerStatus>>,
    shutdown_holder: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
) {
    status_tx.send_modify(|s| *s = ServerStatus::Starting);

    let (tx, rx) = tokio::sync::broadcast::channel(1);
    {
        let mut c = shutdown_holder.lock().unwrap();
        *c = Some(tx);
    }

    tauri::async_runtime::spawn(async move {
        server::start_server(port, config, auth_state, status_tx, rx).await;
    });
}

/// Await until the watch channel value satisfies `pred`, with a timeout.
async fn wait_for_status(
    status_tx: &tokio::sync::watch::Sender<ServerStatus>,
    mut pred: impl FnMut(&ServerStatus) -> bool,
) {
    let mut rx = status_tx.subscribe();
    rx.mark_changed(); // Ensure current value is checked immediately
    let timeout = tokio::time::Duration::from_secs(10);
    let _ = tokio::time::timeout(timeout, rx.wait_for(|s| pred(s))).await;
}

#[tauri::command]
fn get_config(state: tauri::State<Arc<Mutex<AppConfig>>>) -> AppConfig {
    state.lock().unwrap().clone()
}

#[tauri::command]
fn get_server_status(
    control_state: tauri::State<ServerControl>,
    config: tauri::State<Arc<Mutex<AppConfig>>>,
) -> ServerState {
    let status = control_state.status_tx.borrow().clone();
    let port = config.lock().unwrap().server.port;
    ServerState { status, port }
}

#[tauri::command]
fn open_config_dir(app: tauri::AppHandle) -> Result<(), String> {
    let path = config::get_config_path(&app);
    if let Some(parent) = path.parent() {
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("explorer")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn stop_service(
    control_state: tauri::State<'_, ServerControl>,
    config: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<ServerState, String> {
    // Extract Arc clones upfront so State borrows don't cross await points
    let status_tx = control_state.status_tx.clone();
    let shutdown_tx = control_state.shutdown_tx.clone();
    let config_arc = config.inner().clone();

    {
        let control = shutdown_tx.lock().unwrap();
        if let Some(tx) = &*control {
            let _ = tx.send(());
        }
    }

    wait_for_status(&status_tx, |s| {
        matches!(s, ServerStatus::Stopped | ServerStatus::Error(_))
    })
    .await;

    let status = status_tx.borrow().clone();
    let port = config_arc.lock().unwrap().server.port;
    Ok(ServerState { status, port })
}

#[tauri::command]
async fn restart_service(
    state: tauri::State<'_, Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<'_, Arc<Mutex<AuthState>>>,
    control_state: tauri::State<'_, ServerControl>,
    app: tauri::AppHandle,
) -> Result<ServerState, String> {
    // Extract Arc clones upfront so State borrows don't cross await points
    let status_tx = control_state.status_tx.clone();
    let shutdown_tx = control_state.shutdown_tx.clone();
    let config_arc = state.inner().clone();
    let auth_arc = auth_state.inner().clone();

    // 1. Stop existing server
    {
        let control = shutdown_tx.lock().unwrap();
        if let Some(tx) = &*control {
            let _ = tx.send(());
        }
    }

    // 2. Wait for server to actually stop
    wait_for_status(&status_tx, |s| {
        matches!(s, ServerStatus::Stopped | ServerStatus::Error(_))
    })
    .await;

    // 3. Reload config from disk
    let mut new_config = config::load_config(&app);
    new_config.auth.api_key = None;
    {
        let mut config = config_arc.lock().unwrap();
        *config = new_config.clone();
    }

    // 4. Start new server
    spawn_server(
        new_config.server.port,
        config_arc.clone(),
        auth_arc,
        status_tx.clone(),
        shutdown_tx,
    );

    // 5. Wait for server to finish starting
    wait_for_status(&status_tx, |s| {
        !matches!(s, ServerStatus::Starting)
    })
    .await;

    let status = status_tx.borrow().clone();
    let port = config_arc.lock().unwrap().server.port;
    Ok(ServerState { status, port })
}

#[tauri::command]
async fn start_service(
    state: tauri::State<'_, Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<'_, Arc<Mutex<AuthState>>>,
    control_state: tauri::State<'_, ServerControl>,
    app: tauri::AppHandle,
) -> Result<ServerState, String> {
    // Extract Arc clones upfront so State borrows don't cross await points
    let status_tx = control_state.status_tx.clone();
    let shutdown_tx = control_state.shutdown_tx.clone();
    let config_arc = state.inner().clone();
    let auth_arc = auth_state.inner().clone();

    // If already running, return current state
    {
        let current = status_tx.borrow().clone();
        if matches!(current, ServerStatus::Running) {
            let port = config_arc.lock().unwrap().server.port;
            return Ok(ServerState { status: current, port });
        }
    }

    // Reload config from disk
    let mut new_config = config::load_config(&app);
    new_config.auth.api_key = None;
    {
        let mut config = config_arc.lock().unwrap();
        *config = new_config.clone();
    }

    // Start server
    spawn_server(
        new_config.server.port,
        config_arc.clone(),
        auth_arc,
        status_tx.clone(),
        shutdown_tx,
    );

    // Wait for server to finish starting
    wait_for_status(&status_tx, |s| {
        !matches!(s, ServerStatus::Starting)
    })
    .await;

    let status = status_tx.borrow().clone();
    let port = config_arc.lock().unwrap().server.port;
    Ok(ServerState { status, port })
}

#[tauri::command]
fn toggle_feature(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    feature: String,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    match feature.as_str() {
        "system" => config.features.enable_system = !config.features.enable_system,
        "usage" => config.features.enable_usage = !config.features.enable_usage,
        "stream" => config.features.enable_stream = !config.features.enable_stream,
        "media" => config.features.enable_media = !config.features.enable_media,
        "processes" => config.features.enable_processes = !config.features.enable_processes,
        "shutdown" => config.features.enable_shutdown = !config.features.enable_shutdown,
        "restart" => config.features.enable_restart = !config.features.enable_restart,
        "hibernate" => config.features.enable_hibernate = !config.features.enable_hibernate,
        "sleep" => config.features.enable_sleep = !config.features.enable_sleep,
        "autostart" => {
            config.features.enable_autostart = !config.features.enable_autostart;
            if config.features.enable_autostart {
                let _ = app.autolaunch().enable();
            } else {
                let _ = app.autolaunch().disable();
            }
        }
        _ => return Err("Feature not found".to_string()),
    }

    // Persist changes
    config::save_config(&app, &config);

    Ok(config.clone())
}

// ============================================================================
// WebSocket Control Commands
// ============================================================================

#[tauri::command]
fn update_ws_interval(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    topic: String,
    interval_ms: u64,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();

    // Validate interval (minimum 100ms, maximum 60s)
    if interval_ms < 100 || interval_ms > 60000 {
        return Err("Interval must be between 100ms and 60000ms".to_string());
    }

    match topic.as_str() {
        "stats" => config.websocket.stats.interval_ms = interval_ms,
        "media" => config.websocket.media.interval_ms = interval_ms,
        "processes" => config.websocket.processes.interval_ms = interval_ms,
        _ => return Err("Invalid topic. Use: stats, media, or processes".to_string()),
    }

    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn toggle_ws_topic(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    topic: String,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();

    match topic.as_str() {
        "stats" => config.websocket.stats.enabled = !config.websocket.stats.enabled,
        "media" => config.websocket.media.enabled = !config.websocket.media.enabled,
        "processes" => config.websocket.processes.enabled = !config.websocket.processes.enabled,
        _ => return Err("Invalid topic. Use: stats, media, or processes".to_string()),
    }

    config::save_config(&app, &config);
    Ok(config.clone())
}

// ============================================================================
// Server Config Commands
// ============================================================================

#[tauri::command]
fn update_server_port(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    port: u16,
) -> Result<AppConfig, String> {
    if port < 1024 {
        return Err("Port must be >= 1024".to_string());
    }

    let mut config = state.lock().unwrap();
    config.server.port = port;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn update_server_host(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    host: String,
) -> Result<AppConfig, String> {
    // Validate host is a valid IP
    if host.parse::<std::net::IpAddr>().is_err() {
        return Err("Invalid IP address".to_string());
    }

    let mut config = state.lock().unwrap();
    config.server.host = host;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn update_hostname(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    hostname: String,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.display.hostname = hostname;
    config::save_config(&app, &config);
    Ok(config.clone())
}

// ============================================================================
// Auth Commands (DEPRECATED — use set_auth_mode, create_api_key, etc.)
// ============================================================================

#[deprecated(note = "use set_auth_mode instead")]
#[tauri::command]
fn toggle_auth(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut auth = auth_state.lock().unwrap();
    auth.mode = match auth.mode {
        AuthMode::Public => AuthMode::Protected,
        AuthMode::Protected => AuthMode::Public,
    };
    auth_store::save_auth_state(&auth)?;

    let mut config = state.lock().unwrap();
    config.auth.enabled = matches!(auth.mode, AuthMode::Protected);
    config::save_config(&app, &config);
    Ok(())
}

#[deprecated(note = "use create_api_key instead")]
#[tauri::command]
fn update_api_key(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    app: tauri::AppHandle,
    api_key: Option<String>,
) -> Result<(), String> {
    let mut auth = auth_state.lock().unwrap();
    auth.keys.retain(|k| k.source != ApiKeySource::Legacy);

    if let Some(token) = api_key.as_ref().map(|s| s.trim().to_string()) {
        if !token.is_empty() {
            let record = auth_store::create_key_record_from_token(
                &token,
                "Legacy Key",
                vec!["admin".to_string()],
                ApiKeySource::Legacy,
                None,
            )?;
            auth.keys.push(record);
        }
    }

    auth_store::save_auth_state(&auth)?;

    let mut config = state.lock().unwrap();
    config.auth.api_key = None;
    config::save_config(&app, &config);
    Ok(())
}

#[deprecated(note = "use create_api_key instead")]
#[tauri::command]
fn generate_api_key(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut auth = auth_state.lock().unwrap();
    auth.keys.retain(|k| k.source != ApiKeySource::Legacy);

    let (record, _token) = auth_store::create_api_key(
        "Legacy Key".to_string(),
        vec!["admin".to_string()],
        None,
        ApiKeySource::Legacy,
    )?;
    auth.keys.push(record);
    auth_store::save_auth_state(&auth)?;

    let mut config = state.lock().unwrap();
    config.auth.api_key = None;
    config::save_config(&app, &config);
    Ok(())
}

/// Validate an IP address, CIDR, or hostname entry.
/// Accepts: raw IP ("192.168.1.1"), CIDR ("192.168.1.0/24"), or hostname ("pi.local").
/// Hostnames are stored as-is and resolved at match time so DHCP changes are handled.
fn validate_ip_entry(input: &str) -> Result<(), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Entry cannot be empty".to_string());
    }

    // Valid IP
    if input.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }

    // CIDR notation — validate network + prefix
    if input.contains('/') {
        if let Some((network, prefix)) = input.split_once('/') {
            if network.parse::<std::net::IpAddr>().is_ok() && prefix.parse::<u8>().is_ok() {
                return Ok(());
            }
        }
        return Err("Invalid CIDR notation".to_string());
    }

    // Hostname: alphanumeric, hyphens, dots, no spaces, reasonable length
    if input.len() <= 253
        && input
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
        && !input.starts_with('-')
        && !input.starts_with('.')
    {
        return Ok(());
    }

    Err("Invalid IP address, CIDR, or hostname".to_string())
}

#[tauri::command]
fn add_allowed_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<(), String> {
    let entry = ip.trim().to_string();
    validate_ip_entry(&entry)?;

    let mut config = state.lock().unwrap();
    if !config.auth.allowed_ips.contains(&entry) {
        config.auth.allowed_ips.push(entry);
    }
    config::save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn remove_allowed_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<(), String> {
    let mut config = state.lock().unwrap();
    config.auth.allowed_ips.retain(|x| x != &ip);
    config::save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn add_blocked_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<(), String> {
    let entry = ip.trim().to_string();
    validate_ip_entry(&entry)?;

    let mut config = state.lock().unwrap();
    if !config.auth.blocked_ips.contains(&entry) {
        config.auth.blocked_ips.push(entry);
    }
    config::save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn remove_blocked_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<(), String> {
    let mut config = state.lock().unwrap();
    config.auth.blocked_ips.retain(|x| x != &ip);
    config::save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn clear_blocked_ips(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut config = state.lock().unwrap();
    config.auth.blocked_ips.clear();
    config::save_config(&app, &config);
    Ok(())
}

// ============================================================================
// Auth Key Management Commands
// ============================================================================

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
struct ApiKeySummary {
    id: String,
    name: String,
    hint: String,
    scopes: Vec<String>,
    created_at: i64,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
    source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct CreateApiKeyResponse {
    key: String,
    record: ApiKeySummary,
}

fn summarize_key(record: &ApiKeyRecord) -> ApiKeySummary {
    ApiKeySummary {
        id: record.id.clone(),
        name: record.name.clone(),
        hint: record.hint.clone(),
        scopes: record.scopes.clone(),
        created_at: record.created_at,
        expires_at: record.expires_at,
        revoked_at: record.revoked_at,
        source: match record.source {
            ApiKeySource::Legacy => "legacy".to_string(),
            ApiKeySource::User => "user".to_string(),
        },
    }
}

#[tauri::command]
fn set_auth_mode(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    app: tauri::AppHandle,
    mode: String,
) -> Result<(), String> {
    let mode = match mode.as_str() {
        "public" => AuthMode::Public,
        "protected" => AuthMode::Protected,
        _ => return Err("Invalid auth mode".to_string()),
    };

    let mut auth = auth_state.lock().unwrap();
    auth.mode = mode;
    auth_store::save_auth_state(&auth)?;

    let mut config = state.lock().unwrap();
    config.auth.enabled = matches!(auth.mode, AuthMode::Protected);
    config::save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn list_api_keys(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
) -> Result<Vec<ApiKeySummary>, String> {
    let auth = auth_state.lock().unwrap();
    Ok(auth.keys.iter().map(summarize_key).collect())
}

#[tauri::command]
fn create_api_key(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    name: Option<String>,
    scopes: Vec<String>,
    expires_at: Option<i64>,
) -> Result<CreateApiKeyResponse, String> {
    let name = name.unwrap_or_else(|| "API Key".to_string());
    let (record, key) = auth_store::create_api_key(name, scopes, expires_at, ApiKeySource::User)?;

    let mut auth = auth_state.lock().unwrap();
    auth.keys.push(record.clone());
    auth_store::save_auth_state(&auth)?;

    Ok(CreateApiKeyResponse {
        key,
        record: summarize_key(&record),
    })
}

#[tauri::command]
fn revoke_api_key(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    id: String,
) -> Result<bool, String> {
    let mut auth = auth_state.lock().unwrap();
    let updated = auth_store::revoke_key(&mut auth, &id);
    auth_store::save_auth_state(&auth)?;
    Ok(updated)
}

#[tauri::command]
fn remove_api_key(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    id: String,
) -> Result<bool, String> {
    let mut auth = auth_state.lock().unwrap();
    let removed = auth_store::remove_key(&mut auth, &id);
    auth_store::save_auth_state(&auth)?;
    Ok(removed)
}

#[tauri::command]
fn update_api_key_scopes(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    id: String,
    scopes: Vec<String>,
) -> Result<bool, String> {
    let mut auth = auth_state.lock().unwrap();
    let updated = auth_store::update_key_scopes(&mut auth, &id, scopes);
    auth_store::save_auth_state(&auth)?;
    Ok(updated)
}

#[tauri::command]
fn update_api_key_expiration(
    auth_state: tauri::State<Arc<Mutex<AuthState>>>,
    id: String,
    expires_at: Option<i64>,
) -> Result<bool, String> {
    let mut auth = auth_state.lock().unwrap();
    let updated = auth_store::update_key_expiration(&mut auth, &id, expires_at);
    auth_store::save_auth_state(&auth)?;
    Ok(updated)
}

// ============================================================================
// Stats Config Commands
// ============================================================================

#[tauri::command]
fn toggle_gpu_stats(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.stats.gpu_enabled = !config.stats.gpu_enabled;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn update_disk_cache_seconds(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    seconds: u64,
) -> Result<AppConfig, String> {
    if seconds < 1 || seconds > 300 {
        return Err("Seconds must be between 1 and 300".to_string());
    }

    let mut config = state.lock().unwrap();
    config.stats.disk_cache_seconds = seconds;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn update_stream_interval(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    seconds: u64,
) -> Result<AppConfig, String> {
    if seconds < 1 || seconds > 60 {
        return Err("Seconds must be between 1 and 60".to_string());
    }

    let mut config = state.lock().unwrap();
    config.stats.stream_interval_seconds = seconds;
    config::save_config(&app, &config);
    Ok(config.clone())
}

// ============================================================================
// Batch Config Update
// ============================================================================

#[tauri::command]
fn update_config(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    updates: serde_json::Value,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();

    // Apply server updates
    if let Some(server) = updates.get("server") {
        if let Some(port) = server.get("port").and_then(|v| v.as_u64()) {
            if port < 1024 || port > 65535 {
                return Err("Port must be between 1024 and 65535".to_string());
            }
            config.server.port = port as u16;
        }
        if let Some(host) = server.get("host").and_then(|v| v.as_str()) {
            if host.parse::<std::net::IpAddr>().is_err() {
                return Err("Invalid IP address".to_string());
            }
            config.server.host = host.to_string();
        }
    }

    // Apply display updates
    if let Some(display) = updates.get("display") {
        if let Some(hostname) = display.get("hostname").and_then(|v| v.as_str()) {
            config.display.hostname = hostname.to_string();
        }
    }

    // Apply stats updates
    if let Some(stats) = updates.get("stats") {
        if let Some(gpu) = stats.get("gpu_enabled").and_then(|v| v.as_bool()) {
            config.stats.gpu_enabled = gpu;
        }
        if let Some(disk) = stats.get("disk_cache_seconds").and_then(|v| v.as_u64()) {
            config.stats.disk_cache_seconds = disk;
        }
        if let Some(stream) = stats
            .get("stream_interval_seconds")
            .and_then(|v| v.as_u64())
        {
            config.stats.stream_interval_seconds = stream;
        }
    }

    // Apply websocket updates
    if let Some(ws) = updates.get("websocket") {
        if let Some(stats_cfg) = ws.get("stats") {
            if let Some(enabled) = stats_cfg.get("enabled").and_then(|v| v.as_bool()) {
                config.websocket.stats.enabled = enabled;
            }
            if let Some(interval) = stats_cfg.get("interval_ms").and_then(|v| v.as_u64()) {
                config.websocket.stats.interval_ms = interval;
            }
        }
        if let Some(media_cfg) = ws.get("media") {
            if let Some(enabled) = media_cfg.get("enabled").and_then(|v| v.as_bool()) {
                config.websocket.media.enabled = enabled;
            }
            if let Some(interval) = media_cfg.get("interval_ms").and_then(|v| v.as_u64()) {
                config.websocket.media.interval_ms = interval;
            }
        }
        if let Some(proc_cfg) = ws.get("processes") {
            if let Some(enabled) = proc_cfg.get("enabled").and_then(|v| v.as_bool()) {
                config.websocket.processes.enabled = enabled;
            }
            if let Some(interval) = proc_cfg.get("interval_ms").and_then(|v| v.as_u64()) {
                config.websocket.processes.interval_ms = interval;
            }
        }
    }

    config::save_config(&app, &config);
    Ok(config.clone())
}

// ============================================================================
// Network Utility Commands
// ============================================================================

#[tauri::command]
fn get_local_ips() -> Vec<String> {
    let mut ips = Vec::new();

    if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in interfaces {
            if let std::net::IpAddr::V4(ipv4) = ip {
                if !ipv4.is_loopback() {
                    ips.push(ipv4.to_string());
                }
            }
        }
    }

    ips
}

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let current_version = env!("CARGO_PKG_VERSION");
    let update_endpoint = if current_version.contains("beta") {
        "https://api.cntrl.pw/updates/bridge/beta"
    } else {
        "https://api.cntrl.pw/updates/bridge/stable"
    };

    let update = app
        .updater_builder()
        .endpoints(vec![
            url::Url::parse(update_endpoint).map_err(|e| e.to_string())?,
        ])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;

    Ok(update.map(|u| u.version))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Hide dock icon on macOS (tray-only app)
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let mut cfg = config::load_config(app.handle());
            let port = cfg.server.port;
            let autostart_enabled = cfg.features.enable_autostart;

            let auth_state = auth_store::load_or_migrate(&cfg);
            cfg.auth.api_key = None;
            let shared_auth = Arc::new(Mutex::new(auth_state));
            app.manage(shared_auth.clone());

            // Share config state
            let shared_config = Arc::new(Mutex::new(cfg));
            app.manage(shared_config.clone());

            // Server Control with watch channel
            let (status_tx, _status_rx) = tokio::sync::watch::channel(ServerStatus::Starting);
            let status_tx = Arc::new(status_tx);
            let shutdown_tx = Arc::new(Mutex::new(None));

            app.manage(ServerControl {
                shutdown_tx: shutdown_tx.clone(),
                status_tx: status_tx.clone(),
            });

            // Start initial server
            let auth_state = app.state::<Arc<Mutex<AuthState>>>().inner().clone();
            spawn_server(port, shared_config, auth_state, status_tx, shutdown_tx);

            // Sync Autostart
            // let autostart_enabled is extracted above

            if autostart_enabled {
                let _ = app.handle().autolaunch().enable();
            } else {
                let _ = app.handle().autolaunch().disable();
            }

            tray::create_tray(app.handle())?;

            // Only create the window when NOT launched minimized (e.g. autostart)
            let minimized = std::env::args().any(|a| a == "--minimized");
            if !minimized {
                tray::show_or_create_window(app.handle());
            }

            Ok(())
        })
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .invoke_handler(tauri::generate_handler![
            check_for_updates,
            greet,
            get_app_version,
            get_config,
            toggle_feature,
            get_server_status,
            open_config_dir,
            restart_service,
            start_service,
            stop_service,
            // WebSocket controls
            update_ws_interval,
            toggle_ws_topic,
            // Server config
            update_server_port,
            update_server_host,
            update_hostname,
            // Auth
            toggle_auth,
            update_api_key,
            generate_api_key,
            set_auth_mode,
            list_api_keys,
            create_api_key,
            revoke_api_key,
            remove_api_key,
            update_api_key_scopes,
            update_api_key_expiration,
            add_allowed_ip,
            remove_allowed_ip,
            add_blocked_ip,
            remove_blocked_ip,
            clear_blocked_ips,
            // Stats config
            toggle_gpu_stats,
            update_disk_cache_seconds,
            update_stream_interval,
            // Batch update
            update_config,
            // Network utilities
            get_local_ips,
            // macOS window styling
            mac_rounded_corners::enable_rounded_corners,
            mac_rounded_corners::enable_modern_window_style,
            mac_rounded_corners::reposition_traffic_lights
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
