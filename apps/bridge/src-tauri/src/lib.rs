// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

mod config;
mod mac_rounded_corners;
mod server;
mod tray;

use config::AppConfig;
use server::types::{ServerState, ServerStatus};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;

// Server Control State
struct ServerControl {
    tx: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
}

fn spawn_server(
    port: u16,
    config: Arc<Mutex<AppConfig>>,
    status: Arc<Mutex<ServerStatus>>,
    control: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
) {
    let (tx, rx) = tokio::sync::broadcast::channel(1);

    // Store sender
    let mut c = control.lock().unwrap();
    *c = Some(tx);

    tauri::async_runtime::spawn(async move {
        server::start_server(port, config, status, rx).await;
    });
}

#[tauri::command]
fn get_config(state: tauri::State<Arc<Mutex<AppConfig>>>) -> AppConfig {
    state.lock().unwrap().clone()
}

#[tauri::command]
fn get_server_status(
    state: tauri::State<Arc<Mutex<ServerStatus>>>,
    config: tauri::State<Arc<Mutex<AppConfig>>>,
) -> ServerState {
    let status = state.lock().unwrap().clone();
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
fn stop_service(control_state: tauri::State<ServerControl>) -> Result<(), String> {
    let control = control_state.tx.lock().unwrap();
    if let Some(tx) = &*control {
        let _ = tx.send(()); // Send shutdown signal to active server
    }
    Ok(())
}

#[tauri::command]
fn reload_config(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    status_state: tauri::State<Arc<Mutex<ServerStatus>>>,
    control_state: tauri::State<ServerControl>,
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    let new_config = config::load_config(&app);

    // 1. Update config state
    {
        let mut config = state.lock().unwrap();
        *config = new_config.clone();
    }

    // 2. Stop existing server
    {
        let control = control_state.tx.lock().unwrap();
        if let Some(tx) = &*control {
            let _ = tx.send(()); // Send shutdown signal
        }
    }

    // 3. Wait a bit for port release (simple delay)
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Start new server
    spawn_server(
        new_config.server.port,
        state.inner().clone(),
        status_state.inner().clone(),
        control_state.tx.clone(),
    );

    Ok(new_config)
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
// Auth Commands
// ============================================================================

#[tauri::command]
fn toggle_auth(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.auth.enabled = !config.auth.enabled;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn update_api_key(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    api_key: Option<String>,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.auth.api_key = api_key;
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn generate_api_key(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Generate a random-ish key using timestamp and random bytes
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let key = format!("ck_{:x}_{:x}", timestamp, rand::random::<u64>());

    let mut config = state.lock().unwrap();
    config.auth.api_key = Some(key);
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn add_allowed_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<AppConfig, String> {
    // Validate IP
    if ip.parse::<std::net::IpAddr>().is_err() && !ip.contains('/') {
        return Err("Invalid IP address or CIDR".to_string());
    }

    let mut config = state.lock().unwrap();
    if !config.auth.allowed_ips.contains(&ip) {
        config.auth.allowed_ips.push(ip);
    }
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn remove_allowed_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.auth.allowed_ips.retain(|x| x != &ip);
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn add_blocked_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<AppConfig, String> {
    // Validate IP or CIDR
    if ip.parse::<std::net::IpAddr>().is_err() && !ip.contains('/') {
        return Err("Invalid IP address or CIDR".to_string());
    }

    let mut config = state.lock().unwrap();
    if !config.auth.blocked_ips.contains(&ip) {
        config.auth.blocked_ips.push(ip);
    }
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn remove_blocked_ip(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
    ip: String,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.auth.blocked_ips.retain(|x| x != &ip);
    config::save_config(&app, &config);
    Ok(config.clone())
}

#[tauri::command]
fn clear_blocked_ips(
    state: tauri::State<Arc<Mutex<AppConfig>>>,
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    config.auth.blocked_ips.clear();
    config::save_config(&app, &config);
    Ok(config.clone())
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
            config.server.port = port as u16;
        }
        if let Some(host) = server.get("host").and_then(|v| v.as_str()) {
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
        "https://raw.githubusercontent.com/azaek/cntrl/updates/updates/latest-beta.json"
    } else {
        "https://raw.githubusercontent.com/azaek/cntrl/updates/updates/latest-stable.json"
    };

    let update = app
        .updater_builder()
        .endpoints(vec![url::Url::parse(update_endpoint).map_err(|e| e.to_string())?])
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
            let cfg = config::load_config(app.handle());
            let port = cfg.server.port;
            let autostart_enabled = cfg.features.enable_autostart;

            // Share config state
            let shared_config = Arc::new(Mutex::new(cfg));
            app.manage(shared_config.clone());

            // Server Status State
            let server_status = Arc::new(Mutex::new(ServerStatus::Starting));
            app.manage(server_status.clone());

            // Server Control
            let control_tx = Arc::new(Mutex::new(None));
            app.manage(ServerControl {
                tx: control_tx.clone(),
            });

            // Start initial server
            spawn_server(port, shared_config, server_status, control_tx);

            // Sync Autostart
            // let autostart_enabled is extracted above

            if autostart_enabled {
                let _ = app.handle().autolaunch().enable();
            } else {
                let _ = app.handle().autolaunch().disable();
            }

            tray::create_tray(app.handle())?;

            // Handle window close event to hide instead of exit
            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Prevent the window from closing
                        api.prevent_close();
                        // Hide the window instead
                        let _ = window_clone.hide();
                    }
                });
            }

            Ok(())
        })
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
            reload_config,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
