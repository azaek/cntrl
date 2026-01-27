#[cfg(target_os = "windows")]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
#[cfg(target_os = "windows")]
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
#[cfg(target_os = "windows")]
use windows::Win32::Media::Audio::{
    eConsole, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

use crate::server::types::MediaStatus;

#[cfg(target_os = "windows")]
unsafe fn get_volume_info() -> Option<(i32, bool)> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let volume_endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
    let vol_scalar = volume_endpoint.GetMasterVolumeLevelScalar().ok()?;
    let muted = volume_endpoint.GetMute().ok()?.as_bool();
    Some(((vol_scalar * 100.0) as i32, muted))
}

pub async fn get_media_status() -> Option<MediaStatus> {
    #[cfg(target_os = "windows")]
    {
        let (volume, muted) = unsafe { get_volume_info() }.unzip();
        let mut status = "stopped".to_string();
        let mut playing = Some(false);
        let mut title = None;
        let mut artist = None;

        if let Ok(manager_res) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            if let Ok(manager) = manager_res.await {
                if let Ok(session) = manager.GetCurrentSession() {
                    if let Ok(info) = session.GetPlaybackInfo() {
                        if let Ok(s) = info.PlaybackStatus() {
                            status = match s {
                                GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => "playing".to_string(),
                                GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => "paused".to_string(),
                                _ => "stopped".to_string(),
                            };
                        }
                    }
                    playing = Some(status == "playing");
                    if let Ok(props_res) = session.TryGetMediaPropertiesAsync() {
                        if let Ok(props) = props_res.await {
                            if let Ok(t) = props.Title() {
                                title = Some(t.to_string());
                            }
                            if let Ok(a) = props.Artist() {
                                artist = Some(a.to_string());
                            }
                        }
                    }
                }
            }
        }

        return Some(MediaStatus {
            status,
            volume,
            muted,
            playing,
            title: if title.as_deref() == Some("") {
                None
            } else {
                title
            },
            artist: if artist.as_deref() == Some("") {
                None
            } else {
                artist
            },
            supports_ctrl: true,
        });
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // 1. Get Volume info
        let vol_script = "get volume settings";
        let vol_out = Command::new("osascript")
            .args(["-e", vol_script])
            .output()
            .ok();
        let mut volume = None;
        let mut muted = None;

        if let Some(out) = vol_out {
            let s = String::from_utf8_lossy(&out.stdout);
            // output volume:50, input volume:50, alert volume:50, output muted:false
            for part in s.split(',') {
                if part.contains("output volume:") {
                    if let Ok(v) = part.split(':').last().unwrap_or("").trim().parse::<i32>() {
                        volume = Some(v);
                    }
                }
                if part.contains("output muted:") {
                    muted = Some(part.contains("true"));
                }
            }
        }

        // 2. Get Media player info
        let media_script = r#"
            tell application "System Events"
                set spotifyRunning to (name of processes) contains "Spotify"
                set musicRunning to (name of processes) contains "Music"
            end tell

            if spotifyRunning then
                tell application "Spotify"
                    return "Spotify" & "||" & (player state as string) & "||" & (name of current track) & "||" & (artist of current track)
                end tell
            else if musicRunning then
                tell application "Music"
                    return "Music" & "||" & (player state as string) & "||" & (name of current track) & "||" & (artist of current track)
                end tell
            end if
            return "None"
        "#;

        let media_out = Command::new("osascript")
            .args(["-e", media_script])
            .output()
            .ok();
        let mut status = "stopped".to_string();
        let mut title = None;
        let mut artist = None;
        let mut playing = Some(false);

        if let Some(out) = media_out {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if res != "None" && !res.is_empty() {
                let parts: Vec<&str> = res.split("||").collect();
                if parts.len() >= 4 {
                    status = parts[1].to_lowercase(); // playing/paused
                    playing = Some(status == "playing");
                    title = Some(parts[2].to_string());
                    artist = Some(parts[3].to_string());
                }
            }
        }

        return Some(MediaStatus {
            status,
            volume,
            muted,
            playing,
            title,
            artist,
            supports_ctrl: true,
        });
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    None
}

