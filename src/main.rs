#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ac;
mod bridge;
mod gfx;
mod icons;
mod log;
mod panel;
mod platform;
mod scene;
mod settings;
mod shell;
mod smartthings;
mod tray_icon;

use std::path::PathBuf;
use std::ptr::null;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW};

use crate::ac::{CloudUnit, DemoUnit, Unit};
use crate::bridge::Bridge;
use crate::smartthings::Cloud;

/// A second launch asks the running copy to open its panel and quits.
fn already_running() -> bool {
    unsafe {
        CreateMutexW(null(), 1, shell::wide("Local\\PainelDoAr.ArTray").as_ptr());
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        let host = FindWindowW(shell::wide(shell::HOST_CLASS).as_ptr(), null());
        if !host.is_null() {
            let mut process = 0;
            GetWindowThreadProcessId(host, &mut process);
            AllowSetForegroundWindow(process);
            PostMessageW(host, shell::WM_APP_SHOW, 0, 0);
        }
        true
    }
}

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if already_running() {
        return;
    }
    let demo = std::env::args().any(|arg| arg == "--demo");
    let at_startup = std::env::args().any(|arg| arg == "--autostart");
    let dir = PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("dev.ar-tray.desktop");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("não deu para criar {}: {error}", dir.display());
    }
    log::init(&dir);
    let settings_file = dir.join("settings.json");
    settings::set_autostart(&settings_file, settings::autostart_enabled(&settings_file), demo);

    let unit: Box<dyn Unit> = if demo {
        Box::new(DemoUnit::new())
    } else {
        Box::new(CloudUnit::new(Cloud::new(dir.join("sessao.bin")), dir.join("device.json")))
    };
    shell::run(|publish| Bridge::start(unit, publish), settings_file, demo, !at_startup);
}
