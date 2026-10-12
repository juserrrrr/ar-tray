//! The panel: layout, drawing and input, in logical pixels of a 372 by 560 card.

use std::time::Instant;

use serde_json::{Map, Value, json};
use tiny_skia::{Pixmap, Transform};

use crate::gfx::{Align, Canvas, Rgba, Style, circle, fonts, paint, radial, rounded, solid, thousands};
use crate::scene::{self, Scene};

pub const WIDTH: f32 = 372.0;
pub const HEIGHT: f32 = 624.0;
const TEMP_MIN: i64 = 16;
const TEMP_MAX: i64 = 30;

/// Neutral grays with one vivid accent per mode, in a dark and a light version.
#[derive(Clone, Copy)]
struct Palette {
    light: bool,
    background: Rgba,
    glow: Rgba,
    card: Rgba,
    card_hover: Rgba,
    line: Rgba,
    text: Rgba,
    muted: Rgba,
    faint: Rgba,
    danger: Rgba,
    track: Rgba,
    idle: Rgba,
}

const DARK: Palette = Palette {
    light: false,
    background: Rgba::hex(0x121316),
    glow: Rgba::hex(0x1f2126),
    card: Rgba::hex(0x1b1c20),
    card_hover: Rgba::hex(0x24262b),
    line: Rgba(255, 255, 255, 15),
    text: Rgba::hex(0xf2f3f5),
    muted: Rgba::hex(0x9c9ea6),
    faint: Rgba::hex(0x63656d),
    danger: Rgba::hex(0xff8a78),
    track: Rgba::hex(0x35373d),
    idle: Rgba::hex(0x74767e),
};

const LIGHT: Palette = Palette {
    light: true,
    background: Rgba::hex(0xf2f3f5),
    glow: Rgba::hex(0xffffff),
    card: Rgba::hex(0xffffff),
    card_hover: Rgba::hex(0xf7f8fa),
    line: Rgba(0, 0, 0, 17),
    text: Rgba::hex(0x16181d),
    muted: Rgba::hex(0x5d626d),
    faint: Rgba::hex(0x9ba0aa),
    danger: Rgba::hex(0xd4442e),
    track: Rgba::hex(0xd6d9df),
    idle: Rgba::hex(0x8d929c),
};

/// The mode's accent. The tray icon uses the dark version.
pub fn tint_for(mode: Option<&str>, light: bool) -> Rgba {
    let (dark, bright) = match mode {
        Some("dry") => (0x0e9f86, 0x22c3a6),
        Some("wind") => (0x66758a, 0x9eabbb),
        Some("auto") => (0xc98400, 0xf5a524),
        Some("heat") => (0xe2531d, 0xff6a3d),
        _ => (0x0b6cf2, 0x3d8bff),
    };
    Rgba::hex(if light { dark } else { bright })
}

/// White on deep accents, ink on bright ones.
fn on_accent(accent: Rgba) -> Rgba {
    let luminance = 0.2126 * accent.0 as f32 + 0.7152 * accent.1 as f32 + 0.0722 * accent.2 as f32;
    if luminance > 165.0 { Rgba::hex(0x16181d) } else { Rgba::hex(0xffffff) }
}

const HEADER_POWER: (f32, f32) = (334.0, 38.0);
// The unit sits below the header with room to breathe, and its air ends above the setpoint.
const HERO_Y: f32 = 74.0;
const HERO_HEIGHT: f32 = 150.0;
const CLIMATE_Y: f32 = 278.0;
const TEMP_DOWN: (f32, f32) = (42.0, CLIMATE_Y);
const TEMP_UP: (f32, f32) = (330.0, CLIMATE_Y);
/// The slider's track runs between these x and sits on this y.
const SLIDER: (f32, f32, f32) = (34.0, 338.0, 330.0);
const SLIDER_HIT: (f32, f32, f32, f32) = (18.0, 312.0, 336.0, 36.0);
const MODES: (f32, f32, f32, f32) = (14.0, 370.0, 344.0, 62.0);
/// Fan and direction tiles, then the WindFree, sleep and turbo switches.
const TILES_Y: f32 = 444.0;
const SWITCHES_Y: f32 = 508.0;
const SWITCH_HEIGHT: f32 = 46.0;
const STATUS_Y: f32 = 592.0;
const SETTINGS_BUTTON: (f32, f32) = (336.0, STATUS_Y);
const BACK_BUTTON: (f32, f32) = (32.0, 34.0);
const ROW_HEIGHT: f32 = 54.0;
const MENU_WIDTH: f32 = 196.0;
const MENU_ROW: f32 = 36.0;
const CONNECT_TITLE_Y: f32 = 276.0;
const PRIMARY: (f32, f32, f32, f32) = (18.0, 340.0, 336.0, 48.0);
const SECONDARY_LEFT: (f32, f32, f32, f32) = (14.0, 404.0, 168.0, 46.0);
const SECONDARY_RIGHT: (f32, f32, f32, f32) = (190.0, 404.0, 168.0, 46.0);
const SECONDARY_WIDE: (f32, f32, f32, f32) = (14.0, 404.0, 344.0, 46.0);
const DEVICE_ROWS: usize = 4;

struct Choice {
    value: &'static str,
    label: &'static str,
    short: &'static str,
    icon: &'static str,
}

const fn choice(value: &'static str, label: &'static str, short: &'static str, icon: &'static str) -> Choice {
    Choice { value, label, short, icon }
}

const MODE_CHOICES: [Choice; 5] = [
    choice("cool", "Resfriar", "Frio", "snowflake"),
    choice("dry", "Desumidificar", "Seco", "droplets"),
    choice("wind", "Ventilar", "Ventilar", "fan"),
    choice("auto", "Automático", "Auto", "sparkles"),
    choice("heat", "Aquecer", "Quente", "flame"),
];

const FAN_CHOICES: [Choice; 5] = [
    choice("auto", "Automático", "Auto", "sparkles"),
    choice("low", "Baixo", "Baixo", "signal-low"),
    choice("medium", "Médio", "Médio", "signal-medium"),
    choice("high", "Alto", "Alto", "signal-high"),
    choice("turbo", "Turbo", "Turbo", "signal"),
];

const SWING_CHOICES: [Choice; 4] = [
    choice("fixed", "Fixa", "Fixa", "minus"),
    choice("vertical", "Vertical", "Vertical", "move-vertical"),
    choice("horizontal", "Lateral", "Lateral", "move-horizontal"),
    choice("all", "Vertical e lateral", "Ambas", "move"),
];

const SETTING_ROWS: [(&str, &str, &str, Option<&str>); 3] = [
    ("display", "sun", "Luz do display", None),
    ("bipe", "volume-2", "Bipe", Some("Apita quando recebe um comando")),
    ("autoclean", "wind", "Secagem automática", Some("Seca o aparelho por dentro ao desligar")),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Menu {
    Fan,
    Swing,
}

impl Menu {
    fn key(self) -> &'static str {
        match self {
            Menu::Fan => "vento",
            Menu::Swing => "oscilar",
        }
    }

    fn supported_key(self) -> &'static str {
        match self {
            Menu::Fan => "ventos",
            Menu::Swing => "oscilacoes",
        }
    }

    fn choices(self) -> &'static [Choice] {
        match self {
            Menu::Fan => &FAN_CHOICES,
            Menu::Swing => &SWING_CHOICES,
        }
    }

    fn tile(self) -> usize {
        match self {
            Menu::Fan => 0,
            Menu::Swing => 1,
        }
    }
}

/// Which screen the account state calls for.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Screen {
    Main,
    Settings,
    SignIn,
    Waiting,
    Choose,
    NoDevice,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Power,
    TempDown,
    TempUp,
    Slider,
    Mode(usize),
    Tile(usize),
    Settings,
    Retry,
    Back,
    Toggle(usize),
    ChangeDevice,
    Logout,
    Item(usize),
    Primary,
    Reopen,
    Cancel,
    OtherAccount,
    Device(usize),
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Set(&'static str, Value),
    Refresh,
    Hide,
    Autostart(bool),
    Login,
    ReopenLogin,
    CancelLogin,
    Choose(String, Option<String>),
    ChangeDevice,
    Logout,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ok,
    Busy,
    Quiet,
    Error,
}

struct Status {
    tone: Tone,
    icon: &'static str,
    text: String,
    retry: bool,
}

/// A number that follows its target smoothly.
struct Ease {
    value: f32,
    target: f32,
}

impl Ease {
    fn step(&mut self, seconds: f32) -> bool {
        if self.value == self.target {
            return false;
        }
        self.value += (self.target - self.value) * (1.0 - (-seconds / 0.16).exp());
        if (self.target - self.value).abs() < 0.004 {
            self.value = self.target;
        }
        true
    }
}

