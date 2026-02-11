use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, Runtime, WebviewUrl, WebviewWindowBuilder,
    window::Color,
};

pub fn show_or_create_window<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        let app_handle = app.clone();
        let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
            .title("Cntrl Bridge")
            .inner_size(380.0, 626.0)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .background_color(Color(23, 23, 23, 255))
            .center()
            .skip_taskbar(true)
            .visible(false)
            .on_page_load(move |_webview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    if let Some(win) = app_handle.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            })
            .build();
    }
}

pub fn create_tray<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let open_i = MenuItem::with_id(app, "open", "Open Dashboard", true, None::<&str>)?;
    let status_i = MenuItem::with_id(app, "status", "Server: Running", false, None::<&str>)?;

    // Separator
    let menu = Menu::with_items(app, &[&status_i, &open_i, &quit_i])?;

    let _tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => {
                app.exit(0);
            }
            "open" => {
                show_or_create_window(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } => {
                show_or_create_window(tray.app_handle());
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}
