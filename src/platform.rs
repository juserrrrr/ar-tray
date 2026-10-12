//! Windows calls: taskbar theme, startup entry, DPAPI and the default browser.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The taskbar follows the system theme, which can differ from the apps theme.
pub fn taskbar_is_dark() -> bool {
    let (key, value) = (wide(PERSONALIZE_KEY), wide("SystemUsesLightTheme"));
    let mut light: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut light).cast(),
            &mut size,
        )
    };
    status != ERROR_SUCCESS || light == 0
}

/// The apps theme, which the panel follows.
pub fn apps_use_light_theme() -> bool {
    let (key, value) = (wide(PERSONALIZE_KEY), wide("AppsUseLightTheme"));
    let mut light: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&raw mut light).cast(), &mut size)
    };
    status == ERROR_SUCCESS && light == 1
}

/// Tray icon side in pixels for the system DPI (16 at 100%, 24 at 150%).
pub fn tray_icon_size() -> u32 {
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    (16.0 * dpi as f32 / 96.0).round() as u32
}

pub fn set_autostart(name: &str, exe: &Path, enabled: bool) -> Result<(), String> {
    let (key, value) = (wide(RUN_KEY), wide(name));
    let status = if enabled {
        // The flag keeps the panel closed when Windows starts the app.
        let command = wide(&format!("\"{}\" --autostart", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            other => other,
        }
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registro do Windows recusou o início automático (erro {status})"))
    }
}

/// Encrypts with the Windows account key, so the file is useless on another account or PC.
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    crypt(data, true)
}

pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    crypt(data, false)
}

fn crypt(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, null(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, null_mut(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        }
    };
    if ok == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData as *mut c_void) };
    Some(bytes)
}

pub fn open_url(url: &str) {
    let (verb, target) = (wide("open"), wide(url));
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), target.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}
