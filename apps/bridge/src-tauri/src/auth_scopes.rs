use crate::auth_store::AuthMode;
use crate::server::types::WebSocketMessage;

pub const SCOPE_ADMIN: &str = "admin";
pub const SCOPE_SYSTEM_READ: &str = "system:read";
pub const SCOPE_USAGE_READ: &str = "usage:read";
pub const SCOPE_STATS_READ: &str = "stats:read";
pub const SCOPE_MEDIA_READ: &str = "media:read";
pub const SCOPE_MEDIA_CONTROL: &str = "media:control";
pub const SCOPE_PROCESSES_READ: &str = "processes:read";
pub const SCOPE_PROCESSES_CONTROL: &str = "processes:control";
pub const SCOPE_POWER_CONTROL: &str = "power:control";
pub const SCOPE_STREAM_READ: &str = "stream:read";
pub const SCOPE_WS_CONNECT: &str = "ws:connect";

#[derive(Clone, Debug)]
pub struct AuthContext {
    pub mode: AuthMode,
    pub scopes: Vec<String>,
}

impl AuthContext {
    pub fn is_public(&self) -> bool {
        matches!(self.mode, AuthMode::Public)
    }

    pub fn has_scope(&self, required: &str) -> bool {
        self.scopes.iter().any(|s| s == SCOPE_ADMIN || s == required)
    }
}

pub fn required_scope_for_request(method: &str, path: &str) -> Option<&'static str> {
    match (method, path) {
        // /api/status is public (outside auth middleware)
        ("GET", "/api/system") => Some(SCOPE_SYSTEM_READ),
        ("GET", "/api/usage") => Some(SCOPE_USAGE_READ),
        ("GET", "/api/clients") => Some(SCOPE_ADMIN),
        ("GET", "/api/media/status") => Some(SCOPE_MEDIA_READ),
        ("POST", "/api/media/control") => Some(SCOPE_MEDIA_CONTROL),
        ("GET", "/api/stream") => Some(SCOPE_STREAM_READ),
        ("GET", "/api/ws") => Some(SCOPE_WS_CONNECT),
        ("POST", "/api/processes/kill") => Some(SCOPE_PROCESSES_CONTROL),
        ("POST", "/api/processes/focus") => Some(SCOPE_PROCESSES_CONTROL),
        ("POST", "/api/processes/launch") => Some(SCOPE_PROCESSES_CONTROL),
        ("POST", "/api/pw/shutdown") => Some(SCOPE_POWER_CONTROL),
        ("POST", "/api/pw/restart") => Some(SCOPE_POWER_CONTROL),
        ("POST", "/api/pw/sleep") => Some(SCOPE_POWER_CONTROL),
        ("POST", "/api/pw/hibernate") => Some(SCOPE_POWER_CONTROL),
        ("GET", "/api/processes") => Some(SCOPE_PROCESSES_READ),
        _ => {
            if path.starts_with("/api/processes/") && method == "GET" {
                return Some(SCOPE_PROCESSES_READ);
            }
            if path.starts_with("/api/") {
                return Some(SCOPE_ADMIN);
            }
            None
        }
    }
}

pub fn required_scopes_for_topics(topics: &[String]) -> Vec<&'static str> {
    let mut scopes: Vec<&'static str> = Vec::new();

    for topic in topics {
        let t = topic.to_lowercase();
        let required = match t.as_str() {
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
            | "net"
            | "system" => Some(SCOPE_STATS_READ),
            "media" | "stats.media" => Some(SCOPE_MEDIA_READ),
            "processes" | "process" => Some(SCOPE_PROCESSES_READ),
            _ => None,
        };

        if let Some(scope) = required {
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
    }

    scopes
}

pub fn required_scope_for_ws_message(msg: &WebSocketMessage) -> Option<&'static str> {
    match msg {
        WebSocketMessage::Media(_) => Some(SCOPE_MEDIA_CONTROL),
        WebSocketMessage::ProcessKill(_) => Some(SCOPE_PROCESSES_CONTROL),
        WebSocketMessage::ProcessLaunch(_) => Some(SCOPE_PROCESSES_CONTROL),
        WebSocketMessage::ProcessFocus(_) => Some(SCOPE_PROCESSES_CONTROL),
        WebSocketMessage::Subscribe(_) => None,
    }
}
