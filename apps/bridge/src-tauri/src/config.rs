use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AppConfig {
    pub server: ServerConfig,
    pub display: DisplayConfig,
    pub features: FeaturesConfig,
    pub stats: StatsConfig,
    pub auth: AuthConfig,
    #[serde(default)]
    pub websocket: WebSocketConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct DisplayConfig {
    pub hostname: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct FeaturesConfig {
    pub enable_shutdown: bool,
    pub enable_restart: bool,
    pub enable_hibernate: bool,
    pub enable_sleep: bool,
    pub enable_system: bool,
    pub enable_usage: bool,
    pub enable_stats: bool,
    pub enable_media: bool,
    pub enable_processes: bool,
    pub enable_stream: bool,
    pub enable_autostart: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct StatsConfig {
    pub gpu_enabled: bool,
    pub disk_cache_seconds: u64,
    pub stream_interval_seconds: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AuthConfig {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub blocked_ips: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct WebSocketConfig {
    pub stats: TopicConfig,
    pub media: TopicConfig,
    pub processes: TopicConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TopicConfig {
    pub enabled: bool,
    pub interval_ms: u64,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            stats: TopicConfig {
                enabled: true,
                interval_ms: 1000,
            },
            media: TopicConfig {
                enabled: true,
                interval_ms: 500,
            },
            processes: TopicConfig {
                enabled: true,
                interval_ms: 3000,
            },
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                port: 9990,
                host: "0.0.0.0".to_string(),
            },
            display: DisplayConfig {
                hostname: "".to_string(),
            },
            features: FeaturesConfig {
                enable_shutdown: false,
                enable_restart: false,
                enable_hibernate: true,
                enable_sleep: true,
                enable_system: true,
                enable_usage: true,
                enable_stats: true,
                enable_media: true,
                enable_processes: true,
                enable_stream: true,
                enable_autostart: true,
            },
            stats: StatsConfig {
                gpu_enabled: true,
                disk_cache_seconds: 30,
                stream_interval_seconds: 2,
            },
            auth: AuthConfig {
                enabled: false,
                api_key: None,
                allowed_ips: vec![],
                blocked_ips: vec![],
            },
            websocket: WebSocketConfig::default(),
        }
    }
}

pub fn load_config(app: &AppHandle) -> AppConfig {
    let config_path = get_config_path(app);
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                // Re-save to add any new fields with defaults (e.g., websocket)
                save_config(app, &config);
                return config;
            }
        }
    }

    // Save default if not exists
    let default_config = AppConfig::default();
    save_config(app, &default_config);
    default_config
}

pub fn save_config(app: &AppHandle, config: &AppConfig) {
    let config_path = get_config_path(app);
    if let Some(parent) = config_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(config) {
        let _ = fs::write(config_path, content);
    }
}

pub fn get_config_path(app: &AppHandle) -> PathBuf {
    app.path().app_config_dir().unwrap().join("config.json")
}
