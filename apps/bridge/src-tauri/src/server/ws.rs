use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    response::Response,
};
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use futures::{sink::SinkExt, stream::StreamExt};
use crate::server::{handlers::AppState, types::{WebSocketMessage, BroadcastEvent, OperationFeedback}};

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

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcast_tx.subscribe();

    // Local subscription state for THIS connection
    // Starts EMPTY (None). User must subscribe to get data.
    let subscriptions: Arc<Mutex<Option<HashSet<String>>>> = Arc::new(Mutex::new(None));

    // Helper to update global ref-counts and manage loop lifecycle
    let state_for_ops = state.clone();
    let update_global_counts = move |old: &Option<HashSet<String>>, new: &Option<HashSet<String>>| {
        // Track which topics went from 0->1 and 1->0
        let mut topics_to_start: Vec<String> = Vec::new();
        let mut topics_to_stop: Vec<String> = Vec::new();

        {
            let mut global_map = state_for_ops.active_topics.lock().unwrap();

            // Decrement old and track 1->0 transitions
            if let Some(old_set) = old {
                for topic in old_set {
                    if let Some(count) = global_map.get_mut(topic) {
                        if *count > 0 {
                            *count -= 1;
                            if *count == 0 {
                                topics_to_stop.push(topic.clone());
                            }
                        }
                    }
                }
            }

            // Increment new and track 0->1 transitions
            if let Some(new_set) = new {
                for topic in new_set {
                    let entry = global_map.entry(topic.clone()).or_insert(0);
                    let was_zero = *entry == 0;
                    *entry += 1;
                    if was_zero {
                        topics_to_start.push(topic.clone());
                    }
                }
            }
        }

        // Start loops for topics that went 0->1
        for topic in topics_to_start {
            state_for_ops.loop_manager.ensure_loop_running(&topic, state_for_ops.clone());
        }

        // Stop loops for topics that went 1->0
        for topic in topics_to_stop {
            state_for_ops.loop_manager.stop_loop_if_idle(&topic, &state_for_ops);
        }
    };

    // Initialize with NO subscriptions (count doesn't change yet)
    // Actually, decision: Default = ALL or Default = NONE?
    // User requested "Demand-Based". Ideally Default = NONE.
    // But for backward compat with my code 5 mins ago, I had Default=ALL.
    // Let's implement Default=NONE (Explicit Subscribe).
    // So on connect, we do nothing to global counts.
    
    // SEND TASK
    let mut send_task = tokio::spawn({
        let subs = subscriptions.clone();
        async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    BroadcastEvent::SystemStats(payload) => {
                        let msg_opt = {
                            let subs_lock = subs.lock().unwrap();
                            if let Some(topics) = &*subs_lock {
                                // Filter based on topics (check both legacy and new format)
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
                                // Media is separate event usually
                                if !topics.contains("media") { filtered.media = None; }

                                if has_content || topics.contains("system") {
                                    Some(BroadcastEvent::SystemStats(filtered))
                                } else {
                                    None
                                }
                            } else {
                                // None = Initial state = No data. User must send subscribe.
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
                            if let Some(topics) = &*subs_lock {
                                topics.contains("media") || topics.contains("stats.media")
                            } else {
                                false
                            }
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
                            if let Some(topics) = &*subs_lock {
                                topics.contains("processes") || topics.contains("process")
                            } else {
                                false
                            }
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
                            if let Some(topics) = &*subs_lock {
                                topics.contains("media") || topics.contains("stats.media")
                            } else {
                                false
                            }
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
                            if let Some(topics) = &*subs_lock {
                                topics.contains("processes") || topics.contains("process")
                            } else {
                                false
                            }
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
    });

    // RECV TASK
    let mut recv_task = tokio::spawn({
        let state = state.clone();
        let subs = subscriptions.clone();
        async move {
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(cmd) = serde_json::from_str::<WebSocketMessage>(&text) {
                        match cmd {
                            WebSocketMessage::Subscribe(req) => {
                                let mut lock = subs.lock().unwrap();
                                let old_subs = lock.clone();

                                // Expand hierarchical topics
                                let mut new_set = HashSet::new();
                                for t in req.topics {
                                    let expanded = expand_topic(&t.to_lowercase());
                                    for et in expanded {
                                        new_set.insert(et);
                                    }
                                }
                                let new_subs = Some(new_set);

                                update_global_counts(&old_subs, &new_subs);
                                *lock = new_subs;
                            }
                            ref other => {
                                // Handle command and get feedback
                                let feedback = handle_ws_command(other.clone(), &state).await;

                                // Broadcast feedback to all relevant subscribers
                                if let Some(fb) = feedback {
                                    let _ = state.broadcast_tx.send(fb);
                                }
                            }
                        }
                    }
                }
            }

            // CLEANUP ON DISCONNECT
            // When loop ends (websocket closed)
            let lock = subs.lock().unwrap();
            update_global_counts(&*lock, &None);
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
    
    // Ensure we run cleanup if select unwinds? 
    // recv_task does it at end of loop. 
    // If send_task fails, recv_task is aborted.
    // If recv_task is aborted, the cleanup code at the end might NOT run!
    // We need a Drop guard or ensure cleanup runs.
    // The easiest way in async rust without Drop guards is strictly managing the lifecycle.
    // Or we rely on the fact that if `recv_task` is aborted, we can't easily run cleanup code unless we use a destructor.
    // Actually, `active_subs` is shared (Arc Mutex). We can do cleanup in `handle_socket` after the select!
    
    // Cleanup Logic (Post-Disconnect)
    // We need to know what the final subscriptions were.
    let final_subs = subscriptions.lock().unwrap().clone();
    
    // Inline cleanup to avoid closure move issues
    if let Some(old_set) = final_subs {
        let mut global_map = state.active_topics.lock().unwrap();
        for topic in old_set {
            if let Some(count) = global_map.get_mut(&topic) {
                if *count > 0 { *count -= 1; }
            }
        }
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
                message: if success { None } else { Some("Process not found or could not be killed".to_string()) },
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
            SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, VK_VOLUME_MUTE, VK_VOLUME_DOWN, 
            VK_VOLUME_UP, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PREV_TRACK, VK_MEDIA_PLAY_PAUSE, 
            KEYEVENTF_KEYUP
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
