//! The unit's OCF resources mapped to and from the panel's device fields, plus finding units on the
//! network and matching them to the chosen SmartThings aparelho.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::ac::Device;
use crate::local::{Identity, Link, USUAL_PORTS};

/// A unit found on the network, with what identifies it.
#[derive(Clone)]
pub struct Found {
    pub addr: SocketAddr,
    pub name: String,
    pub model: String,
}

fn text(rep: &Value, key: &str) -> Option<String> {
    rep.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn integer(value: &str) -> Value {
    value.parse::<f64>().map_or(Value::Null, |number| json!(number.round() as i64))
}

const MODES: [(&str, &str); 5] = [("Auto", "auto"), ("Cool", "cool"), ("Dry", "dry"), ("Wind", "wind"), ("Heat", "heat")];
const FANS: [(&str, &str); 5] = [("0", "auto"), ("1", "low"), ("2", "medium"), ("3", "high"), ("4", "turbo")];
const SWINGS: [(&str, &str); 4] = [("Fix", "fixed"), ("Up_And_Low", "vertical"), ("Left_And_Right", "horizontal"), ("All", "all")];
const EXTRAS: [(&str, &str); 6] =
    [("Off", "off"), ("Nano", "windFree"), ("NanoSleep", "windFreeSleep"), ("Sleep", "sleep"), ("Quiet", "quiet"), ("Speed", "speed")];

fn map_from<'a>(table: &'a [(&str, &str)], device: &str) -> Option<&'a str> {
    table.iter().find(|(from, _)| *from == device).map(|(_, to)| *to)
}

fn map_to<'a>(table: &'a [(&str, &str)], panel: &str) -> Option<&'a str> {
    table.iter().find(|(_, to)| *to == panel).map(|(from, _)| *from)
}

fn supported(table: &[(&str, &str)], rep: &Value) -> Value {
    let modes = rep["x.com.samsung.da.supportedModes"].as_array();
    let mapped: Vec<Value> = modes
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|device| map_from(table, device))
        .map(|panel| json!(panel))
        .collect();
    json!(mapped)
}

