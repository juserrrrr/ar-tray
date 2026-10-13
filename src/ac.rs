//! Turns panel changes into SmartThings commands and reads the unit back.

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::local::{Identity, Link};
use crate::localac::{self, Found};
use crate::log;
use crate::settings;
use crate::smartthings::{Cloud, CloudError, Command, LoginControl};

pub type Device = Map<String, Value>;
pub type Changes = Vec<(String, Value)>;

const CONFIRM_ATTEMPTS: usize = 4;
const CONFIRM_INTERVAL: Duration = Duration::from_millis(1500);

pub struct Outcome {
    pub device: Device,
    /// Keys the unit has not confirmed yet.
    pub pending: Vec<String>,
}

#[derive(Debug)]
pub enum Failure {
    /// No SmartThings session, so the panel asks to sign in.
    SignedOut,
    Cancelled,
    /// More than one air conditioner on the account, as {id, nome} objects.
    Choose(Vec<Value>),
    NoDevice,
    Failed(String),
}

impl From<CloudError> for Failure {
    fn from(error: CloudError) -> Self {
        match error {
            CloudError::SignedOut => Failure::SignedOut,
            CloudError::Cancelled => Failure::Cancelled,
            CloudError::Failed(message) => Failure::Failed(message),
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Failed(message)
    }
}

pub trait Unit: Send {
    /// Sends the changes in order and returns the unit's state. No changes means a plain refresh.
    fn run(&mut self, changes: &[(String, Value)]) -> Result<Outcome, Failure>;

    fn login(&mut self) -> Result<(), Failure> {
        Ok(())
    }

    fn login_control(&self) -> Option<Arc<LoginControl>> {
        None
    }

    fn choose(&mut self, _id: &str, _name: Option<&str>) {}

    fn forget_device(&mut self) {}

    fn logout(&mut self) {}

    fn is_demo(&self) -> bool {
        false
    }

    /// Turns the home-network path on or off.
    fn set_local(&mut self, _enabled: bool) {}

    /// Looks for air conditioners on the network.
    fn find_local(&mut self) {}

    /// Picks the unit at this address for the network path.
    fn choose_local(&mut self, _addr: &str) {}

    /// The network path's state for the panel: enabled, address, found units and a status line.
    fn local_view(&self) -> Value {
        Value::Null
    }
}

enum Values {
    Range(i64, i64),
    List(&'static [&'static str]),
}

struct Setting {
    key: &'static str,
    capability: &'static str,
    command: &'static str,
    values: Values,
}

const SETTINGS: [Setting; 6] = [
    Setting { key: "temp", capability: "thermostatCoolingSetpoint", command: "setCoolingSetpoint", values: Values::Range(16, 30) },
    Setting {
        key: "modo",
        capability: "airConditionerMode",
        command: "setAirConditionerMode",
        values: Values::List(&["cool", "dry", "wind", "auto", "heat"]),
    },
    Setting {
        key: "vento",
        capability: "airConditionerFanMode",
        command: "setFanMode",
        values: Values::List(&["auto", "low", "medium", "high", "turbo"]),
    },
    Setting {
        key: "extra",
        capability: "custom.airConditionerOptionalMode",
        command: "setAcOptionalMode",
        values: Values::List(&["off", "windFree", "windFreeSleep", "sleep", "quiet", "speed"]),
    },
    Setting {
        key: "oscilar",
        capability: "fanOscillationMode",
        command: "setFanOscillationMode",
        values: Values::List(&["fixed", "vertical", "horizontal", "all"]),
    },
    Setting { key: "autoclean", capability: "custom.autoCleaningMode", command: "setAutoCleaningMode", values: Values::List(&["on", "off"]) },
];

fn plain(capability: &'static str, command: &'static str, arguments: Vec<Value>) -> Command {
    Command { capability, command, arguments }
}

/// The SmartThings command for one panel change, or None when the value is not allowed.
fn command_for(key: &str, value: &Value) -> Option<Command> {
    match (key, value) {
        ("ligado", Value::Bool(on)) => Some(plain("switch", if *on { "on" } else { "off" }, vec![])),
        // samsungce.airConditionerLighting only reports the display here; setting it goes through the
        // OCF execute call, whose option names are reversed (Light_Off lights the display).
        ("display", Value::String(state)) if state == "on" || state == "off" => {
            let option = if state == "on" { "Light_Off" } else { "Light_On" };
            Some(plain("execute", "execute", vec![json!("mode/vs/0"), json!({ "x.com.samsung.da.options": [option] })]))
        }
        ("bipe", Value::String(state)) if state == "on" || state == "off" => {
            Some(plain("audioVolume", "setVolume", vec![json!(if state == "on" { 100 } else { 0 })]))
        }
        _ => {
            let setting = SETTINGS.iter().find(|setting| setting.key == key)?;
            let allowed = match setting.values {
                Values::Range(min, max) => value.as_i64().is_some_and(|number| (min..=max).contains(&number)),
                Values::List(list) => value.as_str().is_some_and(|text| list.contains(&text)),
            };
            allowed.then(|| plain(setting.capability, setting.command, vec![value.clone()]))
        }
    }
}