#[allow(dead_code)]
pub fn get_media_status_sync() -> Option<MediaStatus> {
    #[cfg(target_os = "windows")]
    {
        let (volume, muted) = unsafe { get_volume_info() }.unzip();
        let mut status = "stopped".to_string();
        let mut playing = Some(false);
        let mut title = None;
        let mut artist = None;

        // Synchronous equivalent using .get().ok()
        if let Ok(manager) =
            GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get())
        {
            if let Ok(session) = manager.GetCurrentSession() {
                if let Ok(info) = session.GetPlaybackInfo() {
                    if let Ok(s) = info.PlaybackStatus() {
                        status = match s {
                            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => {
                                "playing".to_string()
                            }
                            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => {
                                "paused".to_string()
                            }
                            _ => "stopped".to_string(),
                        };
                    }
                }
                playing = Some(status == "playing");

                if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
                    if let Ok(t) = props.Title() {
                        title = Some(t.to_string());
                    }
                    if let Ok(a) = props.Artist() {
                        artist = Some(a.to_string());
                    }
                }
            }
        }

        return Some(MediaStatus {
            status,
            volume,
            muted,
            playing,
            title: if title.as_deref() == Some("") {
                None
            } else {
                title
            },
            artist: if artist.as_deref() == Some("") {
                None
            } else {
                artist
            },
            supports_ctrl: true,
        });
    }

    // Reuse async implementation for MacOS since it uses Command (which is sync-ish anyway)
    // Actually our macos impl uses std::process::Command which IS synchronous.
    // The "async" wrapper was just for API consistency.
    #[cfg(target_os = "macos")]
    {
        // We can't call the async function from sync easily without a runtime,
        // but the macos impl logic is completely synchronous (std::process::Command).
        // So we can just copy-paste or extract the logic.
        // For brevity, let's just duplicate the logic since it's short.

        use std::process::Command;

        // 1. Get Volume info
        let vol_script = "get volume settings";
        let vol_out = Command::new("osascript")
            .args(["-e", vol_script])
            .output()
            .ok();
        let mut volume = None;
        let mut muted = None;

        if let Some(out) = vol_out {
            let s = String::from_utf8_lossy(&out.stdout);
            for part in s.split(',') {
                if part.contains("output volume:") {
                    if let Ok(v) = part.split(':').last().unwrap_or("").trim().parse::<i32>() {
                        volume = Some(v);
                    }
                }
                if part.contains("output muted:") {
                    muted = Some(part.contains("true"));
                }
            }
        }

        // 2. Get Media player info
        let media_script = r#"
            tell application "System Events"
                set spotifyRunning to (name of processes) contains "Spotify"
                set musicRunning to (name of processes) contains "Music"
            end tell
            if spotifyRunning then
                tell application "Spotify"
                    return "Spotify" & "||" & (player state as string) & "||" & (name of current track) & "||" & (artist of current track)
                end tell
            else if musicRunning then
                tell application "Music"
                    return "Music" & "||" & (player state as string) & "||" & (name of current track) & "||" & (artist of current track)
                end tell
            end if
            return "None"
        "#;

        let media_out = Command::new("osascript")
            .args(["-e", media_script])
            .output()
            .ok();
        let mut status = "stopped".to_string();
        let mut title = None;
        let mut artist = None;
        let mut playing = Some(false);

        if let Some(out) = media_out {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if res != "None" && !res.is_empty() {
                let parts: Vec<&str> = res.split("||").collect();
                if parts.len() >= 4 {
                    status = parts[1].to_lowercase();
                    playing = Some(status == "playing");
                    title = Some(parts[2].to_string());
                    artist = Some(parts[3].to_string());
                }
            }
        }

        Some(MediaStatus {
            status,
            volume,
            muted,
            playing,
            title,
            artist,
            supports_ctrl: true,
        })
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    None
}

#[cfg(target_os = "macos")]
pub async fn run_media_action(action: &str) -> Option<()> {
    use std::process::Command;
    let script = match action {
        "play" | "pause" | "play_pause" | "toggle_mute" => {
            r#"
            tell application "System Events"
                set spotifyRunning to (name of processes) contains "Spotify"
                set musicRunning to (name of processes) contains "Music"
            end tell
            if spotifyRunning then
                tell application "Spotify" to playpause
            else if musicRunning then
                tell application "Music" to playpause
            end if
            "#
        }
        "next" => {
            r#"
            tell application "System Events"
                set spotifyRunning to (name of processes) contains "Spotify"
                set musicRunning to (name of processes) contains "Music"
            end tell
            if spotifyRunning then
                tell application "Spotify" to next track
            else if musicRunning then
                tell application "Music" to next track
            end if
            "#
        }
        "prev" | "previous" => {
            r#"
            tell application "System Events"
                set spotifyRunning to (name of processes) contains "Spotify"
                set musicRunning to (name of processes) contains "Music"
            end tell
            if spotifyRunning then
                tell application "Spotify" to previous track
            else if musicRunning then
                tell application "Music" to previous track
            end if
            "#
        }
        "volume_up" => "set volume output volume ((output volume of (get volume settings)) + 5)",
        "volume_down" => "set volume output volume ((output volume of (get volume settings)) - 5)",
        _ => return None,
    };

    Command::new("osascript")
        .args(["-e", script])
        .status()
        .ok()?;
    Some(())
}

#[cfg(target_os = "windows")]
pub unsafe fn set_volume(volume: i32) -> Option<()> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let volume_endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
    let vol_scalar = (volume as f32) / 100.0;
    volume_endpoint
        .SetMasterVolumeLevelScalar(vol_scalar, std::ptr::null())
        .ok()?;
    Some(())
}

#[cfg(target_os = "macos")]
pub unsafe fn set_volume(volume: i32) -> Option<()> {
    let script = format!("set volume output volume {}", volume);
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .status()
        .ok()?;
    Some(())
}

#[cfg(target_os = "windows")]
pub unsafe fn set_mute(mute: bool) -> Option<()> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let volume_endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
    volume_endpoint.SetMute(mute, std::ptr::null()).ok()?;
    Some(())
}

#[cfg(target_os = "macos")]
pub unsafe fn set_mute(mute: bool) -> Option<()> {
    let script = format!(
        "set volume output muted {}",
        if mute { "true" } else { "false" }
    );
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .status()
        .ok()?;
    Some(())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub unsafe fn set_volume(_volume: i32) -> Option<()> {
    None
}
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub unsafe fn set_mute(_mute: bool) -> Option<()> {
    None
}