/// Reads the whole collection in one batch and builds the same device shape the cloud path returns.
pub fn read(link: &mut Link) -> Result<Device, String> {
    let response = link.get("/device/0?if=oic.if.b")?;
    if !response.ok() {
        return Err(format!("o ar respondeu {}.{:02} ao ler", response.code >> 5, response.code & 31));
    }
    let items = response.body.as_array().ok_or("resposta do ar fora do formato")?;
    let mut reps: Map<String, Value> = Map::new();
    for item in items {
        if let (Some(href), Some(rep)) = (item.get("href").and_then(Value::as_str), item.get("rep")) {
            reps.insert(href.to_owned(), rep.clone());
        }
    }
    let rep = |href: &str| reps.get(href).cloned().unwrap_or(Value::Null);

    let mode = rep("/mode/vs/0");
    let current_mode = mode["x.com.samsung.da.modes"].as_array().and_then(|list| list.first()).and_then(Value::as_str);
    let temps = rep("/temperatures/vs/0");
    let temp_item = temps["x.com.samsung.da.items"].as_array().and_then(|list| list.first()).cloned().unwrap_or(Value::Null);
    let wind = rep("/wind/strength/vs/0");
    let direction = rep("/wind/direction/vs/0");
    let convenient = rep("/mode/convenient/vs/0");
    let filter = rep("/filter/airdustfilter/vs/0");
    let energy = rep("/energy/consumption/vs/0");
    let humidity = rep("/humidity/vs/0");

    let lifetime = text(&energy, "x.com.samsung.da.cumulativePower")
        .and_then(|watt_hours| watt_hours.parse::<f64>().ok())
        .map(|watt_hours| json!((watt_hours / 100.0).round() / 10.0))
        .unwrap_or(Value::Null);
    let power_now = energy.get("x.com.samsung.da.instantaneousPower").and_then(Value::as_str).and_then(|w| w.parse::<f64>().ok());

    let display = mode["x.com.samsung.da.options"].as_array().and_then(|options| {
        options.iter().filter_map(Value::as_str).find_map(|token| match token {
            "Light_On" => Some("off"),
            "Light_Off" => Some("on"),
            _ => None,
        })
    });

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());

    let fields = [
        ("ligado", json!(text(&rep("/power/vs/0"), "x.com.samsung.da.power").as_deref() == Some("On"))),
        ("temp", text(&temp_item, "x.com.samsung.da.desired").map_or(Value::Null, |value| integer(&value))),
        ("modo", current_mode.and_then(|device| map_from(&MODES, device)).map_or(Value::Null, |panel| json!(panel))),
        ("vento", text(&wind, "x.com.samsung.da.modes").and_then(|index| map_from(&FANS, &index).map(|p| json!(p))).unwrap_or(Value::Null)),
        (
            "extra",
            text(&convenient, "x.com.samsung.da.modes").and_then(|code| map_from(&EXTRAS, &code).map(|p| json!(p))).unwrap_or(Value::Null),
        ),
        (
            "oscilar",
            text(&direction, "x.com.samsung.da.modes").and_then(|code| map_from(&SWINGS, &code).map(|p| json!(p))).unwrap_or(Value::Null),
        ),
        ("display", display.map_or(Value::Null, |state| json!(state))),
        ("bipe", Value::Null),
        ("autoclean", json!(if text(&rep("/option/autoclean/vs/0"), "x.com.samsung.da.settingStatus").as_deref() == Some("On") { "on" } else { "off" })),
        ("filtroAlerta", text(&filter, "x.com.samsung.da.filterDesiredUsage").map_or(Value::Null, |v| integer(&v))),
        ("filtroUso", text(&filter, "x.com.samsung.da.filterUsage").map_or(Value::Null, |v| integer(&v))),
        ("filtroStatus", text(&filter, "x.com.samsung.da.filterStatus").map_or(Value::Null, |s| json!(s))),
        ("potencia", power_now.map_or(Value::Null, |watts| json!(watts.round() as i64))),
        // Read live from the unit, so the panel shows it as measured now.
        ("potenciaFim", json!(now)),
        ("energia", lifetime),
        ("ambiente", temp_from(&temp_item, &rep("/temperature/current/0"))),
        ("umidade", text(&humidity, "x.com.samsung.da.humidity").map_or(Value::Null, |v| integer(&v))),
        ("modos", supported(&MODES, &mode)),
        ("ventos", supported(&FANS, &wind)),
        ("oscilacoes", supported(&SWINGS, &direction)),
        ("extras", supported(&EXTRAS, &convenient)),
    ];
    let mut device: Device = fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect();
    // A stable identity straight from the unit, so the panel and the monthly energy work without the
    // cloud. The caller replaces these with the SmartThings label when an account is connected.
    let information = rep("/information/vs/0");
    if let Some(serial) = text(&information, "x.com.samsung.da.serialNum").filter(|serial| !serial.is_empty()) {
        device.insert("id".into(), json!(serial));
    }
    device.insert("nome".into(), json!("Ar-condicionado"));
    Ok(device)
}

fn temp_from(temp_item: &Value, current: &Value) -> Value {
    if let Some(text) = temp_item.get("x.com.samsung.da.current").and_then(Value::as_str) {
        return integer(text);
    }
    current.get("temperature").and_then(Value::as_f64).map_or(Value::Null, |value| json!(value.round() as i64))
}

/// The OCF path and body for one panel change, or None when the local path can't do it (bipe).
pub fn write_for(key: &str, value: &Value) -> Option<(String, Value)> {
    let items = |desired: String| json!({ "x.com.samsung.da.items": [{ "x.com.samsung.da.id": "0", "x.com.samsung.da.desired": desired }] });
    match (key, value) {
        ("ligado", Value::Bool(on)) => {
            Some(("/power/vs/0".into(), json!({ "x.com.samsung.da.power": if *on { "On" } else { "Off" } })))
        }
        ("temp", value) => {
            let number = value.as_f64()?;
            Some(("/temperatures/vs/0".into(), items(format!("{:.1}", number))))
        }
        ("modo", Value::String(panel)) => Some(("/mode/vs/0".into(), json!({ "x.com.samsung.da.modes": [map_to(&MODES, panel)?] }))),
        ("vento", Value::String(panel)) => Some(("/wind/strength/vs/0".into(), json!({ "x.com.samsung.da.modes": map_to(&FANS, panel)? }))),
        ("oscilar", Value::String(panel)) => {
            Some(("/wind/direction/vs/0".into(), json!({ "x.com.samsung.da.modes": map_to(&SWINGS, panel)? })))
        }
        ("extra", Value::String(panel)) => {
            Some(("/mode/convenient/vs/0".into(), json!({ "x.com.samsung.da.modes": map_to(&EXTRAS, panel)? })))
        }
        ("display", Value::String(state)) if state == "on" || state == "off" => {
            // Same inverted option names the cloud uses: Light_Off lights the display.
            let option = if state == "on" { "Light_Off" } else { "Light_On" };
            Some(("/mode/vs/0".into(), json!({ "x.com.samsung.da.options": [option] })))
        }
        ("autoclean", Value::String(state)) if state == "on" || state == "off" => {
            Some(("/option/autoclean/vs/0".into(), json!({ "x.com.samsung.da.settingStatus": if state == "on" { "On" } else { "Off" } })))
        }
        _ => None,
    }
}

