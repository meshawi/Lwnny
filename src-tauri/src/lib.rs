//! Lwnny: one RGB controller for your whole PC. Tauri shell around [`engine::Engine`].

pub mod devices;
pub mod engine;

use devices::{DeviceInfo, Rgb};
use engine::{ColorFix, DeviceStatus, Engine, Scene, Settings, Shared};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State,
};
use tauri_plugin_autostart::ManagerExt;

pub struct AppState(pub Shared);

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e}")
}

#[derive(Serialize)]
struct Snapshot {
    devices: Vec<DeviceInfo>,
    status: Vec<DeviceStatus>,
    scene: Scene,
    settings: Settings,
    profiles: Vec<String>,
    autostart: bool,
}

#[tauri::command]
fn snapshot(app: tauri::AppHandle, state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let e = state.0.lock().map_err(err)?;
    Ok(Snapshot {
        devices: e.device_infos(),
        status: e.status().to_vec(),
        scene: e.scene().clone(),
        settings: e.settings().clone(),
        profiles: e.list_profiles(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
    })
}

#[tauri::command]
fn rescan(state: State<'_, AppState>) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.rescan();
    Ok(e.status().to_vec())
}

#[tauri::command]
fn set_zone_color(state: State<'_, AppState>, device: String, zone: usize, color: String) -> CmdResult<Vec<DeviceStatus>> {
    let c = Rgb::from_hex(&color).ok_or("bad color")?;
    let mut e = state.0.lock().map_err(err)?;
    e.set_zone_color(&device, zone, c).map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn set_device_color(state: State<'_, AppState>, device: String, color: String) -> CmdResult<Vec<DeviceStatus>> {
    let c = Rgb::from_hex(&color).ok_or("bad color")?;
    let mut e = state.0.lock().map_err(err)?;
    e.set_device_color(&device, c).map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn set_all_color(state: State<'_, AppState>, color: String) -> CmdResult<Vec<DeviceStatus>> {
    let c = Rgb::from_hex(&color).ok_or("bad color")?;
    let mut e = state.0.lock().map_err(err)?;
    e.set_all_color(c);
    Ok(e.push())
}

#[tauri::command]
fn set_locked(state: State<'_, AppState>, device: String, locked: bool) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.set_locked(&device, locked);
    Ok(e.push())
}

#[tauri::command]
fn set_zone_name(state: State<'_, AppState>, device: String, zone: usize, name: String) -> CmdResult<Vec<DeviceInfo>> {
    let mut e = state.0.lock().map_err(err)?;
    e.set_zone_name(&device, zone, &name).map_err(err)?;
    Ok(e.device_infos())
}

/// Flash a zone so the user can see where it is. The front end calls
/// `apply` a moment later to restore the scene.
#[tauri::command]
fn identify_zone(state: State<'_, AppState>, device: String, zone: usize) -> CmdResult<()> {
    let e = state.0.lock().map_err(err)?;
    e.identify_zone(&device, zone).map_err(err)
}


#[tauri::command]
fn set_brightness(state: State<'_, AppState>, brightness: u8) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.set_brightness(brightness);
    Ok(e.push())
}

#[tauri::command]
fn apply(state: State<'_, AppState>) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn save_hardware(state: State<'_, AppState>) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.push();
    Ok(e.save_hardware())
}

#[tauri::command]
fn save_profile(state: State<'_, AppState>, name: String) -> CmdResult<Vec<String>> {
    let e = state.0.lock().map_err(err)?;
    e.save_profile(&name).map_err(err)?;
    Ok(e.list_profiles())
}

#[tauri::command]
fn load_profile(state: State<'_, AppState>, name: String) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.load_profile(&name).map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn delete_profile(state: State<'_, AppState>, name: String) -> CmdResult<Vec<String>> {
    let e = state.0.lock().map_err(err)?;
    e.delete_profile(&name).map_err(err)?;
    Ok(e.list_profiles())
}

#[tauri::command]
fn set_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.set_settings(settings).map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn set_color_fix(state: State<'_, AppState>, device: String, fix: ColorFix) -> CmdResult<Vec<DeviceStatus>> {
    let mut e = state.0.lock().map_err(err)?;
    e.set_color_fix(&device, fix).map_err(err)?;
    Ok(e.push())
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> CmdResult<bool> {
    let al = app.autolaunch();
    if enabled {
        al.enable().map_err(err)?;
    } else {
        al.disable().map_err(err)?;
    }
    al.is_enabled().map_err(err)
}

/// Show the main window, recreating it if it was destroyed to free memory.
fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let Some(cfg) = app.config().app.windows.first().cloned() else { return };
    match tauri::WebviewWindowBuilder::from_config(app, &cfg).and_then(|b| b.build()) {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => log::error!("could not recreate window: {e}"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let engine = Engine::new();
    let start_minimized = engine.settings().start_minimized && std::env::args().any(|a| a == "--minimized");
    let shared: Shared = Arc::new(Mutex::new(engine));

    // Restore the last lighting as soon as we start.
    if let Ok(mut e) = shared.lock() {
        e.push();
    }
    engine::spawn_ticker(shared.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(AppState(shared))
        .invoke_handler(tauri::generate_handler![
            snapshot,
            rescan,
            set_zone_color,
            set_device_color,
            set_all_color,
            set_locked,
            set_zone_name,
            identify_zone,
            set_brightness,
            apply,
            save_hardware,
            save_profile,
            load_profile,
            delete_profile,
            set_settings,
            set_color_fix,
            set_autostart,
        ])
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Open Lwnny", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Lwnny", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let icon = app.default_window_icon().cloned().expect("window icon");
            TrayIconBuilder::with_id("tray")
                .icon(icon)
                .tooltip("Lwnny")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            if start_minimized {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.destroy();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window destroys it (freeing the WebView2 processes,
            // ~300 MB); the tray keeps the app and the lighting alive.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.destroy();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Lwnny")
        .run(|_app, event| {
            // No windows left is not a reason to exit: we live in the tray.
            if let tauri::RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}
