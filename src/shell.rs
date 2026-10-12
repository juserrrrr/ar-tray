//! Win32 side: tray icon and menu, the panel window, painting and input.

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ab_glyph::FontVec;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, EndPaint, GetDC,
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, ReleaseDC, SetDIBitsToDevice,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, HCURSOR, HICON, HTCLIENT, HWND_TOPMOST, ICONINFO, IDC_ARROW, IDC_HAND, KillTimer, LoadCursorW, MF_CHECKED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SW_HIDE, SWP_FRAMECHANGED,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_CONTEXTMENU, WM_DPICHANGED,
    WM_CHAR, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCACTIVATE, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NULL, WM_PAINT, WM_SETCURSOR, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_OVERLAPPED,
    WS_POPUP, WS_THICKFRAME,
};

use crate::bridge::Bridge;
use crate::panel::{Action, Panel};
use crate::{platform, settings, tray_icon};

pub const APP_NAME: &str = "Painel do Ar";
pub const HOST_CLASS: &str = "PainelDoAr.Host";
const PANEL_CLASS: &str = "PainelDoAr.Panel";
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_STATE: u32 = WM_APP + 2;
pub const WM_APP_SHOW: u32 = WM_APP + 3;
// Not exported by windows-sys.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const WM_MOUSELEAVE: u32 = 0x02a3;
const TRAY_ID: u32 = 1;
const FRAME_TIMER: usize = 1;
const FRAME_MS: u32 = 33;
// Clicking the tray icon deactivates an open panel first, so a click right after that means "close".
const REOPEN_GUARD: Duration = Duration::from_millis(300);
const MARGIN: f32 = 12.0;

const MENU_OPEN: usize = 1;
const MENU_POWER: usize = 2;
const MENU_REFRESH: usize = 3;
const MENU_AUTOSTART: usize = 4;
const MENU_QUIT: usize = 5;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn low_word(value: usize) -> u32 {
    (value & 0xffff) as u32
}

fn signed_low(value: isize) -> i32 {
    (value & 0xffff) as u16 as i16 as i32
}

fn signed_high(value: isize) -> i32 {
    ((value >> 16) & 0xffff) as u16 as i16 as i32
}

struct App {
    host: HWND,
    panel: HWND,
    bridge: Arc<Bridge>,
    latest: Arc<Mutex<Option<Value>>>,
    view: Value,
    ui: Panel,
    tray_font: Option<FontVec>,
    tray_icon: HICON,
    icon_key: String,
    tooltip: String,
    taskbar_created: u32,
    visible: bool,
    hidden_at: Option<Instant>,
    tracking_mouse: bool,
    pixels: Vec<u8>,
    hand: HCURSOR,
    arrow: HCURSOR,
    settings_file: PathBuf,
    demo: bool,
    /// Opens the panel once when the first state needs a sign-in, except at Windows startup.
    announce: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app unless a nested message arrived while it was already in use.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut slot| slot.as_mut().map(f)))
}