/// Reads the two identity resources, to label a unit and match it to the cloud aparelho.
fn identify(link: &mut Link) -> Result<Found, String> {
    let device = link.get("/oic/d")?;
    let information = link.get("/information/vs/0")?;
    let name = device.body.get("n").and_then(Value::as_str).unwrap_or("Ar-condicionado").to_owned();
    let model = text(&information.body, "x.com.samsung.da.modelNum").unwrap_or_default();
    Ok(Found { addr: link.remote, name, model })
}

/// The primary IPv4 of this PC, learned without sending anything.
fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(1, 1, 1, 1), 80)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_loopback() => Some(address),
        _ => None,
    }
}

/// A DTLS 1.2 ClientHello with no cookie: a server answers it with a HelloVerifyRequest, which tells
/// a secure port apart from a dead one cheaply, before any certificate work.
fn client_hello() -> Vec<u8> {
    let mut random = [0u8; 32];
    let _ = getrandom::fill(&mut random);
    let mut body = vec![0xfe, 0xfd];
    body.extend_from_slice(&random);
    body.push(0); // session id length
    body.push(0); // cookie length
    body.extend_from_slice(&[0x00, 0x02, 0xc0, 0x2b]); // one cipher suite
    body.extend_from_slice(&[0x01, 0x00]); // compression: null
    let extensions: &[u8] = &[
        0x00, 0x0a, 0x00, 0x04, 0x00, 0x02, 0x00, 0x17, // supported_groups: secp256r1
        0x00, 0x0b, 0x00, 0x02, 0x01, 0x00, // ec_point_formats: uncompressed
        0x00, 0x0d, 0x00, 0x04, 0x00, 0x02, 0x04, 0x03, // signature_algorithms: ecdsa_secp256r1_sha256
    ];
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(extensions);
    let length = (body.len() as u32).to_be_bytes();
    let mut handshake = vec![0x01, length[1], length[2], length[3], 0x00, 0x00, 0x00, 0x00, 0x00, length[1], length[2], length[3]];
    handshake.extend_from_slice(&body);
    let mut record = vec![0x16, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, (handshake.len() >> 8) as u8, handshake.len() as u8];
    record.extend_from_slice(&handshake);
    record
}

