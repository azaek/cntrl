use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct SystemInfo {
    pub hostname: String,
    pub platform: String,
    pub os: OsInfo,
    pub cpu: CpuInfo,
    pub gpu: Option<GpuInfo>,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub network: Option<NetworkInfo>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub build: String,
    pub arch: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct CpuInfo {
    pub manufacturer: String,
    pub brand: String,
    pub cores: usize,
    pub physical_cores: usize,
    pub base_speed: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct GpuInfo {
    pub manufacturer: String,
    pub brand: String,
    pub memory_total: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct MemoryInfo {
    pub total: u64,
    pub slots: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct DiskInfo {
    pub fs: String, // e.g. "C:"
    #[serde(rename = "type")]
    pub disk_type: String, // e.g. "NTFS"
    pub size: u64,
    pub mount: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct NetworkInfo {
    pub name: String,
    pub mac: String,
    pub ipv4: String,
    pub ipv6: String,
}

// Dynamic Usage Types

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct SystemUsage {
    pub uptime: u64,
    pub cpu: CpuUsage,
    pub memory: MemoryUsage,
    pub gpu: Option<GpuUsage>,
    pub disks: Vec<DiskUsage>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct CpuUsage {
    pub current_load: f64,
    pub current_temp: f64,
    pub current_speed: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct MemoryUsage {
    pub used: u64,
    pub free: u64,
    pub used_percent: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct GpuUsage {
    pub current_load: f64,
    pub current_temp: f64,
    pub current_memory: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct DiskUsage {
    pub fs: String,
    pub used: u64,
    pub available: u64,
    pub used_percent: f64,
}

// Stream Payload (Nullable fields)
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct StreamPayload {
    pub timestamp: i64,
    pub uptime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disks: Option<Vec<DiskUsage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkUsage>, // We need to define NetworkUsage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaStatus>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct NetworkUsage {
    pub bytes_sent: u64,
    pub bytes_recv: u64,
}

// Process Types

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ProcessInfo {
    pub name: String,
    pub count: usize,
    pub memory: u64,
    pub memory_mb: f64,
    pub cpu_time: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ProcessDetail {
    pub pid: u32,
    pub name: String,
    pub memory: u64,
    pub cpu: f64,
    pub title: Option<String>,
    pub has_window: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct KillRequest {
    pub pid: Option<u32>,
    pub name: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FocusRequest {
    pub pid: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LaunchRequest {
    pub path: String,
    pub args: Option<Vec<String>>,
}

// Media Types

#[derive(Deserialize, Debug, Clone)]
pub struct MediaControlRequest {
    pub action: String,
    pub value: Option<i32>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct MediaStatus {
    pub status: String,
    pub volume: Option<i32>,
    pub muted: Option<bool>,
    pub playing: Option<bool>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub supports_ctrl: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "status", content = "message")]
pub enum ServerStatus {
    Starting,
    Running,
    Stopped,
    Error(String),
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ServerState {
    pub status: ServerStatus,
    pub port: u16,
}

// Process list payload for WebSocket broadcasting
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ProcessListPayload {
    pub timestamp: i64,
    pub processes: Vec<ProcessInfo>,
    pub total_count: usize,
}

// Generic operation feedback for WebSocket broadcasting
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct OperationFeedback {
    pub success: bool,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// Broadcast Types
#[derive(Clone, Serialize, Debug)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum BroadcastEvent {
    SystemStats(StreamPayload),
    MediaUpdate(MediaStatus),
    ProcessList(ProcessListPayload),
    MediaFeedback(OperationFeedback),
    ProcessFeedback(OperationFeedback),
}

#[derive(Clone, Deserialize, Debug)]
#[serde(tag = "op", content = "data", rename_all = "snake_case")]
pub enum WebSocketMessage {
    Media(MediaControlRequest),
    ProcessKill(KillRequest),
    ProcessFocus(FocusRequest),
    ProcessLaunch(LaunchRequest),
    Subscribe(SubscribeRequest),
}

#[derive(Deserialize, Debug, Clone)]
pub struct SubscribeRequest {
    pub topics: Vec<String>,
}