enum Outcome {
    Default,
    Result(LRESULT),
    Menu(i32, i32),
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Frame handling must work during creation, before the app exists.
    match message {
        WM_NCCALCSIZE if wparam != 0 => return 0,
        WM_NCHITTEST => return HTCLIENT as LRESULT,
        WM_NCACTIVATE => return unsafe { DefWindowProcW(hwnd, message, wparam, -1) },
        WM_ERASEBKGND => return 1,
        WM_DPICHANGED => return 0,
        _ => {}
    }
    // A failure is logged and the message falls back to Windows, so the tray icon stays alive.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_app(|app| app.handle(hwnd, message, wparam, lparam))));
    match outcome.ok().flatten() {
        Some(Outcome::Result(result)) => result,
        Some(Outcome::Menu(x, y)) => {
            show_tray_menu(x, y);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_class(name: &str) -> Vec<u16> {
    let class = wide(name);
    let window_class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: unsafe { GetModuleHandleW(null()) },
        lpszClassName: class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    unsafe { RegisterClassExW(&window_class) };
    class
}

/// Rounded corners, dark border and the system shadow from Windows 11.
fn style_panel(panel: HWND) {
    let set = |attribute: u32, value: u32| unsafe {
        DwmSetWindowAttribute(panel, attribute, (&raw const value).cast(), size_of::<u32>() as u32);
    };
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWCP_ROUND: u32 = 2;
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    set(DWMWA_BORDER_COLOR, 0x003a_312a);
    unsafe { SetWindowPos(panel, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER) };
}

/// Context menus follow the dark taskbar, through uxtheme's unnamed exports.
fn theme_menus(dark: bool) {
    unsafe {
        let module = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if module.is_null() {
            return;
        }
        if let Some(set_mode) = GetProcAddress(module, 135usize as *const u8) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            set_mode(if dark { 2 } else { 0 });
        }
        if let Some(flush) = GetProcAddress(module, 136usize as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

fn icon_from_rgba(rgba: &[u8], size: u32) -> HICON {
    unsafe {
        let screen = GetDC(null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut std::ffi::c_void = null_mut();
        let color = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let target = std::slice::from_raw_parts_mut(bits.cast::<u8>(), rgba.len());
        for (source, destination) in rgba.as_chunks::<4>().0.iter().zip(target.as_chunks_mut::<4>().0) {
            *destination = [source[2], source[1], source[0], source[3]];
        }
        let mask_bits = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, mask_bits.as_ptr().cast());
        let icon_info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

impl App {
    fn notify_data(&self) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.host;
        data.uID = TRAY_ID;
        data
    }

    fn add_tray_icon(&mut self) {
        let mut data = self.notify_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_APP_TRAY;
        data.hIcon = self.tray_icon;
        copy_tip(&mut data, &self.tooltip);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &data);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    fn device(&self) -> Option<&serde_json::Map<String, Value>> {
        self.view["device"].as_object()
    }

    /// Mirrors the state on the tray icon and tooltip, redrawing the icon only when it changes.
    fn update_tray(&mut self, force: bool) {
        let device = self.view["device"].clone();
        let on = device["ligado"] == true;
        let (dark, size) = (platform::taskbar_is_dark(), platform::tray_icon_size());
        // The icon shows the room temperature; the tooltip still has the setpoint.
        let room = device["ambiente"].as_f64().map(|room| format!("{}", room.round() as i64));
        let key = format!("{room:?}|{on}|{}|{dark}|{size}", device["modo"]);
        let tooltip = settings::tooltip_for(&self.view);
        if !force && key == self.icon_key && tooltip == self.tooltip {
            return;
        }
        let mut data = self.notify_data();
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        if force || key != self.icon_key {
            let text = room.unwrap_or_else(|| "--".to_owned());
            let tint = crate::panel::tint_for(device["modo"].as_str(), false);
            let look = tray_icon::Look { text: &text, on, dark_taskbar: dark, tint: [tint.0, tint.1, tint.2] };
            let pixels = if device.is_object() {
                tray_icon::render(self.tray_font.as_ref(), size, &look)
            } else {
                tray_icon::render_unit(size, dark)
            };
            let icon = icon_from_rgba(&pixels, size);
            if !icon.is_null() {
                let old = std::mem::replace(&mut self.tray_icon, icon);
                if !old.is_null() {
                    unsafe { DestroyIcon(old) };
                }
                data.uFlags |= NIF_ICON;
                data.hIcon = self.tray_icon;
            }
            self.icon_key = key;
        }
        self.tooltip = tooltip;
        copy_tip(&mut data, &self.tooltip);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn apply_latest(&mut self) {
        let Some(view) = self.latest.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else { return };
        self.view = view.clone();
        self.ui.set_view(view);
        self.update_energy();
        self.update_tray(false);
        if self.announce {
            match self.view["account"].as_str() {
                Some("signedOut" | "choosing" | "noDevice") => {
                    self.announce = false;
                    if !self.visible {
                        self.show();
                    }
                }
                Some("ready" | "demo") => self.announce = false,
                _ => {}
            }
        }
        self.invalidate();
    }

    /// This month's use comes from the lifetime counter, so each reading updates the month record.
    fn update_energy(&mut self) {
        let device = &self.view["device"];
        let month = match (device["id"].as_str(), device["energia"].as_f64()) {
            (Some(id), Some(total)) if !self.demo => {
                let used = settings::track_month(&self.settings_file, id, total, platform::today());
                Some((used.kwh, used.since))
            }
            _ => None,
        };
        self.ui.set_energy(settings::tariff(&self.settings_file), month);
    }

    fn invalidate(&self) {
        if self.visible {
            unsafe { InvalidateRect(self.panel, null(), 0) };
        }
    }

    fn tray_anchor(&self) -> RECT {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.host,
            uID: TRAY_ID,
            guidItem: unsafe { std::mem::zeroed() },
        };
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { Shell_NotifyIconGetRect(&identifier, &mut rect) } == 0 {
            return rect;
        }
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };
        RECT { left: cursor.x, top: cursor.y, right: cursor.x, bottom: cursor.y }
    }

    /// Opens next to the tray icon, on the side of the screen where the taskbar is.
    fn show(&mut self) {
        let anchor = self.tray_anchor();
        let center = POINT { x: (anchor.left + anchor.right) / 2, y: (anchor.top + anchor.bottom) / 2 };
        let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        unsafe { GetMonitorInfoW(monitor, &mut info) };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let scale = dpi_x.max(96) as f32 / 96.0;
        self.ui.set_scale(scale);
        self.ui.set_light(platform::apps_use_light_theme());
        self.ui.set_autostart(settings::autostart_enabled(&self.settings_file));
        let (width, height) = self.ui.pixel_size();
        let (width, height) = (width as i32, height as i32);
        let margin = (MARGIN * scale).round() as i32;
        let work = info.rcWork;
        let x = (center.x - width / 2).min(work.right - width - margin).max(work.left + margin);
        let taskbar_on_top = center.y < (info.rcMonitor.top + info.rcMonitor.bottom) / 2;
        let y = if taskbar_on_top { work.top + margin } else { work.bottom - height - margin };
        self.visible = true;
        unsafe {
            SetWindowPos(self.panel, HWND_TOPMOST, x, y, width, height, SWP_SHOWWINDOW);
            SetForegroundWindow(self.panel);
            SetTimer(self.panel, FRAME_TIMER, FRAME_MS, None);
            InvalidateRect(self.panel, null(), 0);
        }
        self.bridge.set_visible(true);
    }

    /// Hides the panel and frees its frame buffers, leaving only the tray icon running.
    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        unsafe {
            KillTimer(self.panel, FRAME_TIMER);
            ShowWindow(self.panel, SW_HIDE);
        }
        self.ui.hidden();
        self.pixels = Vec::new();
        self.bridge.set_visible(false);
    }

    fn toggle(&mut self) {
        if self.visible {
            self.hide();
        } else if self.hidden_at.is_none_or(|at| at.elapsed() > REOPEN_GUARD) {
            self.show();
        }
    }

    fn run(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Set(key, value) => self.bridge.set(key, value),
                Action::Refresh => self.bridge.refresh(),
                Action::Hide => self.hide(),
                Action::Autostart(enabled) => settings::set_autostart(&self.settings_file, enabled, self.demo),
                Action::Tariff(price) => settings::set_tariff(&self.settings_file, price),
                Action::MonthUsed(kwh) => {
                    let device = &self.view["device"];
                    if let (Some(id), Some(total)) = (device["id"].as_str(), device["energia"].as_f64()) {
                        settings::set_month_used(&self.settings_file, id, total, kwh, platform::today());
                    }
                    self.update_energy();
                }
                Action::Login => self.bridge.login(),
                Action::ReopenLogin => self.bridge.reopen_login(),
                Action::CancelLogin => self.bridge.cancel_login(),
                Action::Choose(id, name) => self.bridge.choose_device(&id, name.as_deref()),
                Action::ChangeDevice => self.bridge.change_device(),
                Action::Logout => self.bridge.logout(),
            }
        }
        self.invalidate();
    }

    fn paint(&mut self) {
        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        let dc = unsafe { BeginPaint(self.panel, &mut paint) };
        let frame = self.ui.render();
        let (width, height) = (frame.width(), frame.height());
        self.pixels.resize(frame.data().len(), 0);
        // The panel is opaque, so premultiplied RGBA only needs its channels swapped.
        for (source, destination) in frame.data().as_chunks::<4>().0.iter().zip(self.pixels.as_chunks_mut::<4>().0) {
            *destination = [source[2], source[1], source[0], 255];
        }
        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            SetDIBitsToDevice(dc, 0, 0, width, height, 0, 0, 0, height, self.pixels.as_ptr().cast(), &info, DIB_RGB_COLORS);
            EndPaint(self.panel, &paint);
        }
    }

    fn pointer(&self, lparam: LPARAM) -> (f32, f32) {
        let scale = self.ui.pixel_size().0 as f32 / crate::panel::WIDTH;
        (signed_low(lparam) as f32 / scale, signed_high(lparam) as f32 / scale)
    }

    fn menu_command(&mut self, command: usize) {
        match command {
            MENU_OPEN => {
                if !self.visible {
                    self.show();
                }
            }
            MENU_POWER => {
                if let Some(device) = self.device() {
                    let on = device["ligado"] == true;
                    self.bridge.set("ligado", json!(!on));
                }
            }
            MENU_REFRESH => self.bridge.refresh(),
            MENU_AUTOSTART => {
                let enabled = !settings::autostart_enabled(&self.settings_file);
                settings::set_autostart(&self.settings_file, enabled, self.demo);
                self.ui.set_autostart(enabled);
            }
            MENU_QUIT => {
                let data = self.notify_data();
                unsafe {
                    Shell_NotifyIconW(NIM_DELETE, &data);
                    PostQuitMessage(0);
                }
            }
            _ => {}
        }
    }

    fn handle(&mut self, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Outcome {
        if hwnd == self.host {
            return match message {
                WM_APP_STATE => {
                    self.apply_latest();
                    Outcome::Result(0)
                }
                WM_APP_SHOW => {
                    if !self.visible {
                        self.show();
                    }
                    Outcome::Result(0)
                }
                WM_APP_TRAY => match low_word(lparam as usize) {
                    NIN_SELECT | NIN_KEYSELECT => {
                        self.toggle();
                        Outcome::Result(0)
                    }
                    WM_CONTEXTMENU => Outcome::Menu(signed_low(wparam as isize), signed_high(wparam as isize)),
                    _ => Outcome::Result(0),
                },
                WM_SETTINGCHANGE => {
                    theme_menus(platform::taskbar_is_dark());
                    self.update_tray(false);
                    self.ui.set_light(platform::apps_use_light_theme());
                    self.invalidate();
                    Outcome::Default
                }
                message if message == self.taskbar_created => {
                    self.add_tray_icon();
                    self.update_tray(true);
                    Outcome::Result(0)
                }
                _ => Outcome::Default,
            };
        }
        if hwnd != self.panel {
            return Outcome::Default;
        }
        match message {
            WM_PAINT => {
                self.paint();
                Outcome::Result(0)
            }
            WM_TIMER => {
                if self.ui.animating() {
                    self.invalidate();
                }
                Outcome::Result(0)
            }
            WM_MOUSEMOVE => {
                if !self.tracking_mouse {
                    let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.panel, dwHoverTime: 0 };
                    unsafe { TrackMouseEvent(&mut track) };
                    self.tracking_mouse = true;
                }
                let (x, y) = self.pointer(lparam);
                self.ui.pointer_move(x, y);
                self.invalidate();
                Outcome::Result(0)
            }
            WM_MOUSELEAVE => {
                self.tracking_mouse = false;
                self.ui.pointer_leave();
                self.invalidate();
                Outcome::Result(0)
            }
            WM_LBUTTONDOWN => {
                unsafe { SetCapture(self.panel) };
                let (x, y) = self.pointer(lparam);
                self.ui.pointer_down(x, y);
                self.invalidate();
                Outcome::Result(0)
            }
            WM_LBUTTONUP => {
                unsafe { ReleaseCapture() };
                let (x, y) = self.pointer(lparam);
                let actions = self.ui.pointer_up(x, y);
                self.run(actions);
                Outcome::Result(0)
            }
            // Typed characters only matter while a number is being edited.
            WM_CHAR => {
                let actions = char::from_u32(wparam as u32).map(|typed| self.ui.type_char(typed)).unwrap_or_default();
                self.run(actions);
                Outcome::Result(0)
            }
            WM_KEYDOWN => {
                let actions = match wparam as u16 {
                    VK_ESCAPE => self.ui.escape(),
                    VK_UP | VK_RIGHT => self.ui.nudge(1),
                    VK_DOWN | VK_LEFT => self.ui.nudge(-1),
                    _ => return Outcome::Default,
                };
                self.run(actions);
                Outcome::Result(0)
            }
            WM_SETCURSOR if low_word(lparam as usize) == HTCLIENT => {
                unsafe { SetCursor(if self.ui.wants_hand() { self.hand } else { self.arrow }) };
                Outcome::Result(1)
            }
            // While the browser login runs, the panel stays beside the browser.
            WM_ACTIVATE if low_word(wparam) == WA_INACTIVE => {
                if !self.ui.keeps_open() {
                    self.hide();
                }
                Outcome::Result(0)
            }
            _ => Outcome::Default,
        }
    }
}

fn copy_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    let tip: Vec<u16> = text.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip = [0; 128];
    data.szTip[..tip.len()].copy_from_slice(&tip);
}