/// Sweeps the /24 on the usual secure ports and returns the addresses that answered as DTLS servers.
fn responders(deadline: Instant) -> Vec<SocketAddr> {
    let Some(local) = local_ipv4() else { return Vec::new() };
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return Vec::new() };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
    let hello = client_hello();
    let base = local.octets();
    for host in 1u16..=254 {
        let ip = Ipv4Addr::new(base[0], base[1], base[2], host as u8);
        if ip == local {
            continue;
        }
        for port in USUAL_PORTS {
            let _ = socket.send_to(&hello, (ip, port));
        }
    }
    let mut found = Vec::new();
    let mut buffer = [0u8; 256];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, from)) if length > 0 && from.ip() != IpAddr::V4(local) => {
                if !found.contains(&from) {
                    found.push(from);
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    found
}

/// Finds air conditioners on the network, each opened once to read its identity.
pub fn discover(identity: &Identity) -> Vec<Found> {
    let mut units = Vec::new();
    for addr in responders(Instant::now() + Duration::from_secs(4)) {
        if let Ok(mut link) = Link::open(addr, identity)
            && let Ok(found) = identify(&mut link)
            && !found.model.is_empty()
        {
            units.push(found);
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_core_fields_both_ways() {
        assert_eq!(map_from(&EXTRAS, "Nano"), Some("windFree"));
        assert_eq!(map_to(&EXTRAS, "windFree"), Some("Nano"));
        assert_eq!(map_from(&FANS, "4"), Some("turbo"));
        assert_eq!(map_to(&SWINGS, "vertical"), Some("Up_And_Low"));
    }

    #[test]
    fn builds_the_write_bodies_the_unit_accepts() {
        let (path, body) = write_for("temp", &json!(25)).unwrap();
        assert_eq!(path, "/temperatures/vs/0");
        assert_eq!(body["x.com.samsung.da.items"][0]["x.com.samsung.da.desired"], json!("25.0"));
        let (path, body) = write_for("extra", &json!("windFree")).unwrap();
        assert_eq!(path, "/mode/convenient/vs/0");
        assert_eq!(body["x.com.samsung.da.modes"], json!("Nano"));
        assert_eq!(write_for("ligado", &json!(true)).unwrap().1["x.com.samsung.da.power"], json!("On"));
        // bipe is not on the local path, so the caller falls back to the cloud for it.
        assert!(write_for("bipe", &json!("on")).is_none());
    }

    #[test]
    fn client_hello_is_a_dtls_record() {
        let hello = client_hello();
        assert_eq!(&hello[..3], &[0x16, 0xfe, 0xfd]);
        assert_eq!(hello[13], 0x01); // handshake type: client_hello
    }

    // Against a real unit on the LAN, read + a reversible setpoint write. Needs the address in
    // AR_TRAY_LOCAL (host:porta): cargo test local_roundtrip -- --ignored
    #[test]
    #[ignore]
    fn local_roundtrip() {
        let addr: SocketAddr = std::env::var("AR_TRAY_LOCAL").expect("defina AR_TRAY_LOCAL=host:porta").parse().unwrap();
        let identity = Identity::new().unwrap();
        let mut link = Link::open(addr, &identity).unwrap();
        let before = read(&mut link).unwrap();
        println!("lido: ligado={} temp={} modo={} vento={} extra={} ambiente={}",
            before["ligado"], before["temp"], before["modo"], before["vento"], before["extra"], before["ambiente"]);
        assert!(before["modos"].as_array().is_some_and(|modes| !modes.is_empty()));
        let original = before["temp"].as_i64().expect("o ar deve informar a temperatura");
        let target = if original >= 24 { original - 1 } else { original + 1 };
        let (path, body) = write_for("temp", &json!(target)).unwrap();
        assert!(link.post(&path, &body).unwrap().ok(), "o ar deveria aceitar a escrita");
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let after = read(&mut link).unwrap();
        // A running unit reflects the setpoint right away; a unit that is off holds it until it starts.
        if before["ligado"] == json!(true) {
            assert_eq!(after["temp"], json!(target), "a mudança deveria valer com o ar ligado");
            println!("mudou {original} -> {target} e confirmou");
        } else {
            println!("ar desligado: escrita aceita, leitura {} (aplica ao ligar)", after["temp"]);
        }
        // Put it back the way it was.
        let (path, body) = write_for("temp", &json!(original)).unwrap();
        assert!(link.post(&path, &body).unwrap().ok());
    }

    // Times several commands in a row, to check they stay quick: cargo test local_rapid -- --ignored --nocapture
    #[test]
    #[ignore]
    fn local_rapid() {
        let addr: SocketAddr = std::env::var("AR_TRAY_LOCAL").expect("defina AR_TRAY_LOCAL=host:porta").parse().unwrap();
        let identity = Identity::new().unwrap();
        let mut link = Link::open(addr, &identity).unwrap();
        let start = before_temp(&mut link);
        for step in 0..5 {
            let target = 20 + step;
            let (path, body) = write_for("temp", &json!(target)).unwrap();
            let began = std::time::Instant::now();
            let response = link.post(&path, &body).unwrap();
            println!("comando {target} °C: {}.{:02} em {} ms", response.code >> 5, response.code & 31, began.elapsed().as_millis());
            assert!(response.ok());
        }
        let (path, body) = write_for("temp", &json!(start)).unwrap();
        assert!(link.post(&path, &body).unwrap().ok());
    }

    fn before_temp(link: &mut Link) -> i64 {
        read(link).unwrap()["temp"].as_i64().unwrap_or(23)
    }

    // Finds units on the LAN by itself: cargo test local_discover -- --ignored --nocapture
    #[test]
    #[ignore]
    fn local_discover() {
        let identity = Identity::new().unwrap();
        let found = discover(&identity);
        for unit in &found {
            println!("achei {} em {} (modelo {})", unit.name, unit.addr, unit.model);
        }
        assert!(!found.is_empty(), "nenhum ar encontrado na rede");
    }
}
