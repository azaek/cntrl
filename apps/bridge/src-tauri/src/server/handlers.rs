use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, Sse},
    Json,
};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use sysinfo::{Disks, Networks, Pid, System};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, IsWindowVisible, SetForegroundWindow, ShowWindow, SwitchToThisWindow, SW_RESTORE,
};

use crate::config::AppConfig;
use crate::server::types::*;

/// Subscribe to topics - increments ref counts and starts loops if needed
pub fn subscribe_topics(state: &Arc<AppState>, topics: &[&str]) {
    let mut global_map = state.active_topics.lock().unwrap();
    let mut topics_to_start = Vec::new();

    for topic in topics {
        let entry = global_map.entry(topic.to_string()).or_insert(0);
        if *entry == 0 {
            topics_to_start.push(topic.to_string());
        }
        *entry += 1;
    }
    drop(global_map);

    // Start loops outside of lock
    for topic in topics_to_start {
        state
            .loop_manager
            .ensure_loop_running(&topic, state.clone());
    }
}

/// Unsubscribe from topics - decrements ref counts and stops loops if needed
pub fn unsubscribe_topics(state: &Arc<AppState>, topics: &[&str]) {
    let mut global_map = state.active_topics.lock().unwrap();
    let mut topics_to_stop = Vec::new();

    for topic in topics {
        if let Some(count) = global_map.get_mut(*topic) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    topics_to_stop.push(topic.to_string());
                }
            }
        }
    }
    drop(global_map);

    // Stop loops outside of lock
    for topic in topics_to_stop {
        state.loop_manager.stop_loop_if_idle(&topic, state);
    }
}

pub struct AppState {
    pub system: Arc<Mutex<System>>,
    pub networks: Arc<Mutex<Networks>>,
    pub disks: Arc<Mutex<Disks>>,
    pub gpu_cache: Arc<Mutex<Option<crate::server::gpu::GpuData>>>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<crate::server::types::BroadcastEvent>,
    pub active_topics: Arc<Mutex<std::collections::HashMap<String, usize>>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub loop_manager: Arc<crate::server::LoopManager>,
}

pub fn get_or_update_gpu_stats(state: &Arc<AppState>) -> Option<crate::server::gpu::GpuData> {
    let cache_seconds = {
        let config = state.config.lock().unwrap();
        if !config.stats.gpu_enabled {
            return None;
        }
        config.stats.disk_cache_seconds // Reusing disk_cache for GPU too
    };

    let mut cache = state.gpu_cache.lock().unwrap();
    if let Some(stats) = &*cache {
        if stats.last_updated.elapsed() < std::time::Duration::from_secs(cache_seconds) {
            return Some(stats.clone());
        }
    }

    // Refresh
    if let Some(stats) = crate::server::gpu::get_gpu_stats() {
        *cache = Some(stats.clone());
        return Some(stats);
    }

    // Return stale cache if refresh failed
    cache.clone()
}

pub async fn status_handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

pub async fn get_client_count(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "count": state.broadcast_tx.receiver_count()
    }))
}

