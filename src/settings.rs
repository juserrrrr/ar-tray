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
    fn tooltip_shows_dashes_for_missing_readings() {
        let view = json!({ "account": "ready", "device": { "nome": "Ar da sala", "ligado": true, "temp": 23, "ambiente": 27 } });
        assert_eq!(tooltip_for(&view), "Ar da sala ligado em 23 °C\nAmbiente com 27 °C e -% de umidade");
        assert_eq!(tooltip_for(&Value::Null), APP_NAME);
        assert_eq!(tooltip_for(&json!({ "account": "signedOut" })), "Painel do Ar\nConecte sua conta Samsung");
    }
}