fn shown(value: &Value, unit: &str) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(text) => format!("{text}{unit}"),
        other => format!("{other}{unit}"),
    }
}

fn is_windfree(extra: Option<&str>) -> bool {
    matches!(extra, Some("windFree" | "windFreeSleep"))
}

fn is_sleep(extra: Option<&str>) -> bool {
    matches!(extra, Some("sleep" | "windFreeSleep"))
}

/// Samsung's fast turbo, which runs for a while and then returns to the previous setting.
fn is_turbo(extra: Option<&str>) -> bool {
    extra == Some("speed")
}

fn extra_for(windfree: bool, sleep: bool) -> &'static str {
    match (windfree, sleep) {
        (false, false) => "off",
        (true, false) => "windFree",
        (false, true) => "sleep",
        (true, true) => "windFreeSleep",
    }
}

fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} e {last}", rest.join(", ")),
    }
}

fn unconfirmed_name(key: &str) -> &str {
    match key {
        "ligado" => "o liga e desliga",
        "temp" => "a temperatura",
        "modo" => "o modo",
        "vento" => "o vento",
        "extra" => "o WindFree, o sono ou o turbo",
        "oscilar" => "a direção",
        "display" => "a luz do display",
        "bipe" => "o bipe",
        "autoclean" => "a secagem automática",
        other => other,
    }
}

fn inside_circle(x: f32, y: f32, center: (f32, f32), radius: f32) -> bool {
    (x - center.0).hypot(y - center.1) <= radius
}

fn inside(x: f32, y: f32, rect: (f32, f32, f32, f32)) -> bool {
    x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3
}

fn slider_x(value: f32) -> f32 {
    let (left, right, _) = SLIDER;
    left + (value - TEMP_MIN as f32) / (TEMP_MAX - TEMP_MIN) as f32 * (right - left)
}

fn slider_value(x: f32) -> i64 {
    let (left, right, _) = SLIDER;
    let ratio = (x - left) / (right - left);
    (TEMP_MIN as f32 + ratio.clamp(0.0, 1.0) * (TEMP_MAX - TEMP_MIN) as f32).round() as i64
}

/// Tiles 0 and 1 open the fan and direction menus; 2 to 4 switch WindFree, sleep and turbo.
fn tile_rect(index: usize) -> (f32, f32, f32, f32) {
    match index {
        0 | 1 => (14.0 + index as f32 * 176.0, TILES_Y, 168.0, 56.0),
        _ => {
            let width = (344.0 - 16.0) / 3.0;
            (14.0 + (index - 2) as f32 * (width + 8.0), SWITCHES_Y, width, SWITCH_HEIGHT)
        }
    }
}