pub async fn get_system_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SystemInfo>, (StatusCode, Json<Value>)> {
    if !state.config.lock().unwrap().features.enable_system {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "System info disabled"})),
        ));
    }

    let mut sys = state.system.lock().unwrap();
    sys.refresh_all();

    let memory = MemoryInfo {
        total: sys.total_memory(),
        slots: crate::server::hardware::get_memory_slots(),
    };

    let os_info = OsInfo {
        name: System::name().unwrap_or("Unknown".into()),
        version: System::os_version().unwrap_or("Unknown".into()),
        build: System::kernel_version().unwrap_or("Unknown".into()),
        arch: std::env::consts::ARCH.to_string(),
    };

    let global = sys.global_cpu_info();
    let mut brand = global.brand().to_string();
    let mut vendor = global.vendor_id().to_string();

    if (brand.is_empty() || brand == "Unknown") && !sys.cpus().is_empty() {
        brand = sys.cpus()[0].brand().to_string();
    }
    if (vendor.is_empty() || vendor == "Unknown") && !sys.cpus().is_empty() {
        vendor = sys.cpus()[0].vendor_id().to_string();
    }

    if vendor == "GenuineIntel" {
        vendor = "Intel".to_string();
    } else if vendor == "AuthenticAMD" {
        vendor = "AMD".to_string();
    }

    let cpu_info = CpuInfo {
        manufacturer: vendor,
        brand: brand,
        cores: sys.cpus().len(),
        physical_cores: sys.physical_core_count().unwrap_or(0),
        base_speed: crate::server::hardware::get_cpu_speed_ghz(),
    };

    // Network
    let mut net_info = None;
    let networks = state.networks.lock().unwrap();
    for (name, data) in networks.iter() {
        // Simple heuristic for primary interface
        if !name.contains("Loopback") && !name.contains("vEthernet") {
            let mut ipv4 = String::new();
            if let Ok(ip) = local_ip_address::local_ip() {
                ipv4 = ip.to_string();
            }
            let ipv6 = String::new();

            net_info = Some(NetworkInfo {
                name: name.clone(),
                mac: data.mac_address().to_string(),
                ipv4,
                ipv6,
            });
            break;
        }
    }

    let disks_lock = state.disks.lock().unwrap();
    let disks = disks_lock
        .iter()
        .map(|d| DiskInfo {
            fs: d
                .mount_point()
                .to_string_lossy()
                .trim_end_matches('\\')
                .into(),
            disk_type: d.file_system().to_string_lossy().into(),
            size: d.total_space(),
            mount: d.mount_point().to_string_lossy().into(),
        })
        .collect();

    let gpu_data = get_or_update_gpu_stats(&state);
    let gpu = gpu_data.map(|g| GpuInfo {
        manufacturer: g.vendor,
        brand: g.model,
        memory_total: g.vram_total_mb,
    });

    Ok(Json(SystemInfo {
        hostname: System::host_name().unwrap_or("Unknown".into()),
        platform: std::env::consts::OS.to_string(),
        os: os_info,
        cpu: cpu_info,
        gpu,
        memory,
        disks,
        network: net_info,
    }))
}

pub async fn get_system_usage(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SystemUsage>, (StatusCode, Json<Value>)> {
    if !state.config.lock().unwrap().features.enable_usage {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Usage data disabled"})),
        ));
    }

    let mut sys = state.system.lock().unwrap();
    sys.refresh_cpu();
    sys.refresh_memory();

    let mut disks_lock = state.disks.lock().unwrap();
    disks_lock.refresh_list();

    let cpu = CpuUsage {
        current_load: sys.global_cpu_info().cpu_usage() as f64,
        current_temp: 0.0,
        current_speed: crate::server::hardware::get_cpu_speed_ghz(),
    };

    let memory = MemoryUsage {
        used: sys.used_memory(),
        free: sys.free_memory(),
        used_percent: (sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0,
    };

    let disks = disks_lock
        .iter()
        .map(|d| DiskUsage {
            fs: d
                .mount_point()
                .to_string_lossy()
                .trim_end_matches('\\')
                .into(),
            used: d.total_space() - d.available_space(),
            available: d.available_space(),
            used_percent: ((d.total_space() - d.available_space()) as f64 / d.total_space() as f64)
                * 100.0,
        })
        .collect();

    let gpu_data = get_or_update_gpu_stats(&state);
    let gpu = gpu_data.map(|g| GpuUsage {
        current_load: g.load_percent,
        current_temp: g.temp_c,
        current_memory: g.vram_used_mb,
    });

    Ok(Json(SystemUsage {
        uptime: System::uptime(),
        cpu,
        memory,
        gpu,
        disks,
    }))
}

#[derive(Deserialize)]
pub struct StreamParams {
    pub fields: Option<String>,
}

pub async fn handle_stream(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
) -> Result<
    Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>>,
    (StatusCode, Json<Value>),