/// Power on goes first and power off last, so climate settings reach a running unit.
fn ordered_commands(changes: &[(String, Value)]) -> Result<Vec<Command>, String> {
    let mut ordered: Vec<&(String, Value)> = changes.iter().filter(|(key, _)| key != "ligado").collect();
    if let Some(power) = changes.iter().find(|(key, _)| key == "ligado") {
        if power.1 == Value::Bool(true) {
            ordered.insert(0, power);
        } else {
            ordered.push(power);
        }
    }
    ordered
        .into_iter()
        .map(|(key, value)| command_for(key, value).ok_or_else(|| format!("valor inválido para {key}: {value}")))
        .collect()
}

/// Integral floats become integers so 23.0 from the API matches 23 from the panel.
fn number(value: &Value) -> Value {
    match value.as_f64() {
        Some(float) if value.is_f64() && float.fract() == 0.0 && float.abs() < 1e15 => json!(float as i64),
        _ => value.clone(),
    }
}

fn read_device(main: &Value) -> Device {
    let attribute = |capability: &str, name: &str| main[capability][name]["value"].clone();
    let report = &main["powerConsumptionReport"]["powerConsumption"]["value"];
    let energy = report["energy"].as_f64().map(|watt_hours| json!((watt_hours / 100.0).round() / 10.0));
    let fields = [
        ("ligado", json!(main["switch"]["switch"]["value"] == "on")),
        ("temp", number(&attribute("thermostatCoolingSetpoint", "coolingSetpoint"))),
        ("modo", attribute("airConditionerMode", "airConditionerMode")),
        ("vento", attribute("airConditionerFanMode", "fanMode")),
        ("extra", attribute("custom.airConditionerOptionalMode", "acOptionalMode")),
        ("oscilar", attribute("fanOscillationMode", "fanOscillationMode")),
        ("display", attribute("samsungce.airConditionerLighting", "lighting")),
        ("bipe", attribute("audioVolume", "volume").as_f64().map_or(Value::Null, |volume| json!(if volume > 0.0 { "on" } else { "off" }))),
        ("autoclean", attribute("custom.autoCleaningMode", "autoCleaningMode")),
        ("filtroAlerta", number(&attribute("samsungce.dustFilterAlarm", "alarmThreshold"))),
        ("filtroUso", number(&attribute("custom.dustFilter", "dustFilterUsage"))),
        ("filtroStatus", attribute("custom.dustFilter", "dustFilterStatus")),
        ("potencia", number(&report["power"])),
        // The unit reports power every few minutes; this is when the last report ended.
        ("potenciaFim", report["end"].clone()),
        ("energia", energy.unwrap_or(Value::Null)),
        ("ambiente", number(&attribute("temperatureMeasurement", "temperature"))),
        ("umidade", number(&attribute("relativeHumidityMeasurement", "humidity"))),
        ("modos", attribute("airConditionerMode", "supportedAcModes")),
        ("ventos", attribute("airConditionerFanMode", "supportedAcFanModes")),
        ("oscilacoes", attribute("fanOscillationMode", "supportedFanOscillationModes")),
        ("extras", attribute("custom.airConditionerOptionalMode", "supportedAcOptionalMode")),
    ];
    fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

fn unconfirmed(changes: &[(String, Value)], device: &Device) -> Vec<String> {
    changes.iter().filter(|(key, value)| device.get(key) != Some(value)).map(|(key, _)| key.clone()).collect()
}

#[derive(Clone, Serialize, Deserialize)]
struct DeviceRef {
    id: String,
    nome: Option<String>,
}

pub struct CloudUnit {
    cloud: Cloud,
    device_file: PathBuf,
    device: Option<DeviceRef>,
}

impl CloudUnit {
    /// The chosen aparelho's id and label, so the local path can carry the same identity.
    pub fn chosen(&self) -> Option<(String, Option<String>)> {
        self.device.as_ref().map(|device| (device.id.clone(), device.nome.clone()))
    }

    pub fn new(cloud: Cloud, device_file: PathBuf) -> Self {
        let device = fs::read(&device_file).ok().and_then(|json| serde_json::from_slice(&json).ok());
        Self { cloud, device_file, device }
    }

    /// A single air conditioner is picked on its own; with several, the panel asks which one.
    fn device(&mut self) -> Result<DeviceRef, Failure> {
        if let Some(device) = &self.device {
            return Ok(device.clone());
        }
        let is_air_conditioner = |device: &&Value| {
            device["components"].as_array().into_iter().flatten().any(|component| {
                component["capabilities"].as_array().into_iter().flatten().any(|capability| capability["id"] == "airConditionerMode")
            })
        };
        let devices = self.cloud.devices()?;
        let found: Vec<DeviceRef> = devices
            .iter()
            .filter(is_air_conditioner)
            .filter_map(|device| {
                Some(DeviceRef {
                    id: device["deviceId"].as_str()?.to_owned(),
                    nome: device["label"].as_str().or(device["name"].as_str()).map(str::to_owned),
                })
            })
            .collect();
        match found.as_slice() {
            [] => Err(Failure::NoDevice),
            [only] => {
                let only = only.clone();
                self.save(&only);
                Ok(only)
            }
            many => Err(Failure::Choose(many.iter().map(|device| json!({ "id": device.id, "nome": device.nome })).collect())),
        }
    }

    fn save(&mut self, device: &DeviceRef) {
        if let Ok(json) = serde_json::to_vec_pretty(device) {
            let _ = fs::write(&self.device_file, json);
        }
        self.device = Some(device.clone());
    }

    fn read(&mut self, device: &DeviceRef) -> Result<Device, Failure> {
        let mut state = read_device(&self.cloud.status(&device.id)?);
        state.insert("id".into(), json!(device.id));
        state.insert("nome".into(), json!(device.nome));
        Ok(state)
    }
}

impl Unit for CloudUnit {
    fn run(&mut self, changes: &[(String, Value)]) -> Result<Outcome, Failure> {
        let commands = ordered_commands(changes)?;
        let device = self.device()?;
        for command in &commands {
            self.cloud.command(&device.id, command)?;
        }
        let mut state = self.read(&device)?;
        for _ in 1..CONFIRM_ATTEMPTS {
            if unconfirmed(changes, &state).is_empty() {
                break;
            }
            sleep(CONFIRM_INTERVAL);
            state = self.read(&device)?;
        }
        Ok(Outcome { pending: unconfirmed(changes, &state), device: state })
    }

    fn login(&mut self) -> Result<(), Failure> {
        Ok(self.cloud.login()?)
    }

    fn login_control(&self) -> Option<Arc<LoginControl>> {
        Some(self.cloud.control())
    }

    fn choose(&mut self, id: &str, name: Option<&str>) {
        self.save(&DeviceRef { id: id.to_owned(), nome: name.map(str::to_owned) });
    }

    fn forget_device(&mut self) {
        self.device = None;
        let _ = fs::remove_file(&self.device_file);
    }

    fn logout(&mut self) {
        self.cloud.logout();
        self.forget_device();
    }
}

/// A fake unit for trying the panel without a SmartThings account (`--demo`).
pub struct DemoUnit {
    device: Device,
}

impl DemoUnit {
    pub fn new() -> Self {
        let device = json!({
            "nome": "Ar da sala (demonstração)", "ligado": true, "temp": 23, "modo": "cool", "vento": "auto",
            "extra": "windFree", "oscilar": "vertical", "display": "on", "bipe": "off", "autoclean": "on",
            "filtroAlerta": 500, "filtroUso": 37, "filtroStatus": "normal", "potencia": 640, "energia": 812.4,
            "ambiente": 27, "umidade": 58,
            "modos": ["cool", "dry", "wind", "auto", "heat"], "ventos": ["auto", "low", "medium", "high", "turbo"],
            "oscilacoes": ["fixed", "vertical", "horizontal", "all"], "extras": ["off", "windFree", "windFreeSleep", "sleep", "speed"],
        });
        let Value::Object(device) = device else { unreachable!() };
        Self { device }
    }
}

impl Unit for DemoUnit {
    fn is_demo(&self) -> bool {
        true
    }

    fn run(&mut self, changes: &[(String, Value)]) -> Result<Outcome, Failure> {
        ordered_commands(changes)?;
        sleep(Duration::from_millis(400));
        for (key, value) in changes {
            self.device.insert(key.clone(), value.clone());
        }
        Ok(Outcome { device: self.device.clone(), pending: vec![] })
    }
}

/// Power on first and power off last, so climate changes reach a running unit, as the cloud path does.
fn order(changes: &[(String, Value)]) -> Vec<(String, Value)> {
    let mut ordered: Vec<(String, Value)> = changes.iter().filter(|(key, _)| key != "ligado").cloned().collect();
    if let Some(power) = changes.iter().find(|(key, _)| key == "ligado") {
        if power.1 == Value::Bool(true) {
            ordered.insert(0, power.clone());
        } else {
            ordered.push(power.clone());
        }
    }
    ordered
}

const LOCAL_CONFIRM: usize = 3;
const DISCOVERY_COOLDOWN: Duration = Duration::from_secs(45);

/// Controls the unit over the home network when it can, and through SmartThings otherwise. The
/// network path is tried first on every command and refresh; any local failure falls back to the
/// cloud, so the panel keeps working exactly as before.
pub struct HybridUnit {
    cloud: CloudUnit,
    settings: PathBuf,
    identity: Option<Identity>,
    enabled: bool,
    addr: Option<SocketAddr>,
    session: Option<Link>,
    units: Vec<Found>,
    status: String,
    cooldown_until: Option<Instant>,
}

impl HybridUnit {
    pub fn new(cloud: CloudUnit, settings: PathBuf) -> Self {
        let enabled = settings::local_enabled(&settings);
        let addr = settings::local_addr(&settings).and_then(|text| text.parse().ok());
        let (identity, status) = match Identity::new() {
            Ok(identity) => (Some(identity), String::new()),
            Err(error) => {
                log::line(format_args!("local: sem identidade: {error}"));
                (None, "Não deu para preparar o controle local".to_owned())
            }
        };
        Self { cloud, settings, identity, enabled, addr, session: None, units: Vec::new(), status, cooldown_until: None }
    }

    fn can_do_locally(&self, changes: &[(String, Value)]) -> bool {
        changes.iter().all(|(key, value)| localac::write_for(key, value).is_some())
    }

    /// Reuses the open session, opening one if needed. The secure port moves when the unit restarts,
    /// so a failed open looks for the same host again before giving up.
    fn open(&mut self) -> Result<&mut Link, String> {
        if self.identity.is_none() {
            return Err("sem identidade local".into());
        }
        let addr = self.addr.ok_or("nenhum ar escolhido na rede")?;
        if self.session.is_none() {
            let identity = self.identity.as_ref().unwrap();
            match Link::open(addr, identity) {
                Ok(link) => self.session = Some(link),
                Err(first) => match localac::discover(identity).into_iter().find(|unit| unit.addr.ip() == addr.ip()) {
                    Some(found) => {
                        let link = Link::open(found.addr, identity).map_err(|_| first.clone())?;
                        self.session = Some(link);
                        self.addr = Some(found.addr);
                        settings::set_local_addr(&self.settings, Some(&found.addr.to_string()));
                    }
                    None => return Err(first),
                },
            }
        }
        Ok(self.session.as_mut().unwrap())
    }

    fn read_local(&mut self) -> Result<Device, String> {
        let chosen = self.cloud.chosen();
        let link = self.open()?;
        let mut state = localac::read(link)?;
        if let Some((id, nome)) = chosen {
            state.insert("id".into(), json!(id));
            state.insert("nome".into(), json!(nome));
        }
        Ok(state)
    }

    fn local_run(&mut self, changes: &[(String, Value)]) -> Result<Outcome, String> {
        let ordered = order(changes);
        {
            let link = self.open()?;
            for (key, value) in &ordered {
                let (path, body) = localac::write_for(key, value).ok_or_else(|| format!("sem caminho local para {key}"))?;
                let response = link.post(&path, &body)?;
                if !response.ok() {
                    return Err(format!("o ar recusou {key}"));
                }
            }
        }
        let mut state = self.read_local()?;
        for _ in 1..LOCAL_CONFIRM {
            if unconfirmed(changes, &state).is_empty() {
                break;
            }
            sleep(Duration::from_millis(400));
            state = self.read_local()?;
        }
        let pending = unconfirmed(changes, &state);
        Ok(Outcome { device: state, pending })
    }

    fn try_discover(&mut self) {
        let Some(identity) = self.identity.as_ref() else {
            return;
        };
        self.units = localac::discover(identity);
        self.cooldown_until = Some(Instant::now() + DISCOVERY_COOLDOWN);
        match self.units.len() {
            0 => self.status = "Nenhum ar encontrado na rede".into(),
            1 => {
                let unit = self.units[0].clone();
                self.addr = Some(unit.addr);
                settings::set_local_addr(&self.settings, Some(&unit.addr.to_string()));
                self.status = format!("Achei {} na rede", unit.name);
            }
            many => self.status = format!("{many} ares na rede, escolha um"),
        }
    }
}

impl Unit for HybridUnit {
    fn run(&mut self, changes: &[(String, Value)]) -> Result<Outcome, Failure> {
        if self.enabled && self.identity.is_some() && self.can_do_locally(changes) {
            if self.addr.is_none() && self.cooldown_until.is_none_or(|at| Instant::now() >= at) {
                self.try_discover();
            }
            if self.addr.is_some() {
                match self.local_run(changes) {
                    Ok(outcome) => {
                        if !self.status.starts_with("Achei") {
                            self.status = "Controlando pela rede".into();
                        }
                        return Ok(outcome);
                    }
                    Err(error) => {
                        log::line(format_args!("local: {error}"));
                        self.session = None;
                        self.status = "Rede falhou agora, usando o SmartThings".into();
                    }
                }
            }
        }
        self.cloud.run(changes)
    }

    fn login(&mut self) -> Result<(), Failure> {
        self.cloud.login()
    }

    fn login_control(&self) -> Option<Arc<LoginControl>> {
        self.cloud.login_control()
    }

    fn choose(&mut self, id: &str, name: Option<&str>) {
        self.cloud.choose(id, name);
    }

    fn forget_device(&mut self) {
        self.cloud.forget_device();
    }

    fn logout(&mut self) {
        self.cloud.logout();
    }

    fn set_local(&mut self, enabled: bool) {
        self.enabled = enabled;
        settings::set_local_enabled(&self.settings, enabled);
        if !enabled {
            self.session = None;
            self.status = String::new();
        } else if self.addr.is_none() {
            self.try_discover();
        }
    }

    fn find_local(&mut self) {
        self.cooldown_until = None;
        self.try_discover();
    }

    fn choose_local(&mut self, addr: &str) {
        match addr.parse::<SocketAddr>() {
            Ok(parsed) => {
                self.addr = Some(parsed);
                settings::set_local_addr(&self.settings, Some(addr));
                self.session = None;
                self.status = "Ar escolhido na rede".into();
            }
            Err(_) => self.status = "Endereço inválido".into(),
        }
    }

    fn local_view(&self) -> Value {
        let units: Vec<Value> =
            self.units.iter().map(|unit| json!({ "addr": unit.addr.to_string(), "nome": unit.name, "modelo": unit.model })).collect();
        json!({
            "enabled": self.enabled,
            "addr": self.addr.map(|addr| addr.to_string()),
            "units": units,
            "status": self.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(key: &str, value: Value) -> (String, Value) {
        (key.to_owned(), value)
    }

    #[test]
    fn power_on_goes_first_and_power_off_last() {
        let on = ordered_commands(&[change("temp", json!(22)), change("ligado", json!(true))]).unwrap();
        assert_eq!(on.iter().map(|command| command.command).collect::<Vec<_>>(), ["on", "setCoolingSetpoint"]);
        let off = ordered_commands(&[change("ligado", json!(false)), change("modo", json!("dry"))]).unwrap();
        assert_eq!(off.iter().map(|command| command.command).collect::<Vec<_>>(), ["setAirConditionerMode", "off"]);
    }

    #[test]
    fn rejects_values_outside_the_unit_range() {
        assert!(command_for("temp", &json!(31)).is_none());
        assert!(command_for("modo", &json!("turbo")).is_none());
        assert!(command_for("ligado", &json!("on")).is_none());
        assert!(command_for("display", &json!("on")).is_some());
    }

    #[test]
    fn display_uses_the_inverted_samsung_option() {
        let command = command_for("display", &json!("off")).unwrap();
        assert_eq!(command.arguments[1], json!({ "x.com.samsung.da.options": ["Light_On"] }));
    }

    #[test]
    fn reads_status_and_normalizes_numbers() {
        let main = json!({
            "switch": { "switch": { "value": "on" } },
            "thermostatCoolingSetpoint": { "coolingSetpoint": { "value": 23.0 } },
            "audioVolume": { "volume": { "value": 0 } },
            "powerConsumptionReport": { "powerConsumption": { "value": { "power": 512, "energy": 812345 } } },
        });
        let device = read_device(&main);
        assert_eq!(device["ligado"], json!(true));
        assert_eq!(device["temp"], json!(23));
        assert_eq!(device["bipe"], json!("off"));
        assert_eq!(device["energia"], json!(812.3));
        assert_eq!(device["modo"], Value::Null);
        assert!(unconfirmed(&[change("temp", json!(23))], &device).is_empty());
    }
}