/// Seconds since 1970 for an ISO 8601 UTC time such as 2026-10-12T00:24:03Z.
fn epoch_seconds(iso: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from the civil calendar, after Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn settings_row(index: usize) -> (f32, f32, f32, f32) {
    // Three switches, then filter and energy, then autostart, then the account.
    let y = match index {
        0..=2 => 68.0 + index as f32 * ROW_HEIGHT,
        3 | 4 => 246.0 + (index - 3) as f32 * ROW_HEIGHT,
        5 => 370.0,
        _ => 440.0 + (index - 6) as f32 * ROW_HEIGHT,
    };
    (14.0, y, 344.0, ROW_HEIGHT)
}

fn device_row(index: usize) -> (f32, f32, f32, f32) {
    (14.0, PRIMARY.1 + index as f32 * 54.0, 344.0, 48.0)
}

pub struct Panel {
    view: Value,
    autostart: bool,
    palette: Palette,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    menu: Option<Menu>,
    settings: bool,
    drag: Option<i64>,
    wheel: i32,
    power: Ease,
    windfree: Ease,
    scene: Scene,
    clock: Instant,
    last_tick: Instant,
    scale: f32,
    base: Option<Pixmap>,
    frame: Option<Pixmap>,
    dirty: bool,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            view: Value::Null,
            autostart: false,
            palette: DARK,
            hover: None,
            pressed: None,
            menu: None,
            settings: false,
            drag: None,
            wheel: 0,
            power: Ease { value: 0.0, target: 0.0 },
            windfree: Ease { value: 0.0, target: 0.0 },
            scene: Scene::new(),
            clock: Instant::now(),
            last_tick: Instant::now(),
            scale: 1.0,
            base: None,
            frame: None,
            dirty: true,
        }
    }

    pub fn set_view(&mut self, view: Value) {
        self.view = view;
        let on = self.device().is_some_and(|device| device["ligado"] == true);
        self.power.target = if on { 1.0 } else { 0.0 };
        self.windfree.target = if is_windfree(self.extra()) { 1.0 } else { 0.0 };
        if self.device().is_none() {
            self.menu = None;
            self.drag = None;
        }
        if self.account_screen().is_some() {
            self.settings = false;
        }
        self.dirty = true;
    }

    pub fn set_autostart(&mut self, enabled: bool) {
        self.autostart = enabled;
        self.dirty = true;
    }

    pub fn set_light(&mut self, light: bool) {
        if light != self.palette.light {
            self.palette = if light { LIGHT } else { DARK };
            self.dirty = true;
        }
    }

    /// Frees the frame buffers while hidden and resets transient UI.
    pub fn hidden(&mut self) {
        self.menu = None;
        self.settings = false;
        self.hover = None;
        self.pressed = None;
        self.drag = None;
        self.base = None;
        self.frame = None;
        self.power.value = self.power.target;
        self.windfree.value = self.windfree.target;
        self.dirty = true;
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.base = None;
            self.frame = None;
            self.dirty = true;
        }
    }

    pub fn pixel_size(&self) -> (u32, u32) {
        ((WIDTH * self.scale).round() as u32, (HEIGHT * self.scale).round() as u32)
    }

    fn account(&self) -> &str {
        self.view["account"].as_str().unwrap_or("starting")
    }

    /// While the browser login runs, the panel stays open next to it.
    pub fn keeps_open(&self) -> bool {
        self.account() == "signingIn"
    }

    fn account_screen(&self) -> Option<Screen> {
        match self.account() {
            "signedOut" => Some(Screen::SignIn),
            "signingIn" => Some(Screen::Waiting),
            "choosing" => Some(Screen::Choose),
            "noDevice" => Some(Screen::NoDevice),
            _ => None,
        }
    }

    fn screen(&self) -> Screen {
        self.account_screen().unwrap_or(if self.settings { Screen::Settings } else { Screen::Main })
    }

    fn device(&self) -> Option<&Map<String, Value>> {
        self.view["device"].as_object()
    }

    fn field(&self, key: &str) -> &Value {
        self.device().and_then(|device| device.get(key)).unwrap_or(&Value::Null)
    }

    fn extra(&self) -> Option<&str> {
        self.field("extra").as_str()
    }

    fn is_on(&self) -> bool {
        self.field("ligado") == true
    }

    fn temp(&self) -> Option<i64> {
        self.drag.or_else(|| self.field("temp").as_i64())
    }

    fn mode(&self) -> Option<&str> {
        self.field("modo").as_str()
    }

    fn tint(&self) -> Rgba {
        tint_for(self.mode(), self.palette.light)
    }

    fn accent(&self) -> Rgba {
        if self.is_on() { self.tint() } else { self.palette.idle }
    }

    fn supports(&self, list_key: &str, value: &str) -> bool {
        match self.field(list_key).as_array() {
            Some(list) if !list.is_empty() => list.iter().any(|item| item == value),
            _ => true,
        }
    }

    /// WindFree and sleep need the optional mode capability, which only some units have.
    fn supports_extra(&self, value: &str) -> bool {
        match self.field("extras").as_array() {
            Some(list) if !list.is_empty() => list.iter().any(|item| item == value),
            _ => !self.field("extra").is_null(),
        }
    }

    fn visible_modes(&self) -> Vec<&'static Choice> {
        let current = self.mode();
        MODE_CHOICES.iter().filter(|choice| self.supports("modos", choice.value) || Some(choice.value) == current).collect()
    }

    fn menu_choices(&self, menu: Menu) -> Vec<&'static Choice> {
        let current = self.field(menu.key()).as_str();
        menu.choices().iter().filter(|choice| self.supports(menu.supported_key(), choice.value) || Some(choice.value) == current).collect()
    }

    fn devices(&self) -> Vec<(String, Option<String>)> {
        self.view["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|device| Some((device["id"].as_str()?.to_owned(), device["nome"].as_str().map(str::to_owned))))
            .take(DEVICE_ROWS)
            .collect()
    }

    fn mode_rect(&self, index: usize, count: usize) -> (f32, f32, f32, f32) {
        let (x, y, width, height) = MODES;
        let gap = 4.0;
        let button = (width - 10.0 - gap * (count as f32 - 1.0)) / count as f32;
        (x + 5.0 + index as f32 * (button + gap), y + 5.0, button, height - 10.0)
    }

    fn menu_rect(&self, menu: Menu) -> (f32, f32, f32, f32) {
        let count = self.menu_choices(menu).len() as f32;
        let height = 12.0 + count * MENU_ROW;
        let (tile_x, tile_y, _, _) = tile_rect(menu.tile());
        let x = tile_x.clamp(12.0, WIDTH - MENU_WIDTH - 12.0);
        (x, tile_y - 8.0 - height, MENU_WIDTH, height)
    }

    fn status(&self) -> Status {
        let status = |tone, icon, text: &str, retry| Status { tone, icon, text: text.to_owned(), retry };
        match self.view["error"]["kind"].as_str() {
            Some("failed") => return status(Tone::Error, "cloud-off", "Sem resposta do SmartThings", true),
            Some("unconfirmed") => {
                let keys: Vec<&str> = self.view["error"]["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(unconfirmed_name).collect();
                return status(Tone::Error, "triangle-alert", &format!("O ar não confirmou {}", join_names(&keys)), false);
            }
            _ => {}
        }
        if self.device().is_none() {
            return status(Tone::Busy, "loader-circle", "Conectando ao SmartThings…", false);
        }
        if self.view["syncing"] == true {
            return status(Tone::Busy, "loader-circle", "Sincronizando…", false);
        }
        if self.view["held"].as_array().is_some_and(|held| !held.is_empty()) {
            return status(Tone::Quiet, "clock", "Aplica quando ligar", false);
        }
        if self.account() == "demo" {
            return status(Tone::Quiet, "sparkles", "Modo demonstração, sem conta Samsung", false);
        }
        status(Tone::Ok, "shield-check", "Conectado ao SmartThings", false)
    }

    /// Where the status text ends and the retry link sits.
    fn status_layout(&self, status: &Status) -> (String, Option<(f32, f32)>) {
        let fonts = fonts();
        let retry_width = if status.retry { fonts.measure("Tentar de novo", Style::Medium, 13.0) + 12.0 } else { 0.0 };
        let text = fonts.fit(&status.text, Style::Regular, 13.0, 270.0 - retry_width);
        let retry = status.retry.then(|| {
            let start = 42.0 + fonts.measure(&text, Style::Regular, 13.0) + 12.0;
            (start, start + retry_width - 12.0)
        });
        (text, retry)
    }

    fn enabled(&self, hit: Hit) -> bool {
        let has_device = self.device().is_some();
        match hit {
            Hit::Power | Hit::Slider | Hit::Mode(_) | Hit::Tile(0 | 1) | Hit::Item(_) => has_device,
            Hit::TempDown => has_device && self.temp().is_some_and(|temp| temp > TEMP_MIN),
            Hit::TempUp => has_device && self.temp().is_some_and(|temp| temp < TEMP_MAX),
            Hit::Tile(2) => has_device && self.supports_extra("windFree"),
            Hit::Tile(3) => has_device && self.supports_extra("sleep"),
            Hit::Tile(4) => has_device && self.supports_extra("speed"),
            Hit::Toggle(index) if index < 3 => has_device,
            _ => true,
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        match self.screen() {
            Screen::SignIn => inside(x, y, PRIMARY).then_some(Hit::Primary),
            Screen::Waiting => [(SECONDARY_LEFT, Hit::Reopen), (SECONDARY_RIGHT, Hit::Cancel)].into_iter().find(|(rect, _)| inside(x, y, *rect)).map(|(_, hit)| hit),
            Screen::Choose => (0..self.devices().len()).find(|&index| inside(x, y, device_row(index))).map(Hit::Device),
            Screen::NoDevice => [(PRIMARY, Hit::Primary), (SECONDARY_WIDE, Hit::OtherAccount)].into_iter().find(|(rect, _)| inside(x, y, *rect)).map(|(_, hit)| hit),
            Screen::Settings => self.settings_hit(x, y),
            Screen::Main => self.main_hit(x, y),
        }
    }

    fn settings_hit(&self, x: f32, y: f32) -> Option<Hit> {
        if inside_circle(x, y, BACK_BUTTON, 20.0) {
            return Some(Hit::Back);
        }
        if let Some(index) = [0, 1, 2, 5].into_iter().position(|row| inside(x, y, settings_row(row))) {
            return Some(Hit::Toggle(index));
        }
        if self.account() == "demo" {
            return None;
        }
        [(6, Hit::ChangeDevice), (7, Hit::Logout)].into_iter().find(|(row, _)| inside(x, y, settings_row(*row))).map(|(_, hit)| hit)
    }

    fn main_hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(menu) = self.menu {
            let rect = self.menu_rect(menu);
            if inside(x, y, rect) {
                let row = ((y - rect.1 - 6.0) / MENU_ROW).floor();
                let count = self.menu_choices(menu).len();
                return (row >= 0.0 && (row as usize) < count).then_some(Hit::Item(row as usize));
            }
        }
        let candidates = [
            (inside_circle(x, y, HEADER_POWER, 22.0), Hit::Power),
            (inside_circle(x, y, TEMP_DOWN, 23.0), Hit::TempDown),
            (inside_circle(x, y, TEMP_UP, 23.0), Hit::TempUp),
            (inside(x, y, SLIDER_HIT), Hit::Slider),
            (inside_circle(x, y, SETTINGS_BUTTON, 19.0), Hit::Settings),
        ];
        if let Some((_, hit)) = candidates.into_iter().find(|(within, _)| *within) {
            return Some(hit);
        }
        let modes = self.visible_modes().len();
        if let Some(index) = (0..modes).find(|&index| inside(x, y, self.mode_rect(index, modes))) {
            return Some(Hit::Mode(index));
        }
        if let Some(index) = (0..5).find(|&index| inside(x, y, tile_rect(index))) {
            return Some(Hit::Tile(index));
        }
        let status = self.status();
        if let (_, Some((start, end))) = self.status_layout(&status)
            && x >= start
            && x <= end
            && (y - STATUS_Y).abs() < 12.0
        {
            return Some(Hit::Retry);
        }
        None
    }

    /// The pointer shows a hand over controls that would react to a click.
    pub fn wants_hand(&self) -> bool {
        self.drag.is_some() || self.hover.is_some_and(|hit| self.enabled(hit))
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        if self.drag.is_some() {
            let value = slider_value(x);
            if self.drag != Some(value) {
                self.drag = Some(value);
                self.dirty = true;
            }
            return;
        }
        let hit = self.hit(x, y);
        if hit != self.hover {
            self.hover = hit;
            self.dirty = true;
        }
    }

    pub fn pointer_leave(&mut self) {
        if self.hover.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.pressed = self.hit(x, y).filter(|&hit| self.enabled(hit));
        if self.pressed == Some(Hit::Slider) && self.menu.is_none() {
            self.drag = Some(slider_value(x));
        }
        self.dirty = true;
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) -> Vec<Action> {
        let pressed = self.pressed.take();
        self.dirty = true;
        if let Some(value) = self.drag.take() {
            return vec![Action::Set("temp", json!(value))];
        }
        let hit = self.hit(x, y).filter(|&hit| self.enabled(hit));
        let open_menu = self.menu.take();
        if hit.is_none() || hit != pressed {
            return Vec::new();
        }
        match (hit, open_menu) {
            (Some(Hit::Item(index)), Some(menu)) => {
                let choices = self.menu_choices(menu);
                choices.get(index).map(|choice| vec![Action::Set(menu.key(), json!(choice.value))]).unwrap_or_default()
            }
            (Some(Hit::Tile(tile @ (0 | 1))), open) => {
                let menu = if tile == 0 { Menu::Fan } else { Menu::Swing };
                if open != Some(menu) {
                    self.menu = Some(menu);
                }
                Vec::new()
            }
            (Some(hit), _) => self.activate(hit),
            (None, _) => Vec::new(),
        }
    }

    fn activate(&mut self, hit: Hit) -> Vec<Action> {
        let on = self.is_on();
        let extra = self.extra();
        let (windfree, sleep) = (is_windfree(extra), is_sleep(extra));
        let action = match hit {
            Hit::Power => Action::Set("ligado", json!(!on)),
            Hit::TempDown | Hit::TempUp => {
                let step = if hit == Hit::TempUp { 1 } else { -1 };
                let Some(temp) = self.temp() else { return Vec::new() };
                Action::Set("temp", json!((temp + step).clamp(TEMP_MIN, TEMP_MAX)))
            }
            Hit::Mode(index) => match self.visible_modes().get(index) {
                Some(choice) => Action::Set("modo", json!(choice.value)),
                None => return Vec::new(),
            },
            Hit::Tile(2) => Action::Set("extra", json!(extra_for(!windfree, sleep))),
            Hit::Tile(3) => Action::Set("extra", json!(extra_for(windfree, !sleep))),
            Hit::Tile(4) => Action::Set("extra", json!(if is_turbo(extra) { "off" } else { "speed" })),
            Hit::Settings => {
                self.settings = true;
                return Vec::new();
            }
            Hit::Back => {
                self.settings = false;
                return Vec::new();
            }
            Hit::Retry => Action::Refresh,
            Hit::Toggle(3) => {
                self.autostart = !self.autostart;
                Action::Autostart(self.autostart)
            }
            Hit::Toggle(index) => {
                let (key, ..) = SETTING_ROWS[index];
                Action::Set(key, json!(if self.field(key) == "on" { "off" } else { "on" }))
            }
            Hit::ChangeDevice => {
                self.settings = false;
                Action::ChangeDevice
            }
            Hit::Logout => {
                self.settings = false;
                Action::Logout
            }
            // "Procurar de novo" on the empty account looks for devices again.
            Hit::Primary if self.account() == "noDevice" => Action::Refresh,
            Hit::Primary => Action::Login,
            Hit::Reopen => Action::ReopenLogin,
            Hit::Cancel => Action::CancelLogin,
            Hit::OtherAccount => Action::Logout,
            Hit::Device(index) => match self.devices().into_iter().nth(index) {
                Some((id, name)) => Action::Choose(id, name),
                None => return Vec::new(),
            },
            Hit::Slider | Hit::Tile(_) | Hit::Item(_) => return Vec::new(),
        };
        vec![action]
    }

    /// One notch of the wheel moves the setpoint one degree.
    pub fn wheel(&mut self, delta: i32) -> Vec<Action> {
        if self.screen() != Screen::Main || self.menu.is_some() || self.device().is_none() {
            return Vec::new();
        }
        self.wheel += delta;
        let notches = self.wheel / 120;
        if notches == 0 {
            return Vec::new();
        }
        self.wheel -= notches * 120;
        self.nudge(notches as i64)
    }

    pub fn escape(&mut self) -> Vec<Action> {
        self.dirty = true;
        if self.menu.take().is_some() {
            return Vec::new();
        }
        if self.screen() == Screen::Settings {
            self.settings = false;
            return Vec::new();
        }
        vec![Action::Hide]
    }

    pub fn nudge(&mut self, step: i64) -> Vec<Action> {
        if self.screen() != Screen::Main || self.device().is_none() {
            return Vec::new();
        }
        let Some(temp) = self.temp() else { return Vec::new() };
        let next = (temp + step).clamp(TEMP_MIN, TEMP_MAX);
        if next == temp { Vec::new() } else { vec![Action::Set("temp", json!(next))] }
    }

    fn spinning(&self) -> bool {
        match self.screen() {
            Screen::Main => self.status().icon == "loader-circle",
            Screen::Waiting => true,
            _ => false,
        }
    }

    /// True while something moves, so the window keeps asking for frames.
    pub fn animating(&self) -> bool {
        let air = self.screen() == Screen::Main && self.power.value > 0.01;
        air || self.spinning() || self.dirty || self.power.value != self.power.target || self.windfree.value != self.windfree.target
    }

    fn look(&self) -> scene::Look {
        let fan = self.field("vento").as_str().unwrap_or("auto");
        let (mut seconds, mut strength) = match fan {
            "low" => (3.4, 0.6),
            "medium" => (2.6, 0.75),
            "high" => (1.8, 0.92),
            "turbo" => (1.15, 1.0),
            _ => (2.4, 0.85),
        };
        if self.mode() == Some("dry") {
            (seconds, strength) = (3.6, 0.5);
        }
        let swing = self.field("oscilar").as_str().unwrap_or("fixed");
        let connected = self.account_screen().is_none();
        scene::Look {
            tint: self.tint(),
            power: if connected { self.power.value } else { 0.0 },
            windfree: self.windfree.value,
            display: (connected && self.field("display") == "on").then(|| shown(self.field("temp"), "")),
            flow_seconds: seconds,
            flow_strength: strength,
            sway_vertical: matches!(swing, "vertical" | "all"),
            sway_horizontal: matches!(swing, "horizontal" | "all"),
            light: self.palette.light,
        }
    }

    /// Draws the frame: the still layer only when something changed, the moving parts on every call.
    pub fn render(&mut self) -> &Pixmap {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        let moved = self.power.step(elapsed) | self.windfree.step(elapsed);
        let (width, height) = self.pixel_size();
        let scale = Transform::from_scale(self.scale, self.scale);
        let screen = self.screen();
        let base = match self.base.take() {
            Some(base) if !(self.dirty || moved) => base,
            _ => {
                let mut base = Pixmap::new(width, height).expect("tamanho do painel");
                let mut canvas = Canvas { pixmap: &mut base, transform: scale };
                match screen {
                    Screen::Main => self.draw_main(&mut canvas),
                    Screen::Settings => self.draw_settings(&mut canvas),
                    _ => self.draw_connect(&mut canvas, screen),
                }
                base
            }
        };
        self.dirty = false;
        let mut frame = self.frame.take().filter(|frame| frame.width() == width && frame.height() == height).unwrap_or_else(|| Pixmap::new(width, height).expect("tamanho do painel"));
        frame.data_mut().copy_from_slice(base.data());
        self.base = Some(base);
        {
            let seconds = self.clock.elapsed().as_secs_f32();
            let mut canvas = Canvas { pixmap: &mut frame, transform: scale };
            let turn = (seconds * 400.0) % 360.0;
            match screen {
                Screen::Main => {
                    let look = self.look();
                    let mut air = Canvas { pixmap: canvas.pixmap, transform: scale.pre_concat(Scene::transform(0.0, HERO_Y, HERO_HEIGHT, WIDTH)) };
                    self.scene.draw_air(&mut air, &look, seconds);
                    if self.spinning() {
                        canvas.icon_turned("loader-circle", 26.0, STATUS_Y, 15.0, self.palette.muted, turn);
                    }
                    if let Some(menu) = self.menu {
                        self.draw_menu(&mut canvas, menu);
                    }
                }
                Screen::Waiting => {
                    let (x, y, _, height) = PRIMARY;
                    canvas.icon_turned("loader-circle", x + 24.0, y + height / 2.0, 18.0, self.palette.muted, turn);
                }
                _ => {}
            }
        }
        self.frame.insert(frame)
    }

    fn draw_background(&self, canvas: &mut Canvas, accent_glow: f32) {
        let p = self.palette;
        let full = rounded(0.0, 0.0, WIDTH, HEIGHT, 0.0);
        canvas.fill(&full, &solid(p.background));
        canvas.fill(&full, &paint(radial(WIDTH / 2.0, -60.0, 340.0, 260.0, &[(0.0, p.glow), (1.0, p.glow.alpha(0.0))])));
        let glow = self.tint().alpha(accent_glow);
        if glow.3 > 0 {
            canvas.fill(&full, &paint(radial(WIDTH / 2.0, 150.0, 250.0, 140.0, &[(0.0, glow), (1.0, glow.alpha(0.0))])));
        }
    }

    fn hovered(&self, hit: Hit) -> bool {
        self.hover == Some(hit) && self.enabled(hit)
    }

    fn draw_hero(&mut self, canvas: &mut Canvas) {
        let mut hero = Canvas { pixmap: canvas.pixmap, transform: canvas.transform.pre_concat(Scene::transform(0.0, HERO_Y, HERO_HEIGHT, WIDTH)) };
        let look = self.look();
        self.scene.draw_unit(&mut hero, &look);
    }

    fn draw_main(&mut self, canvas: &mut Canvas) {
        let p = self.palette;
        self.draw_background(canvas, 0.08 * self.power.value);
        let fonts = fonts();
        let loading = self.device().is_none();
        let accent = self.accent();

        // Header: name, readings and power.
        let name = self.field("nome").as_str().unwrap_or("Ar-condicionado");
        canvas.text(&fonts.fit(name, Style::Semibold, 15.0, 260.0), Style::Semibold, 15.0, 18.0, 30.0, p.text, Align::Left);
        let mut chip_x = 18.0;
        for (icon, value) in [("thermometer", shown(self.field("ambiente"), "°")), ("droplet", shown(self.field("umidade"), "%"))] {
            let width = 30.0 + fonts.measure(&value, Style::Semibold, 12.5) + 10.0;
            canvas.card(chip_x, 40.0, width, 24.0, 12.0, p.card, p.line);
            canvas.icon(icon, chip_x + 15.0, 52.0, 13.0, p.muted);
            canvas.text(&value, Style::Semibold, 12.5, chip_x + 26.0, 56.5, p.text, Align::Left);
            chip_x += width + 6.0;
        }
        self.draw_power(canvas);
        self.draw_hero(canvas);

        // Setpoint with steppers.
        for (hit, center, icon) in [(Hit::TempDown, TEMP_DOWN, "minus"), (Hit::TempUp, TEMP_UP, "plus")] {
            let fill = if self.hovered(hit) { p.card_hover } else { p.card };
            canvas.fill(&circle(center.0, center.1, 22.0), &solid(fill));
            canvas.stroke(&circle(center.0, center.1, 21.5), &solid(p.line), 1.0);
            canvas.icon(icon, center.0, center.1, 18.0, if self.enabled(hit) { p.text } else { p.faint });
        }
        let number = self.temp().map_or_else(|| "-".to_owned(), |temp| temp.to_string());
        let number_color = if self.is_on() && self.mode() != Some("wind") { p.text } else { p.muted };
        let number_width = fonts.measure(&number, Style::Display, 66.0);
        let unit_width = fonts.measure("°C", Style::Light, 19.0);
        let left = WIDTH / 2.0 - (number_width + 3.0 + unit_width) / 2.0;
        canvas.text(&number, Style::Display, 66.0, left, CLIMATE_Y + 23.0, number_color, Align::Left);
        canvas.text("°C", Style::Light, 19.0, left + number_width + 3.0, CLIMATE_Y - 6.0, p.muted, Align::Left);

        self.draw_slider(canvas, accent);
        self.draw_modes(canvas, accent);
        self.draw_tiles(canvas, accent);
        self.draw_status(canvas, accent);

        if loading {
            // Controls wait for the first reading, under a veil of the background.
            let veil = solid(p.background.alpha(0.6));
            for rect in [(18.0, CLIMATE_Y - 24.0, 336.0, 48.0), SLIDER_HIT, MODES, (14.0, TILES_Y, 344.0, SWITCHES_Y + SWITCH_HEIGHT - TILES_Y)] {
                canvas.fill(&rounded(rect.0 - 2.0, rect.1 - 2.0, rect.2 + 4.0, rect.3 + 4.0, 16.0), &veil);
            }
        }
    }

    fn draw_power(&self, canvas: &mut Canvas) {
        let p = self.palette;
        let (cx, cy) = HEADER_POWER;
        let on = self.power.value;
        let tint = self.tint();
        canvas.fill(&circle(cx, cy, 32.0), &paint(radial(cx, cy, 32.0, 32.0, &[(0.45, tint.alpha(0.35 * on)), (1.0, tint.alpha(0.0))])));
        let idle = if self.hovered(Hit::Power) { p.card_hover } else { p.card };
        canvas.fill(&circle(cx, cy, 21.0), &solid(idle.mix(tint, on)));
        canvas.stroke(&circle(cx, cy, 20.5), &solid(p.line.mix(tint.alpha(0.0), on)), 1.0);
        let icon = if self.is_on() { on_accent(tint) } else if self.hovered(Hit::Power) { p.text } else { p.muted };
        canvas.icon("power", cx, cy, 18.0, icon);
    }

    /// A thin track with a round knob, like the Windows 11 slider.
    fn draw_slider(&self, canvas: &mut Canvas, accent: Rgba) {
        let p = self.palette;
        let (left, right, y) = SLIDER;
        let value = self.temp().unwrap_or(23) as f32;
        let knob = slider_x(value);
        canvas.fill(&rounded(left, y - 2.0, right - left, 4.0, 2.0), &solid(p.track));
        canvas.fill(&rounded(left, y - 2.0, knob - left, 4.0, 2.0), &solid(accent));
        for degree in TEMP_MIN..=TEMP_MAX {
            let x = slider_x(degree as f32);
            let major = degree == TEMP_MIN || degree == TEMP_MAX || degree == 23;
            canvas.fill(&circle(x, y + 11.0, if major { 1.5 } else { 1.0 }), &solid(if major { p.muted } else { p.faint }));
        }
        let active = self.drag.is_some() || self.hovered(Hit::Slider);
        canvas.fill(&circle(knob, y + 1.0, 11.0), &solid(Rgba(0, 0, 0, if p.light { 40 } else { 90 })));
        canvas.fill(&circle(knob, y, 10.5), &solid(if p.light { Rgba::hex(0xffffff) } else { Rgba::hex(0x45474e) }));
        canvas.stroke(&circle(knob, y, 10.0), &solid(p.line), 1.0);
        canvas.fill(&circle(knob, y, if active { 6.5 } else { 5.0 }), &solid(accent));
        for degree in [TEMP_MIN, 23, TEMP_MAX] {
            canvas.text(&format!("{degree}°"), Style::Regular, 11.0, slider_x(degree as f32), y + 28.0, p.faint, Align::Center);
        }
    }

    fn draw_modes(&self, canvas: &mut Canvas, accent: Rgba) {
        let p = self.palette;
        let (x, y, width, height) = MODES;
        canvas.card(x, y, width, height, 17.0, p.card, p.line);
        let modes = self.visible_modes();
        let current = self.mode();
        for (index, choice) in modes.iter().enumerate() {
            let rect = self.mode_rect(index, modes.len());
            let selected = Some(choice.value) == current;
            let hovered = self.hovered(Hit::Mode(index));
            if selected {
                canvas.fill(&rounded(rect.0, rect.1, rect.2, rect.3, 12.0), &solid(accent));
            } else if hovered {
                canvas.fill(&rounded(rect.0, rect.1, rect.2, rect.3, 12.0), &solid(p.card_hover));
            }
            let color = if selected { on_accent(accent) } else if hovered { p.text } else { p.muted };
            let center = rect.0 + rect.2 / 2.0;
            canvas.icon(choice.icon, center, rect.1 + 18.0, 17.0, color);
            canvas.text(choice.short, Style::Medium, 11.5, center, rect.1 + 42.0, color, Align::Center);
        }
    }

    fn draw_tiles(&self, canvas: &mut Canvas, accent: Rgba) {
        let p = self.palette;
        let fonts = fonts();
        let extra = self.extra();
        let fan = self.field("vento").as_str();
        let swing = self.field("oscilar").as_str();
        let short = |choices: &[Choice], value: Option<&str>| {
            choices.iter().find(|choice| Some(choice.value) == value).map_or_else(|| value.unwrap_or("-").to_owned(), |choice| choice.short.to_owned())
        };
        let swing_icon = SWING_CHOICES.iter().find(|choice| Some(choice.value) == swing).map_or("minus", |choice| choice.icon);

        // Fan and direction: wide tiles that open their option menus.
        for (index, (icon, label, value)) in [("fan", "Vento", short(&FAN_CHOICES, fan)), (swing_icon, "Direção", short(&SWING_CHOICES, swing))].into_iter().enumerate() {
            let (x, y, width, height) = tile_rect(index);
            let enabled = self.enabled(Hit::Tile(index));
            let expanded = self.menu.is_some_and(|menu| menu.tile() == index);
            let fill = if expanded || self.hovered(Hit::Tile(index)) { p.card_hover } else { p.card };
            canvas.card(x, y, width, height, 16.0, fill, p.line);
            let dim = |color: Rgba| if enabled { color } else { color.alpha(0.4) };
            canvas.fill(&rounded(x + 10.0, y + 12.0, 32.0, 32.0, 10.0), &solid(dim(p.track.alpha(if p.light { 0.6 } else { 0.8 }))));
            canvas.icon(icon, x + 26.0, y + 28.0, 16.0, dim(p.text));
            canvas.text(label, Style::Regular, 11.5, x + 52.0, y + 24.0, dim(p.muted), Align::Left);
            let value = if self.device().is_some() { value } else { "-".into() };
            canvas.text(&fonts.fit(&value, Style::Semibold, 14.0, width - 82.0), Style::Semibold, 14.0, x + 52.0, y + 42.0, dim(p.text), Align::Left);
            canvas.icon_turned("chevron-up", x + width - 18.0, y + 28.0, 14.0, dim(p.faint), if expanded { 180.0 } else { 0.0 });
        }

        // WindFree, sleep and turbo: switches that fill with the accent while they run.
        let switches = [("feather", "WindFree", is_windfree(extra)), ("moon", "Sono", is_sleep(extra)), ("rocket", "Turbo", is_turbo(extra))];
        for (offset, (icon, label, on)) in switches.into_iter().enumerate() {
            let index = offset + 2;
            let (x, y, width, height) = tile_rect(index);
            let enabled = self.enabled(Hit::Tile(index));
            let active = on && enabled;
            let hovered = self.hovered(Hit::Tile(index));
            let fill = if active { accent } else if hovered { p.card_hover } else { p.card };
            canvas.card(x, y, width, height, 14.0, fill, if active { accent } else { p.line });
            let ink = if active { on_accent(accent) } else if enabled { p.text } else { p.text.alpha(0.4) };
            let label_width = fonts.measure(label, Style::Medium, 13.5);
            let start = x + width / 2.0 - (label_width + 24.0) / 2.0;
            canvas.icon(icon, start + 8.0, y + height / 2.0, 16.0, ink);
            canvas.text(label, Style::Medium, 13.5, start + 24.0, y + height / 2.0 + 5.0, ink, Align::Left);
        }
    }

    fn draw_status(&self, canvas: &mut Canvas, accent: Rgba) {
        let p = self.palette;
        let status = self.status();
        let (text, retry) = self.status_layout(&status);
        let color = if status.tone == Tone::Error { p.danger } else { p.muted };
        let icon_color = match status.tone {
            Tone::Ok => accent.mix(p.text, 0.15),
            Tone::Error => p.danger,
            _ => p.muted,
        };
        if status.icon != "loader-circle" {
            canvas.icon(status.icon, 26.0, STATUS_Y, 15.0, icon_color);
        }
        canvas.text(&text, Style::Regular, 13.0, 42.0, STATUS_Y + 4.5, color, Align::Left);
        if let Some((start, end)) = retry {
            canvas.text("Tentar de novo", Style::Medium, 13.0, start, STATUS_Y + 4.5, p.text, Align::Left);
            canvas.fill(&rounded(start, STATUS_Y + 7.5, end - start, 1.0, 0.5), &solid(p.text.alpha(if self.hovered(Hit::Retry) { 0.9 } else { 0.5 })));
        }
        let (cx, cy) = SETTINGS_BUTTON;
        if self.hovered(Hit::Settings) {
            canvas.fill(&circle(cx, cy, 18.0), &solid(p.card_hover));
        }
        canvas.icon("sliders-horizontal", cx, cy, 18.0, if self.hovered(Hit::Settings) { p.text } else { p.muted });
    }

    fn draw_menu(&self, canvas: &mut Canvas, menu: Menu) {
        let p = self.palette;
        let (x, y, width, height) = self.menu_rect(menu);
        let shadow = if p.light { 0.45 } else { 1.0 };
        for (grow, alpha) in [(10.0, 0.12), (5.0, 0.2), (1.5, 0.3)] {
            canvas.fill(&rounded(x - grow, y - grow + 6.0, width + grow * 2.0, height + grow * 2.0, 15.0 + grow), &solid(Rgba(0, 0, 0, (alpha * shadow * 255.0) as u8)));
        }
        canvas.card(x, y, width, height, 15.0, if p.light { p.card } else { p.card_hover }, p.line);
        let current = self.field(menu.key()).as_str();
        let accent = self.accent();
        for (index, choice) in self.menu_choices(menu).into_iter().enumerate() {
            let row_y = y + 6.0 + index as f32 * MENU_ROW;
            let selected = Some(choice.value) == current;
            let hovered = self.hover == Some(Hit::Item(index));
            if hovered {
                canvas.fill(&rounded(x + 6.0, row_y, width - 12.0, MENU_ROW, 10.0), &solid(p.track.alpha(0.5)));
            }
            let color = if selected || hovered { p.text } else { p.muted };
            canvas.icon(choice.icon, x + 24.0, row_y + MENU_ROW / 2.0, 16.0, if selected { accent } else { color });
            canvas.text(choice.label, Style::Regular, 14.0, x + 42.0, row_y + 23.0, color, Align::Left);
            if selected {
                canvas.icon("check", x + width - 22.0, row_y + MENU_ROW / 2.0, 16.0, accent);
            }
        }
    }

    fn draw_switch(&self, canvas: &mut Canvas, x: f32, cy: f32, on: bool, enabled: bool) {
        let p = self.palette;
        let accent = self.tint();
        let track = if on { accent } else { p.track };
        let track = if enabled { track } else { track.alpha(0.4) };
        canvas.fill(&rounded(x, cy - 11.0, 40.0, 22.0, 11.0), &solid(track));
        let knob_x = if on { x + 29.0 } else { x + 11.0 };
        canvas.fill(&circle(knob_x, cy + 0.8, 8.0), &solid(Rgba(0, 0, 0, 50)));
        canvas.fill(&circle(knob_x, cy, 8.0), &solid(Rgba::hex(0xffffff)));
    }

    fn draw_settings(&mut self, canvas: &mut Canvas) {
        let p = self.palette;
        self.draw_background(canvas, 0.0);
        let fonts = fonts();
        let (bx, by) = BACK_BUTTON;
        if self.hovered(Hit::Back) {
            canvas.fill(&circle(bx, by, 18.0), &solid(p.card_hover));
        }
        canvas.icon("chevron-left", bx, by, 20.0, p.text);
        canvas.text("Ajustes", Style::Medium, 17.0, 58.0, 31.0, p.text, Align::Left);
        let name = self.field("nome").as_str().unwrap_or("");
        canvas.text(&fonts.fit(name, Style::Regular, 12.0, 280.0), Style::Regular, 12.0, 58.0, 47.0, p.muted, Align::Left);

        let demo = self.account() == "demo";
        let group = |canvas: &mut Canvas, first: usize, rows: usize| {
            let (x, y, width, _) = settings_row(first);
            canvas.card(x, y, width, ROW_HEIGHT * rows as f32, 16.0, p.card, p.line);
            for row in 1..rows {
                canvas.fill(&rounded(x + 14.0, y + ROW_HEIGHT * row as f32, width - 28.0, 1.0, 0.5), &solid(p.line));
            }
        };
        group(canvas, 0, 3);
        group(canvas, 3, 2);
        group(canvas, 5, 1);
        if !demo {
            group(canvas, 6, 2);
        }
        let hover_row = |canvas: &mut Canvas, rect: (f32, f32, f32, f32)| {
            canvas.fill(&rounded(rect.0 + 4.0, rect.1 + 4.0, rect.2 - 8.0, rect.3 - 8.0, 12.0), &solid(p.card_hover));
        };

        let has_device = self.device().is_some();
        for (index, (key, icon, title, note)) in SETTING_ROWS.iter().enumerate() {
            let rect @ (x, y, width, height) = settings_row(index);
            let cy = y + height / 2.0;
            if self.hovered(Hit::Toggle(index)) {
                hover_row(canvas, rect);
            }
            canvas.icon(icon, x + 24.0, cy, 18.0, p.muted);
            match note {
                Some(note) => {
                    canvas.text(title, Style::Regular, 14.0, x + 46.0, cy - 2.0, p.text, Align::Left);
                    canvas.text(note, Style::Regular, 12.0, x + 46.0, cy + 14.0, p.muted, Align::Left);
                }
                None => canvas.text(title, Style::Regular, 14.0, x + 46.0, cy + 5.0, p.text, Align::Left),
            }
            self.draw_switch(canvas, x + width - 54.0, cy, self.field(key) == "on", has_device);
        }

        // Filter wear and energy use.
        let (x, y, width, height) = settings_row(3);
        let cy = y + height / 2.0;
        canvas.icon("air-vent", x + 24.0, cy, 18.0, p.muted);
        canvas.text("Filtro", Style::Regular, 14.0, x + 46.0, cy - 6.0, p.text, Align::Left);
        let usage = self.field("filtroUso").as_f64();
        let filter_status = self.field("filtroStatus").as_str();
        let note = match (filter_status, usage, self.field("filtroAlerta").as_f64()) {
            (Some("wash"), ..) => "Hora de lavar".to_owned(),
            (Some("replace"), ..) => "Hora de trocar".to_owned(),
            (_, Some(usage), Some(alert)) => format!("Lavar daqui a {} h de uso", (((100.0 - usage) / 100.0) * alert).max(0.0).round()),
            _ => "-".to_owned(),
        };
        canvas.text(&note, Style::Regular, 12.0, x + 46.0, cy + 9.0, p.muted, Align::Left);
        let meter_width = width - 46.0 - 18.0;
        canvas.fill(&rounded(x + 46.0, cy + 15.0, meter_width, 4.0, 2.0), &solid(p.track));
        let alert = filter_status.is_some_and(|status| status != "normal");
        let filled = meter_width * (usage.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0) as f32;
        if filled > 0.5 {
            canvas.fill(&rounded(x + 46.0, cy + 15.0, filled, 4.0, 2.0), &solid(if alert { p.danger } else { self.tint() }));
        }

        let (x, y, width, height) = settings_row(4);
        let cy = y + height / 2.0;
        canvas.icon("zap", x + 24.0, cy, 18.0, p.muted);
        canvas.text("Consumo", Style::Regular, 14.0, x + 46.0, cy - 2.0, p.text, Align::Left);
        let total = self.field("energia").as_f64().map_or_else(|| "-".to_owned(), |kwh| format!("{} kWh no total", thousands(kwh.round() as i64)));
        // The unit sends power every few minutes, so the reading says how old it is.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs() as i64);
        let measured = self.field("potenciaFim").as_str().and_then(epoch_seconds).map(|end| (now - end).max(0) / 60);
        let energy = match measured {
            Some(0) => format!("{total} · medido agora"),
            Some(minutes) => format!("{total} · medido há {minutes} min"),
            None => total,
        };
        canvas.text(&energy, Style::Regular, 12.0, x + 46.0, cy + 14.0, p.muted, Align::Left);
        canvas.text(&shown(self.field("potencia"), " W"), Style::Semibold, 15.0, x + width - 18.0, cy + 5.0, p.text, Align::Right);

        let rect @ (x, y, width, height) = settings_row(5);
        let cy = y + height / 2.0;
        if self.hovered(Hit::Toggle(3)) {
            hover_row(canvas, rect);
        }
        canvas.icon("monitor", x + 24.0, cy, 18.0, p.muted);
        canvas.text("Abrir com o Windows", Style::Regular, 14.0, x + 46.0, cy + 5.0, p.text, Align::Left);
        self.draw_switch(canvas, x + width - 54.0, cy, self.autostart, true);

        if demo {
            return;
        }
        for (row, hit, icon, title, color) in [
            (6, Hit::ChangeDevice, "arrow-left-right", "Trocar de aparelho", p.text),
            (7, Hit::Logout, "log-out", "Sair da conta Samsung", p.danger),
        ] {
            let rect @ (x, y, width, height) = settings_row(row);
            let cy = y + height / 2.0;
            if self.hovered(hit) {
                hover_row(canvas, rect);
            }
            canvas.icon(icon, x + 24.0, cy, 18.0, if color == p.text { p.muted } else { color });
            canvas.text(title, Style::Regular, 14.0, x + 46.0, cy + 5.0, color, Align::Left);
            canvas.icon("chevron-right", x + width - 22.0, cy, 15.0, p.faint);
        }
    }

    fn draw_button(&self, canvas: &mut Canvas, rect: (f32, f32, f32, f32), icon: Option<&str>, label: &str, primary: bool, hit: Hit) {
        let p = self.palette;
        let (x, y, width, height) = rect;
        let accent = tint_for(Some("cool"), p.light);
        let hovered = self.hovered(hit);
        let (fill, ink) = if primary {
            (if hovered { accent.mix(Rgba::hex(0xffffff), 0.12) } else { accent }, on_accent(accent))
        } else {
            (if hovered { p.card_hover } else { p.card }, p.text)
        };
        canvas.card(x, y, width, height, 14.0, fill, if primary { fill } else { p.line });
        let fonts = fonts();
        let text_width = fonts.measure(label, Style::Semibold, 14.0);
        let icon_space = if icon.is_some() { 26.0 } else { 0.0 };
        let start = x + width / 2.0 - (text_width + icon_space) / 2.0;
        if let Some(icon) = icon {
            canvas.icon(icon, start + 9.0, y + height / 2.0, 17.0, ink);
        }
        canvas.text(label, Style::Semibold, 14.0, start + icon_space, y + height / 2.0 + 5.0, ink, Align::Left);
    }

    /// Sign in, wait for the browser, pick a unit or explain an empty account.
    fn draw_connect(&mut self, canvas: &mut Canvas, screen: Screen) {
        let p = self.palette;
        self.draw_background(canvas, 0.0);
        canvas.text("Painel do Ar", Style::Semibold, 15.0, 18.0, 30.0, p.text, Align::Left);
        canvas.text("Conta Samsung SmartThings", Style::Regular, 12.0, 18.0, 47.0, p.muted, Align::Left);
        self.draw_hero(canvas);

        let (title, body): (&str, [&str; 2]) = match screen {
            Screen::SignIn => ("Conecte sua conta Samsung", ["O Painel do Ar usa o SmartThings para ler", "e controlar o seu ar-condicionado."]),
            Screen::Waiting => ("Continue no navegador", ["Entre na sua conta Samsung na aba que abriu.", "O painel se conecta sozinho quando terminar."]),
            Screen::Choose => ("Escolha o ar-condicionado", ["Sua conta tem mais de um aparelho.", "Dá para trocar depois nos ajustes."]),
            _ => ("Nenhum ar-condicionado", ["Esta conta não tem um ar-condicionado no", "SmartThings. Confira no app do celular."]),
        };
        canvas.text(title, Style::Semibold, 19.0, WIDTH / 2.0, CONNECT_TITLE_Y, p.text, Align::Center);
        canvas.text(body[0], Style::Regular, 13.0, WIDTH / 2.0, CONNECT_TITLE_Y + 24.0, p.muted, Align::Center);
        canvas.text(body[1], Style::Regular, 13.0, WIDTH / 2.0, CONNECT_TITLE_Y + 42.0, p.muted, Align::Center);

        let (footer_icon, footer, footer_color) = match screen {
            Screen::SignIn => {
                self.draw_button(canvas, PRIMARY, Some("log-in"), "Entrar com a conta Samsung", true, Hit::Primary);
                let (x, y, width, _) = SECONDARY_WIDE;
                canvas.card(x, y, width, 2.0 * ROW_HEIGHT, 16.0, p.card, p.line);
                canvas.fill(&rounded(x + 14.0, y + ROW_HEIGHT, width - 28.0, 1.0, 0.5), &solid(p.line));
                for (index, (icon, row_title, note)) in [
                    ("lock", "Fica só neste computador", "O acesso é guardado criptografado no Windows"),
                    ("refresh-cw", "Sem precisar entrar toda hora", "O login se renova sozinho enquanto você usa"),
                ]
                .into_iter()
                .enumerate()
                {
                    let cy = y + ROW_HEIGHT * index as f32 + ROW_HEIGHT / 2.0;
                    canvas.icon(icon, x + 24.0, cy, 17.0, p.muted);
                    canvas.text(row_title, Style::Regular, 14.0, x + 46.0, cy - 2.0, p.text, Align::Left);
                    canvas.text(note, Style::Regular, 12.0, x + 46.0, cy + 14.0, p.muted, Align::Left);
                }
                if self.view["loginError"] == true {
                    ("triangle-alert", "Não deu para entrar. Tente de novo.", p.danger)
                } else {
                    ("external-link", "O login abre no seu navegador", p.muted)
                }
            }
            Screen::Waiting => {
                let (x, y, width, height) = PRIMARY;
                canvas.card(x, y, width, height, 14.0, p.card, p.line);
                canvas.text("Aguardando o login…", Style::Semibold, 14.0, x + 44.0, y + height / 2.0 + 5.0, p.muted, Align::Left);
                self.draw_button(canvas, SECONDARY_LEFT, Some("external-link"), "Abrir de novo", false, Hit::Reopen);
                self.draw_button(canvas, SECONDARY_RIGHT, Some("x"), "Cancelar", false, Hit::Cancel);
                ("clock", "A aba não abriu? Use Abrir de novo.", p.muted)
            }
            Screen::Choose => {
                let fonts = fonts();
                for (index, (_, name)) in self.devices().into_iter().enumerate() {
                    let (x, y, width, height) = device_row(index);
                    let hovered = self.hovered(Hit::Device(index));
                    canvas.card(x, y, width, height, 14.0, if hovered { p.card_hover } else { p.card }, p.line);
                    canvas.fill(&rounded(x + 10.0, y + 10.0, 30.0, 30.0, 10.0), &solid(p.track.alpha(0.8)));
                    canvas.icon("air-vent", x + 25.0, y + 25.0, 16.0, p.text);
                    let name = name.unwrap_or_else(|| "Ar-condicionado".to_owned());
                    canvas.text(&fonts.fit(&name, Style::Semibold, 14.0, width - 90.0), Style::Semibold, 14.0, x + 52.0, y + 30.0, p.text, Align::Left);
                    canvas.icon("chevron-right", x + width - 20.0, y + 25.0, 16.0, if hovered { p.text } else { p.faint });
                }
                ("arrow-left-right", "Escolha um para controlar pelo painel", p.muted)
            }
            _ => {
                self.draw_button(canvas, PRIMARY, Some("search"), "Procurar de novo", true, Hit::Primary);
                self.draw_button(canvas, SECONDARY_WIDE, Some("log-out"), "Entrar com outra conta", false, Hit::OtherAccount);
                ("cloud-off", "O ar precisa estar no app SmartThings", p.muted)
            }
        };
        canvas.icon(footer_icon, 26.0, STATUS_Y, 15.0, footer_color);
        canvas.text(footer, Style::Regular, 13.0, 42.0, STATUS_Y + 4.5, footer_color, Align::Left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_view() -> Value {
        json!({
            "device": {
                "nome": "Ar da sala", "ligado": true, "temp": 23, "modo": "cool", "vento": "auto", "extra": "windFree",
                "oscilar": "vertical", "display": "on", "bipe": "off", "autoclean": "on", "filtroAlerta": 500, "filtroUso": 37,
                "filtroStatus": "normal", "potencia": 640, "energia": 812.4, "ambiente": 27, "umidade": 58,
                "modos": ["cool", "dry", "wind", "auto", "heat"], "extras": ["off", "windFree", "windFreeSleep", "sleep", "speed"],
            },
            "syncing": false, "held": [], "error": null, "updatedAt": 0, "account": "ready", "devices": [], "loginError": false,
        })
    }

    fn account_view(account: &str) -> Value {
        json!({
            "device": null, "syncing": false, "held": [], "error": null, "account": account, "loginError": false,
            "devices": [{ "id": "a", "nome": "Ar da sala" }, { "id": "b", "nome": "Ar do quarto" }],
        })
    }

    fn click(panel: &mut Panel, x: f32, y: f32) -> Vec<Action> {
        panel.pointer_down(x, y);
        panel.pointer_up(x, y)
    }

    fn center(rect: (f32, f32, f32, f32)) -> (f32, f32) {
        (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0)
    }

    #[test]
    fn controls_send_the_expected_changes() {
        let mut panel = Panel::new();
        panel.set_view(demo_view());
        assert_eq!(click(&mut panel, HEADER_POWER.0, HEADER_POWER.1), [Action::Set("ligado", json!(false))]);
        assert_eq!(click(&mut panel, TEMP_UP.0, TEMP_UP.1), [Action::Set("temp", json!(24))]);
        let (x, y) = center(tile_rect(3));
        assert_eq!(click(&mut panel, x, y), [Action::Set("extra", json!("windFreeSleep"))]);
        let (x, y) = center(panel.mode_rect(4, 5));
        assert_eq!(click(&mut panel, x, y), [Action::Set("modo", json!("heat"))]);
        assert_eq!(panel.wheel(-120), [Action::Set("temp", json!(22))]);
    }

    #[test]
    fn dragging_the_slider_sends_one_change_on_release() {
        let mut panel = Panel::new();
        panel.set_view(demo_view());
        let y = SLIDER.2;
        panel.pointer_down(slider_x(23.0), y);
        panel.pointer_move(slider_x(27.0), y);
        assert_eq!(panel.temp(), Some(27));
        assert_eq!(panel.pointer_up(slider_x(27.0), y), [Action::Set("temp", json!(27))]);
    }

    #[test]
    fn menus_offer_only_supported_options() {
        let mut panel = Panel::new();
        let mut view = demo_view();
        view["device"]["ventos"] = json!(["auto", "low", "high"]);
        panel.set_view(view);
        let (x, y) = center(tile_rect(0));
        assert!(click(&mut panel, x, y).is_empty());
        assert_eq!(panel.menu, Some(Menu::Fan));
        let rect = panel.menu_rect(Menu::Fan);
        let third = rect.1 + 6.0 + MENU_ROW * 2.5;
        assert_eq!(click(&mut panel, rect.0 + 40.0, third), [Action::Set("vento", json!("high"))]);
        assert_eq!(panel.menu, None);
    }

    #[test]
    fn unsupported_extras_are_disabled() {
        let mut panel = Panel::new();
        let mut view = demo_view();
        view["device"]["extra"] = Value::Null;
        view["device"]["extras"] = Value::Null;
        panel.set_view(view);
        let (x, y) = center(tile_rect(2));
        assert!(click(&mut panel, x, y).is_empty());
    }

    #[test]
    fn turbo_switches_the_fast_mode_on_and_off() {
        let mut panel = Panel::new();
        let mut view = demo_view();
        view["device"]["extras"] = json!(["off", "windFree", "sleep", "speed"]);
        panel.set_view(view.clone());
        let (x, y) = center(tile_rect(4));
        assert_eq!(click(&mut panel, x, y), [Action::Set("extra", json!("speed"))]);
        view["device"]["extra"] = json!("speed");
        panel.set_view(view);
        assert_eq!(click(&mut panel, x, y), [Action::Set("extra", json!("off"))]);
    }

    #[test]
    fn reads_report_times() {
        assert_eq!(epoch_seconds("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(epoch_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(epoch_seconds("2026-10-12T00:24:03.159Z"), Some(1_791_764_643));
        assert_eq!(epoch_seconds("ontem"), None);
    }

    #[test]
    fn escape_closes_layers_before_hiding() {
        let mut panel = Panel::new();
        panel.set_view(demo_view());
        click(&mut panel, SETTINGS_BUTTON.0, SETTINGS_BUTTON.1);
        assert!(panel.settings);
        assert!(panel.escape().is_empty());
        assert_eq!(panel.escape(), [Action::Hide]);
    }

    #[test]
    fn account_screens_drive_the_connection() {
        let mut panel = Panel::new();
        panel.set_view(account_view("signedOut"));
        let (x, y) = center(PRIMARY);
        assert_eq!(click(&mut panel, x, y), [Action::Login]);
        panel.set_view(account_view("signingIn"));
        assert!(panel.keeps_open());
        let (x, y) = center(SECONDARY_RIGHT);
        assert_eq!(click(&mut panel, x, y), [Action::CancelLogin]);
        let (x, y) = center(SECONDARY_LEFT);
        assert_eq!(click(&mut panel, x, y), [Action::ReopenLogin]);
        panel.set_view(account_view("choosing"));
        let (x, y) = center(device_row(1));
        assert_eq!(click(&mut panel, x, y), [Action::Choose("b".into(), Some("Ar do quarto".into()))]);
        panel.set_view(account_view("noDevice"));
        let (x, y) = center(PRIMARY);
        assert_eq!(click(&mut panel, x, y), [Action::Refresh]);
        let (x, y) = center(SECONDARY_WIDE);
        assert_eq!(click(&mut panel, x, y), [Action::Logout]);
    }

    #[test]
    fn settings_offer_device_change_and_sign_out() {
        let mut panel = Panel::new();
        panel.set_view(demo_view());
        click(&mut panel, SETTINGS_BUTTON.0, SETTINGS_BUTTON.1);
        let (x, y) = center(settings_row(7));
        assert_eq!(click(&mut panel, x, y), [Action::Logout]);
        click(&mut panel, SETTINGS_BUTTON.0, SETTINGS_BUTTON.1);
        let (x, y) = center(settings_row(6));
        assert_eq!(click(&mut panel, x, y), [Action::ChangeDevice]);
    }

    /// Renders the panel to target/preview for a visual check: cargo test preview -- --ignored
    #[test]
    #[ignore]
    fn preview() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("preview");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut blade = demo_view();
        blade["device"]["extra"] = json!("off");
        blade["device"]["vento"] = json!("high");
        let mut heat = demo_view();
        heat["device"]["modo"] = json!("heat");
        heat["device"]["extra"] = json!("off");
        let mut off = demo_view();
        off["device"]["ligado"] = json!(false);
        let mut failed = demo_view();
        failed["error"] = json!({ "kind": "failed", "message": "x" });
        let cases = [
            ("windfree", demo_view()),
            ("vento", blade),
            ("quente", heat),
            ("desligado", off),
            ("erro", failed),
            ("conectando", account_view("starting")),
            ("entrar", account_view("signedOut")),
            ("aguardando", account_view("signingIn")),
            ("escolher", account_view("choosing")),
            ("sem-aparelho", account_view("noDevice")),
        ];
        let render = |name: &str, view: &Value, light: bool, scale: f32, setup: &dyn Fn(&mut Panel)| {
            let mut panel = Panel::new();
            panel.set_light(light);
            panel.set_scale(scale);
            panel.set_view(view.clone());
            panel.hidden();
            panel.set_view(view.clone());
            setup(&mut panel);
            panel.clock -= std::time::Duration::from_millis(1700);
            let theme = if light { "claro" } else { "escuro" };
            panel.render().save_png(dir.join(format!("{name}-{theme}@{scale}.png"))).unwrap();
        };
        for (name, view) in &cases {
            for light in [false, true] {
                render(name, view, light, 1.0, &|_| {});
            }
        }
        render("vento", &cases[1].1, false, 1.5, &|_| {});
        render("detalhe", &cases[1].1, false, 3.0, &|_| {});
        render("detalhe-windfree", &cases[0].1, true, 3.0, &|_| {});
        render("menu", &demo_view(), false, 1.0, &|panel| {
            let (x, y) = center(tile_rect(1));
            click(panel, x, y);
            panel.pointer_move(200.0, panel.menu_rect(Menu::Swing).1 + 6.0 + MENU_ROW * 1.5);
        });
        for light in [false, true] {
            render("ajustes", &demo_view(), light, 1.0, &|panel| {
                click(panel, SETTINGS_BUTTON.0, SETTINGS_BUTTON.1);
            });
        }
    }
}