/// The tray menu runs its own message loop, so it is shown outside the app borrow.
fn show_tray_menu(x: i32, y: i32) {
    let Some((host, has_device, on, autostart, signed_out)) = with_app(|app| {
        let on = app.device().is_some_and(|device| device["ligado"] == true);
        let signed_out = app.view["account"] == "signedOut";
        (app.host, app.device().is_some(), on, settings::autostart_enabled(&app.settings_file), signed_out)
    }) else {
        return;
    };
    let command = unsafe {
        let menu = CreatePopupMenu();
        let item = |flags, id, text: &str| AppendMenuW(menu, flags, id, wide(text).as_ptr());
        item(MF_STRING, MENU_OPEN, if signed_out { "Conectar conta Samsung" } else { "Abrir painel" });
        item(MF_STRING | if has_device { 0 } else { MF_GRAYED }, MENU_POWER, if on { "Desligar" } else { "Ligar" });
        item(MF_STRING, MENU_REFRESH, "Atualizar agora");
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        item(MF_STRING | if autostart { MF_CHECKED } else { 0 }, MENU_AUTOSTART, "Abrir com o Windows");
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        item(MF_STRING, MENU_QUIT, "Sair");
        SetForegroundWindow(host);
        let command = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, host, null());
        PostMessageW(host, WM_NULL, 0, 0);
        DestroyMenu(menu);
        command
    };
    if command > 0 {
        with_app(|app| app.menu_command(command as usize));
    }
}