> {
    if !state.config.lock().unwrap().features.enable_stream {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Stream disabled"})),
        ));
    }

    let fields = params.fields.unwrap_or_default();
    let fields_set: std::collections::HashSet<String> = fields
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Subscribe to stats topic and get cleanup handle
    subscribe_topics(&state, &["stats"]);
    let state_for_cleanup = state.clone();

    let stream = async_stream::stream! {
        let mut rx = state.broadcast_tx.subscribe();

        // Use scopeguard to ensure cleanup runs when stream is dropped
        let _cleanup = scopeguard::guard((), |_| {
            unsubscribe_topics(&state_for_cleanup, &["stats"]);
        });

        loop {
            match rx.recv().await {
                Ok(event) => {
                    match event {
                        BroadcastEvent::SystemStats(payload) => {
                            let mut filtered_payload = payload.clone();
                            if !fields_set.is_empty() {
                                if !fields_set.contains("cpu") { filtered_payload.cpu = None; }
                                if !fields_set.contains("memory") { filtered_payload.memory = None; }
                                if !fields_set.contains("gpu") { filtered_payload.gpu = None; }
                                if !fields_set.contains("disks") { filtered_payload.disks = None; }
                                if !fields_set.contains("network") { filtered_payload.network = None; }
                            }

                            yield Ok::<Event, Infallible>(Event::default().json_data(filtered_payload).unwrap());
                        }
                        BroadcastEvent::MediaUpdate(_) |
                        BroadcastEvent::ProcessList(_) |
                        BroadcastEvent::MediaFeedback(_) |
                        BroadcastEvent::ProcessFeedback(_) => {
                            continue;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(
        Sse::new(Box::pin(stream) as Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>)
            .keep_alive(KeepAlive::default()),
    )
}

pub async fn list_processes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ProcessInfo>>, (StatusCode, Json<Value>)> {
    if !state.config.lock().unwrap().features.enable_processes {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Process control disabled"})),
        ));
    }
    Ok(Json(crate::server::process::get_processes_list(&state)))
}

pub async fn get_process_details(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<Vec<ProcessDetail>>, (StatusCode, Json<Value>)> {
    if !state.config.lock().unwrap().features.enable_processes {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Process control disabled"})),
        ));
    }

    let mut sys = state.system.lock().unwrap();
    sys.refresh_processes();

    let mut result = Vec::new();
    let name_lower = name.to_lowercase();

    #[cfg(target_os = "windows")]
    let window_map = unsafe { get_window_map() };

    #[cfg(target_os = "macos")]
    let window_map = crate::server::process::get_macos_window_map();

    for (pid, proc) in sys.processes() {
        if proc.name().to_lowercase().contains(&name_lower) {
            let pid_val = pid.to_string().parse::<u32>().unwrap_or(0);

            #[cfg(target_os = "windows")]
            let (title, has_window) = window_map
                .get(&pid_val)
                .map(|wi| (wi.title.clone(), wi.visible))
                .unwrap_or((None, false));

            #[cfg(target_os = "macos")]
            let (title, has_window) = (None, window_map.contains(&pid_val));

            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            let (title, has_window) = (None, false);

            result.push(ProcessDetail {
                pid: pid_val,
                name: proc.name().to_string(),
                memory: proc.memory(),
                cpu: proc.cpu_usage() as f64,
                title,
                has_window,
            });
        }
    }

    Ok(Json(result))
}

pub async fn launch_process(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LaunchRequest>,
) -> (StatusCode, Json<Value>) {
    if !state.config.lock().unwrap().features.enable_processes {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Process control disabled"})),
        );
    }

    let mut cmd = std::process::Command::new(&payload.path);
    if let Some(args) = payload.args {
        cmd.args(args);
    }

    match cmd.spawn() {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "success"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to launch: {}", e)})),
        ),
    }
}

pub async fn kill_process(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<KillRequest>,
) -> (StatusCode, Json<Value>) {
    if !state.config.lock().unwrap().features.enable_processes {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Process control disabled"})),
        );
    }

    let mut sys = state.system.lock().unwrap();
    sys.refresh_processes();

    let mut killed = false;
    let mut count = 0;

    if let Some(pid) = payload.pid {
        if let Some(process) = sys.process(Pid::from(pid as usize)) {
            if process.kill() {
                killed = true;
                count = 1;
            }
        }
    } else if let Some(name) = payload.name {
        for process in sys.processes().values() {
            if process.name() == name {
                if process.kill() {
                    killed = true;
                    count += 1;
                }
            }
        }
    }

    if killed || count > 0 {
        (
            StatusCode::OK,
            Json(json!({"status": "success", "count": count})),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Process not found or could not be killed"})),
        )
    }
}

