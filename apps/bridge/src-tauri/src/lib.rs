// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

mod server;
mod tray;
mod config;
mod mac_rounded_corners;

use std::sync::{Arc, Mutex};
use config::AppConfig;
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use server::types::{ServerStatus, ServerState};

// Server Control State
struct ServerControl {
    tx: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
}

fn spawn_server(
    port: u16, 
    config: Arc<Mutex<AppConfig>>, 
    status: Arc<Mutex<ServerStatus>>, 
    control: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>
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
fn get_server_status(state: tauri::State<Arc<Mutex<ServerStatus>>>, config: tauri::State<Arc<Mutex<AppConfig>>>) -> ServerState {
    let status = state.lock().unwrap().clone();
    let port = config.lock().unwrap().server.port;
    ServerState {
        status,
        port
    }
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
    app: tauri::AppHandle
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
        control_state.tx.clone()
    );

    Ok(new_config)
}

#[tauri::command]
fn toggle_feature(state: tauri::State<Arc<Mutex<AppConfig>>>, app: tauri::AppHandle, feature: String) -> Result<AppConfig, String> {
    let mut config = state.lock().unwrap();
    match feature.as_str() {
        "system" => config.features.enable_system = !config.features.enable_system,
        "usage" => config.features.enable_usage = !config.features.enable_usage,
        "stream" => config.features.enable_stream = !config.features.enable_stream,
        "media" => config.features.enable_media = !config.features.enable_media,
        "processes" => config.features.enable_processes = !config.features.enable_processes,
        "autostart" => {
            config.features.enable_autostart = !config.features.enable_autostart;
            if config.features.enable_autostart {
                let _ = app.autolaunch().enable();
            } else {
                let _ = app.autolaunch().disable();
            }
        },
        _ => return Err("Feature not found".to_string()),
    }
    
    // Persist changes
    config::save_config(&app, &config);
    
    Ok(config.clone())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            app.manage(ServerControl { tx: control_tx.clone() });

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
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            toggle_feature,
            get_server_status,
            open_config_dir,
            reload_config,
            stop_service,
            mac_rounded_corners::enable_rounded_corners,
            mac_rounded_corners::enable_modern_window_style,
            mac_rounded_corners::reposition_traffic_lights
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