/// Creates the windows, starts the bridge and runs the message loop until "Sair".
pub fn run(start_bridge: impl FnOnce(Box<dyn Fn(&Value) + Send + Sync>) -> Arc<Bridge>, settings_file: PathBuf, demo: bool, announce: bool) {
    let host_class = register_class(HOST_CLASS);
    let panel_class = register_class(PANEL_CLASS);
    let title = wide(APP_NAME);
    let instance = unsafe { GetModuleHandleW(null()) };
    let host = unsafe {
        CreateWindowExW(0, host_class.as_ptr(), title.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, null_mut(), null_mut(), instance, null())
    };
    let panel = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            panel_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_THICKFRAME,
            0,
            0,
            372,
            560,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    style_panel(panel);
    theme_menus(platform::taskbar_is_dark());

    let latest: Arc<Mutex<Option<Value>>> = Arc::default();
    let shared = Arc::clone(&latest);
    let host_address = host as usize;
    let bridge = start_bridge(Box::new(move |view| {
        *shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(view.clone());
        unsafe { PostMessageW(host_address as HWND, WM_APP_STATE, 0, 0) };
    }));

    let mut ui = Panel::new();
    ui.set_autostart(settings::autostart_enabled(&settings_file));
    let mut app = App {
        host,
        panel,
        bridge,
        latest,
        view: Value::Null,
        ui,
        tray_font: tray_icon::load_font(),
        tray_icon: null_mut(),
        icon_key: String::new(),
        tooltip: APP_NAME.to_owned(),
        taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        visible: false,
        hidden_at: None,
        tracking_mouse: false,
        pixels: Vec::new(),
        hand: unsafe { LoadCursorW(null_mut(), IDC_HAND) },
        arrow: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        settings_file,
        demo,
        announce,
    };
    app.update_tray(true);
    app.add_tray_icon();
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(App::apply_latest);

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