pub async fn focus_process(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<FocusRequest>,
) -> (StatusCode, Json<Value>) {
    if !state.config.lock().unwrap().features.enable_processes {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Process control disabled"})),
        );
    }
    #[cfg(target_os = "windows")]
    {
        let pid = payload.pid;
        unsafe {
            let hwnd = find_window_for_pid(pid);
            if hwnd.0 != 0 {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
                let _ = SwitchToThisWindow(hwnd, BOOL(1));
                return (StatusCode::OK, Json(json!({"status": "success"})));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let pid = payload.pid;
        let script = format!(
            "tell application \"System Events\" to set frontmost of the first process whose unix id is {} to true",
            pid
        );

        match std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    return (StatusCode::OK, Json(json!({"status": "success"})));
                } else {
                    // Try to be helpful if it fails
                    let err = String::from_utf8_lossy(&output.stderr);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to focus: {}", err)})),
                    );
                }
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to execute osascript: {}", e)})),
                )
            }
        }
    }

    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "Action not supported on this platform"})),
    )
}

// Power Handler
pub async fn power_action(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(action): axum::extract::Path<String>,
) -> (StatusCode, Json<Value>) {
    let features = state.config.lock().unwrap().features.clone();

    match action.as_str() {
        "shutdown" => {
            if !features.enable_shutdown {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "Shutdown disabled"})),
                );
            }
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::System::Shutdown::{
                    ExitWindowsEx, EWX_SHUTDOWN, EXIT_WINDOWS_FLAGS, SHTDN_REASON_MAJOR_OTHER,
                };
                let _ = ExitWindowsEx(
                    EXIT_WINDOWS_FLAGS(EWX_SHUTDOWN.0 | 0x00000004),
                    SHTDN_REASON_MAJOR_OTHER,
                );
            }
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("osascript")
                    .args(["-e", "tell app \"System Events\" to shut down"])
                    .spawn()
                    .ok();
            }
            (StatusCode::OK, Json(json!({"status": "success"})))
        }
        "restart" => {
            if !features.enable_restart {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "Restart disabled"})),
                );
            }
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::System::Shutdown::{
                    ExitWindowsEx, EWX_REBOOT, EXIT_WINDOWS_FLAGS, SHTDN_REASON_MAJOR_OTHER,
                };
                let _ = ExitWindowsEx(
                    EXIT_WINDOWS_FLAGS(EWX_REBOOT.0 | 0x00000004),
                    SHTDN_REASON_MAJOR_OTHER,
                );
            }
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("osascript")
                    .args(["-e", "tell app \"System Events\" to restart"])
                    .spawn()
                    .ok();
            }
            (StatusCode::OK, Json(json!({"status": "success"})))
        }
        "sleep" => {
            if !features.enable_sleep {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "Sleep disabled"})),
                );
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("rundll32.exe")
                    .args(["powrprof.dll,SetSuspendState", "0", "1", "0"])
                    .spawn()
                    .ok();
            }
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("pmset")
                    .args(["sleepnow"])
                    .spawn()
                    .ok();
            }
            (StatusCode::OK, Json(json!({"status": "success"})))
        }
        "hibernate" => {
            if !features.enable_hibernate {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "Hibernate disabled"})),
                );
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("shutdown")
                    .args(["/h"])
                    .spawn()
                    .ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Same as sleep
                std::process::Command::new("pmset")
                    .args(["sleepnow"])
                    .spawn()
                    .ok();
            }
            (StatusCode::OK, Json(json!({"status": "success"})))
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Unsupported action"})),
        ),
    }
}

// Media Handler

pub async fn get_media_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<MediaStatus>, (StatusCode, Json<Value>)> {
    if !state.config.lock().unwrap().features.enable_media {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Media control disabled"})),
        ));
    }

    if let Some(status) = crate::server::media::get_media_status().await {
        return Ok(Json(status));
    }

    Ok(Json(MediaStatus {
        status: "unknown".to_string(),
        volume: None,
        muted: None,
        playing: None,
        title: None,
        artist: None,
        supports_ctrl: true,
    }))
}

