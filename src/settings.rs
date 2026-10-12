//! settings.json, the Windows startup entry and the tray tooltip text.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::platform;
use crate::shell::APP_NAME;

fn read(file: &Path) -> Value {
    fs::read(file).ok().and_then(|json| serde_json::from_slice(&json).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

pub fn autostart_enabled(file: &Path) -> bool {
    read(file)["autostart"].as_bool().unwrap_or(true)
}

/// Also rewritten on every start, so the entry follows the exe if it moves.
pub fn set_autostart(file: &Path, enabled: bool, demo: bool) {
    let mut settings = read(file);
    settings["autostart"] = json!(enabled);
    if let Err(error) = fs::write(file, settings.to_string()) {
        eprintln!("não deu para salvar os ajustes: {error}");
    }
    if demo {
        return;
    }
    let result = std::env::current_exe().map_err(|error| error.to_string());
    if let Err(error) = result.and_then(|exe| platform::set_autostart(APP_NAME, &exe, enabled)) {
        eprintln!("{error}");
    }
}

/// A typical Brazilian residential price with taxes, until the user sets theirs.
const DEFAULT_TARIFF: f64 = 0.90;

pub fn tariff(file: &Path) -> f64 {
    read(file)["tarifaKwh"].as_f64().filter(|price| *price > 0.0).unwrap_or(DEFAULT_TARIFF)
}

pub fn set_tariff(file: &Path, price: f64) {
    let mut settings = read(file);
    settings["tarifaKwh"] = json!((price * 100.0).round() / 100.0);
    if let Err(error) = fs::write(file, settings.to_string()) {
        eprintln!("não deu para salvar o preço do kWh: {error}");
    }
}

/// Energy used this month and the day counting started, as "dd/mm".
pub struct MonthUse {
    pub kwh: f64,
    pub since: String,
}

/// SmartThings only reports the unit's lifetime energy, so the reading at the start of each month
/// (or the first one seen in it) is kept to tell this month's use.
pub fn track_month(file: &Path, device: &str, total_kwh: f64, today: (u16, u16, u16)) -> MonthUse {
    let (year, month, day) = today;
    let key = format!("{year:04}-{month:02}");
    let mut settings = read(file);
    let record = &settings["energia"];
    let current = record["mes"] == key.as_str() && record["aparelho"] == device && record["inicioKwh"].as_f64().is_some_and(|start| start <= total_kwh);
    if !current {
        settings["energia"] = json!({ "mes": key, "aparelho": device, "inicioKwh": total_kwh, "desde": format!("{day:02}/{month:02}") });
        if let Err(error) = fs::write(file, settings.to_string()) {
            eprintln!("não deu para salvar o consumo do mês: {error}");
        }
    }
    let record = &settings["energia"];
    MonthUse { kwh: (total_kwh - record["inicioKwh"].as_f64().unwrap_or(total_kwh)).max(0.0), since: record["desde"].as_str().unwrap_or("").to_owned() }
}

fn shown(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The tray tooltip for a bridge view.
pub fn tooltip_for(view: &Value) -> String {
    match view["account"].as_str() {
        Some("signedOut" | "signingIn") => return format!("{APP_NAME}\nConecte sua conta Samsung"),
        Some("choosing") => return format!("{APP_NAME}\nEscolha o ar-condicionado"),
        Some("noDevice") => return format!("{APP_NAME}\nNenhum ar-condicionado na conta"),
        _ => {}
    }
    let device = &view["device"];
    if !device.is_object() {
        return APP_NAME.into();
    }
    let name = device["nome"].as_str().unwrap_or("Ar-condicionado");
    let power = if device["ligado"] == true { format!("ligado em {} °C", shown(&device["temp"])) } else { "desligado".into() };
    format!("{name} {power}\nAmbiente com {} °C e {}% de umidade", shown(&device["ambiente"]), shown(&device["umidade"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_this_month_from_the_first_reading() {
        let file = std::env::temp_dir().join(format!("ar-tray-teste-{}.json", std::process::id()));
        let _ = fs::remove_file(&file);
        let first = track_month(&file, "a", 300.0, (2026, 10, 11));
        assert_eq!((first.kwh, first.since.as_str()), (0.0, "11/10"));
        let later = track_month(&file, "a", 312.5, (2026, 10, 20));
        assert_eq!((later.kwh, later.since.as_str()), (12.5, "11/10"));
        let next_month = track_month(&file, "a", 320.0, (2026, 11, 1));
        assert_eq!((next_month.kwh, next_month.since.as_str()), (0.0, "01/11"));
        let other_unit = track_month(&file, "b", 50.0, (2026, 11, 2));
        assert_eq!(other_unit.kwh, 0.0);
        assert_eq!(tariff(&file), DEFAULT_TARIFF);
        set_tariff(&file, 1.234);
        assert_eq!(tariff(&file), 1.23);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn tooltip_shows_dashes_for_missing_readings() {
        let view = json!({ "account": "ready", "device": { "nome": "Ar da sala", "ligado": true, "temp": 23, "ambiente": 27 } });
        assert_eq!(tooltip_for(&view), "Ar da sala ligado em 23 °C\nAmbiente com 27 °C e -% de umidade");
        assert_eq!(tooltip_for(&Value::Null), APP_NAME);
        assert_eq!(tooltip_for(&json!({ "account": "signedOut" })), "Painel do Ar\nConecte sua conta Samsung");
    }
}
