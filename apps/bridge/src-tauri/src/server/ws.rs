use crate::server::{
    handlers::{subscribe_topics, unsubscribe_topics, AppState},
    types::{BroadcastEvent, OperationFeedback, WebSocketMessage},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Expand hierarchical topic subscriptions.
/// e.g., "stats" expands to ["stats", "stats.cpu", "stats.memory", "stats.gpu", "stats.disks", "stats.network"]
fn expand_topic(topic: &str) -> Vec<String> {
    match topic {
        "stats" => vec![
            "stats".to_string(),
            "stats.cpu".to_string(),
            "stats.memory".to_string(),
            "stats.gpu".to_string(),
            "stats.disks".to_string(),
            "stats.network".to_string(),
            // Also include legacy aliases for backward compatibility
            "cpu".to_string(),
            "memory".to_string(),
            "gpu".to_string(),
            "disks".to_string(),
            "network".to_string(),
        ],
        // Map legacy topics to new format
        "cpu" => vec!["cpu".to_string(), "stats.cpu".to_string()],
        "memory" => vec!["memory".to_string(), "stats.memory".to_string()],
        "gpu" => vec!["gpu".to_string(), "stats.gpu".to_string()],
        "disks" => vec!["disks".to_string(), "stats.disks".to_string()],
        "network" | "net" => vec!["network".to_string(), "stats.network".to_string()],
        // Other topics stay as-is
        _ => vec![topic.to_string()],
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcast_tx.subscribe();

    // Channel for sending messages from recv_task (errors, acks)
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel::<String>(32);

    // Local subscription state for THIS connection
    // Starts EMPTY (None). User must subscribe to get data.
    let subscriptions: Arc<Mutex<Option<HashSet<String>>>> = Arc::new(Mutex::new(None));

    // SEND TASK - handles both broadcast events and outgoing messages from recv_task
    let mut send_task = tokio::spawn({
        let subs = subscriptions.clone();
        async move {
            loop {
                tokio::select! {
                    // Handle outgoing messages from recv_task (errors, acks)
                    Some(msg) = outgoing_rx.recv() => {
                        if sender.send(Message::Text(msg)).await.is_err() {
                            break;
                        }
                    }
                    // Handle broadcast events
                    result = rx.recv() => {
                        let event = match result {
                            Ok(e) => e,
                            Err(_) => break,
                        };

                        match event {
                            BroadcastEvent::SystemStats(payload) => {
                                let msg_opt = {
                                    let subs_lock = subs.lock().unwrap();
                                    if let Some(topics) = &*subs_lock {
                                        let mut filtered = payload.clone();
                                        let mut has_content = false;

                                        let has_cpu = topics.contains("cpu") || topics.contains("stats.cpu") || topics.contains("stats");
                                        let has_memory = topics.contains("memory") || topics.contains("stats.memory") || topics.contains("stats");
                                        let has_gpu = topics.contains("gpu") || topics.contains("stats.gpu") || topics.contains("stats");
                                        let has_disks = topics.contains("disks") || topics.contains("stats.disks") || topics.contains("stats");
                                        let has_network = topics.contains("network") || topics.contains("stats.network") || topics.contains("stats");

                                        if !has_cpu { filtered.cpu = None; } else { has_content = true; }
                                        if !has_memory { filtered.memory = None; } else { has_content = true; }
                                        if !has_gpu { filtered.gpu = None; } else { has_content = true; }
                                        if !has_disks { filtered.disks = None; } else { has_content = true; }
                                        if !has_network { filtered.network = None; } else { has_content = true; }
                                        if !topics.contains("media") { filtered.media = None; }

                                        if has_content || topics.contains("system") {
                                            Some(BroadcastEvent::SystemStats(filtered))
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                };

                                if let Some(msg) = msg_opt {
                                    if let Ok(text) = serde_json::to_string(&msg) {
                                        if sender.send(Message::Text(text)).await.is_err() { break; }
                                    }
                                }
                            }
                            BroadcastEvent::MediaUpdate(status) => {
                                let should_send = {
                                    let subs_lock = subs.lock().unwrap();
                                    subs_lock.as_ref().map_or(false, |t| t.contains("media") || t.contains("stats.media"))
                                };
                                if should_send {
                                    if let Ok(text) = serde_json::to_string(&BroadcastEvent::MediaUpdate(status)) {
                                        if sender.send(Message::Text(text)).await.is_err() { break; }
                                    }
                                }
                            }
                            BroadcastEvent::ProcessList(payload) => {
                                let should_send = {
                                    let subs_lock = subs.lock().unwrap();
                                    subs_lock.as_ref().map_or(false, |t| t.contains("processes") || t.contains("process"))
                                };
                                if should_send {
                                    if let Ok(text) = serde_json::to_string(&BroadcastEvent::ProcessList(payload)) {
                                        if sender.send(Message::Text(text)).await.is_err() { break; }
                                    }
                                }
                            }
                            BroadcastEvent::MediaFeedback(feedback) => {
                                let should_send = {
                                    let subs_lock = subs.lock().unwrap();
                                    subs_lock.as_ref().map_or(false, |t| t.contains("media") || t.contains("stats.media"))
                                };
                                if should_send {
                                    if let Ok(text) = serde_json::to_string(&BroadcastEvent::MediaFeedback(feedback)) {
                                        if sender.send(Message::Text(text)).await.is_err() { break; }
                                    }
                                }
                            }
                            BroadcastEvent::ProcessFeedback(feedback) => {
                                let should_send = {
                                    let subs_lock = subs.lock().unwrap();
                                    subs_lock.as_ref().map_or(false, |t| t.contains("processes") || t.contains("process"))
                                };
                                if should_send {
                                    if let Ok(text) = serde_json::to_string(&BroadcastEvent::ProcessFeedback(feedback)) {
                                        if sender.send(Message::Text(text)).await.is_err() { break; }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // RECV TASK
    let mut recv_task = tokio::spawn({
        let state = state.clone();
        let subs = subscriptions.clone();
        let tx = outgoing_tx;
        async move {
            while let Some(result) = receiver.next().await {
                match result {
                    Ok(msg) => {
                        if let Message::Text(text) = msg {
                            match serde_json::from_str::<WebSocketMessage>(&text) {
                                Ok(cmd) => {
                                    match cmd {
                                        WebSocketMessage::Subscribe(req) => {
                                            // Get old subscriptions
                                            let old_subs = {
                                                let lock = subs.lock().unwrap();
                                                lock.clone()
                                            };

                                            // Expand hierarchical topics
                                            let mut new_set = HashSet::new();
                                            for t in req.topics {
                                                let expanded = expand_topic(&t.to_lowercase());
                                                for et in expanded {
                                                    new_set.insert(et);
                                                }
                                            }

                                            // Unsubscribe from old topics
                                            if let Some(old_set) = &old_subs {
                                                let old_refs: Vec<&str> =
                                                    old_set.iter().map(|s| s.as_str()).collect();
                                                unsubscribe_topics(&state, &old_refs);
                                            }

                                            // Subscribe to new topics
                                            let new_refs: Vec<&str> =
                                                new_set.iter().map(|s| s.as_str()).collect();
                                            subscribe_topics(&state, &new_refs);

                                            // Update local subscription state
                                            {
                                                let mut lock = subs.lock().unwrap();
                                                *lock = Some(new_set);
                                            }
                                        }
                                        ref other => {
                                            // Handle command and get feedback
                                            let feedback =
                                                handle_ws_command(other.clone(), &state).await;

                                            // Broadcast feedback to all relevant subscribers
                                            if let Some(fb) = feedback {
                                                let _ = state.broadcast_tx.send(fb);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Send error back to client via outgoing channel
                                    let error_msg = serde_json::json!({
                                        "type": "error",
                                        "data": {
                                            "code": "PARSE_ERROR",
                                            "message": format!("Invalid message format: {}", e)
                                        }
                                    });
                                    if let Ok(text) = serde_json::to_string(&error_msg) {
                                        let _ = tx.send(text).await;
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
            }

            // CLEANUP ON DISCONNECT - take() ensures we only cleanup once
            let old_subs = subs.lock().unwrap().take();
            if let Some(old_set) = old_subs {
                let old_refs: Vec<&str> = old_set.iter().map(|s| s.as_str()).collect();
                unsubscribe_topics(&state, &old_refs);
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

    // Cleanup after disconnect (handles case where recv_task was aborted)
    // Get the final subscriptions and properly decrement counts + stop loops
    let final_subs = subscriptions.lock().unwrap().take(); // take() to avoid double cleanup

    if let Some(old_set) = final_subs {
        let topic_refs: Vec<&str> = old_set.iter().map(|s| s.as_str()).collect();
        crate::server::handlers::unsubscribe_topics(&state, &topic_refs);
    }
}

async fn handle_ws_command(cmd: WebSocketMessage, state: &Arc<AppState>) -> Option<BroadcastEvent> {
    match cmd {
        WebSocketMessage::Media(req) => {
            if !state.config.lock().unwrap().features.enable_media {
                return Some(BroadcastEvent::MediaFeedback(OperationFeedback {
                    success: false,
                    action: req.action.clone(),
                    message: Some("Media control disabled".to_string()),
                    pid: None,
                    name: None,
                }));
            }

            let action = req.action.as_str();
            let mut success = true;
            let mut error_msg: Option<String> = None;

            if action == "set_volume" {
                if let Some(vol) = req.value {
                    if unsafe { crate::server::media::set_volume(vol) }.is_none() {
                        success = false;
                        error_msg = Some("Failed to set volume".to_string());
                    }
                } else {
                    success = false;
                    error_msg = Some("Value required for set_volume".to_string());
                }
            } else if action == "mute" || action == "unmute" || action == "toggle_mute" {
                #[cfg(target_os = "macos")]
                {
                    if action == "toggle_mute" {
                        crate::server::media::run_media_action("toggle_mute").await;
                    } else {
                        unsafe { crate::server::media::set_mute(action == "mute") };
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    if action != "toggle_mute" {
                        unsafe { crate::server::media::set_mute(action == "mute") };
                    } else {
                        trigger_windows_media_key("mute");
                    }
                }
            } else {
                #[cfg(target_os = "macos")]
                {
                    crate::server::media::run_media_action(action).await;
                }
                #[cfg(target_os = "windows")]
                {
                    trigger_windows_media_key(action);
                }
            }

            Some(BroadcastEvent::MediaFeedback(OperationFeedback {
                success,
                action: req.action,
                message: error_msg,
                pid: None,
                name: None,
            }))
        }
        WebSocketMessage::ProcessKill(req) => {
            if !state.config.lock().unwrap().features.enable_processes {
                return Some(BroadcastEvent::ProcessFeedback(OperationFeedback {
                    success: false,
                    action: "kill".to_string(),
                    message: Some("Process control disabled".to_string()),
                    pid: req.pid,
                    name: req.name,
                }));
            }

            let mut sys = state.system.lock().unwrap();
            sys.refresh_processes();

            let mut success = false;
            let mut killed_name: Option<String> = None;

            if let Some(pid) = req.pid {
                if let Some(process) = sys.process(sysinfo::Pid::from(pid as usize)) {
                    killed_name = Some(process.name().to_string());
                    success = process.kill();
                }
            } else if let Some(ref name) = req.name {
                for process in sys.processes().values() {
                    if process.name() == name {
                        if process.kill() {
                            success = true;
                            killed_name = Some(name.clone());
                            break;
                        }
                    }
                }
            }

            Some(BroadcastEvent::ProcessFeedback(OperationFeedback {
                success,
                action: "kill".to_string(),
                message: if success {
                    None
                } else {
                    Some("Process not found or could not be killed".to_string())
                },
                pid: req.pid,
                name: killed_name.or(req.name),
            }))
        }
        WebSocketMessage::ProcessLaunch(req) => {
            if !state.config.lock().unwrap().features.enable_processes {
                return Some(BroadcastEvent::ProcessFeedback(OperationFeedback {
                    success: false,
                    action: "launch".to_string(),
                    message: Some("Process control disabled".to_string()),
                    pid: None,
                    name: Some(req.path),
                }));
            }

            let mut cmd = std::process::Command::new(&req.path);
            if let Some(args) = req.args {
                cmd.args(args);
            }

            let result = cmd.spawn();
            let success = result.is_ok();
            let message = result.err().map(|e| e.to_string());

            Some(BroadcastEvent::ProcessFeedback(OperationFeedback {
                success,
                action: "launch".to_string(),
                message,
                pid: None,
                name: Some(req.path),
            }))
        }
        WebSocketMessage::ProcessFocus(_) => {
            // Focus doesn't generate feedback (it's handled elsewhere)
            None
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn trigger_windows_media_key(action: &str) {
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
        }
    }
}