pub async fn media_control(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<MediaControlRequest>,
) -> (StatusCode, Json<Value>) {
    if !state.config.lock().unwrap().features.enable_media {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Media control disabled"})),
        );
    }

    let action = payload.action.as_str();

    // 1. Common Volume/Mute logic
    if action == "set_volume" {
        if let Some(vol) = payload.value {
            if unsafe { crate::server::media::set_volume(vol) }.is_some() {
                return (StatusCode::OK, Json(json!({"status": "success"})));
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to set volume"})),
            );
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Value required for set_volume"})),
        );
    }

    if action == "mute" || action == "unmute" || action == "toggle_mute" {
        #[cfg(target_os = "macos")]
        {
            if action == "toggle_mute" {
                if crate::server::media::run_media_action("toggle_mute")
                    .await
                    .is_some()
                {
                    return (StatusCode::OK, Json(json!({"status": "success"})));
                }
            } else {
                let mute = action == "mute";
                if unsafe { crate::server::media::set_mute(mute) }.is_some() {
                    return (StatusCode::OK, Json(json!({"status": "success"})));
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            if action != "toggle_mute" {
                let mute = action == "mute";
                if unsafe { crate::server::media::set_mute(mute) }.is_some() {
                    return (StatusCode::OK, Json(json!({"status": "success"})));
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Failed to set mute state"})),
                );
            }
        }
    }

    // 2. Playback Control
    #[cfg(target_os = "macos")]
    {
        if crate::server::media::run_media_action(action)
            .await
            .is_some()
        {
            return (StatusCode::OK, Json(json!({"status": "success"})));
        }
    }

    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_MEDIA_NEXT_TRACK,
            VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
        };

        let vk = match action {
            "volume_up" => Some(VK_VOLUME_UP),
            "volume_down" => Some(VK_VOLUME_DOWN),
            "mute" | "toggle_mute" => Some(VK_VOLUME_MUTE),
            "next" => Some(VK_MEDIA_NEXT_TRACK),
            "prev" | "previous" => Some(VK_MEDIA_PREV_TRACK),
            "play" | "pause" | "play_pause" => Some(VK_MEDIA_PLAY_PAUSE),
            _ => None,
        };

        if let Some(key) = vk {
            let inputs = [
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: key,
                            ..Default::default()
                        },
                    },
                },
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: key,
                            dwFlags: KEYEVENTF_KEYUP,
                            ..Default::default()
                        },
                    },
                },
            ];
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
            return (StatusCode::OK, Json(json!({"status": "success"})));
        }
    }

    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": format!("Unsupported action: {}", action)})),
    )
}

// WIN32 Helpers
#[cfg(target_os = "windows")]
struct EnumContext {
    pid: u32,
    hwnd: HWND,
}

#[cfg(target_os = "windows")]
unsafe fn find_window_for_pid(pid: u32) -> HWND {
    let mut context = EnumContext {
        pid,
        hwnd: HWND::default(),
    };
    let lparam = LPARAM(&mut context as *mut EnumContext as isize);
    let _ = EnumWindows(Some(enum_window_proc), lparam);
    context.hwnd
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = &mut *(lparam.0 as *mut EnumContext);
    let mut pid = 0;
    let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == context.pid && IsWindowVisible(hwnd).as_bool() {
        context.hwnd = hwnd;
        return BOOL(0);
    }
    BOOL(1)
}

#[cfg(target_os = "windows")]
struct WindowInfo {
    title: Option<String>,
    visible: bool,
}

#[cfg(target_os = "windows")]
struct WindowMapContext {
    map: std::collections::HashMap<u32, WindowInfo>,
}

#[cfg(target_os = "windows")]
unsafe fn get_window_map() -> std::collections::HashMap<u32, WindowInfo> {
    let mut context = WindowMapContext {
        map: std::collections::HashMap::new(),
    };
    let lparam = LPARAM(&mut context as *mut WindowMapContext as isize);
    let _ = EnumWindows(Some(window_map_enum_proc), lparam);
    context.map
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn window_map_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = &mut *(lparam.0 as *mut WindowMapContext);
    let mut pid = 0;
    let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != 0 && IsWindowVisible(hwnd).as_bool() {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title = if len > 0 {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else {
            None
        };
        let entry = context.map.entry(pid).or_insert(WindowInfo {
            title: None,
            visible: true,
        });
        if entry.title.is_none() && title.is_some() {
            entry.title = title;
        }
    }
    BOOL(1)
}
