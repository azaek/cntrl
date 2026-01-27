#[cfg(target_os = "macos")]
use cocoa::appkit::{NSView, NSWindow, NSWindowStyleMask};
#[cfg(target_os = "macos")]
use cocoa::base::{id, YES};
#[cfg(target_os = "macos")]
use cocoa::foundation::NSPoint;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

#[cfg(target_os = "macos")]
use tauri::{Runtime, WebviewWindow};

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn enable_rounded_corners<R: Runtime>(window: WebviewWindow<R>) -> Result<(), String> {
    unsafe {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as id;

        // Get the content view
        let content_view: id = msg_send![ns_window, contentView];

        // Make the view layer-backed
        let _: () = msg_send![content_view, setWantsLayer: YES];

        // Get the layer
        let layer: id = msg_send![content_view, layer];

        // Set corner radius (13.0 is standard macOS corner radius)
        let corner_radius: f64 = 13.0;
        let _: () = msg_send![layer, setCornerRadius: corner_radius];

        // Clip to bounds so content respects the corner radius
        let _: () = msg_send![layer, setMasksToBounds: YES];

        // Enable shadow on the window
        let _: () = msg_send![ns_window, setHasShadow: YES];

        // Make window non-opaque
        let _: () = msg_send![ns_window, setOpaque: cocoa::base::NO];

        // Set background color to clear
        let ns_color: id = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![ns_window, setBackgroundColor: ns_color];
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn enable_modern_window_style<R: Runtime>(window: WebviewWindow<R>) -> Result<(), String> {
    unsafe {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as id;

        // Set titlebar appearance to transparent
        let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: true];

        // Hide title
        let _: () = msg_send![ns_window, setTitleVisibility: 1]; // NSWindowTitleHidden = 1

        // Enable full size content view
        let current_style: NSWindowStyleMask = msg_send![ns_window, styleMask];
        let new_style = current_style | NSWindowStyleMask::NSFullSizeContentViewWindowMask;
        let _: () = msg_send![ns_window, setStyleMask: new_style];
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn reposition_traffic_lights<R: Runtime>(
    window: WebviewWindow<R>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    unsafe {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as id;

        // Get the standard window buttons (close, minimize, maximize)
        let close_button: id = msg_send![ns_window, standardWindowButton: 0]; // NSWindowCloseButton
        let miniaturize_button: id = msg_send![ns_window, standardWindowButton: 1]; // NSWindowMiniaturizeButton
        let zoom_button: id = msg_send![ns_window, standardWindowButton: 2]; // NSWindowZoomButton

        if !close_button.is_null() {
            let point = NSPoint::new(x, y);
            let _: () = msg_send![close_button, setFrameOrigin: point];
        }

        if !miniaturize_button.is_null() {
            let point = NSPoint::new(x + 20.0, y);
            let _: () = msg_send![miniaturize_button, setFrameOrigin: point];
        }

        if !zoom_button.is_null() {
            let point = NSPoint::new(x + 40.0, y);
            let _: () = msg_send![zoom_button, setFrameOrigin: point];
        }
    }

    Ok(())
}

// Non-macOS platforms - return empty implementations
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn enable_rounded_corners() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn enable_modern_window_style() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn reposition_traffic_lights(_x: f64, _y: f64) -> Result<(), String> {
    Ok(())
}
