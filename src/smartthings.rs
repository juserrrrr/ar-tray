//! SmartThings REST API with the same browser login as the official SmartThings CLI.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::{log, platform};

const API_URL: &str = "https://api.smartthings.com/v1";
const OAUTH_URL: &str = "https://oauthin-regional.api.smartthings.com/oauth";
const REFRESH_URL: &str = "https://auth-global.api.smartthings.com/oauth/token";
// Public PKCE client of the SmartThings CLI. Its redirect URIs only allow these local ports.
const CLIENT_ID: &str = "d18cf96e-c626-4433-bf51-ddbb10c5d1ed";
const LOGIN_PORTS: [u16; 3] = [61973, 61974, 61975];
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REFRESH_MARGIN_SECS: u64 = 60 * 60;
const USER_AGENT: &str = concat!("ar-tray/", env!("CARGO_PKG_VERSION"));

pub struct Command {
    pub capability: &'static str,
    pub command: &'static str,
    pub arguments: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

#[derive(Debug)]
pub enum CloudError {
    /// No session, or SmartThings refused to renew it.
    SignedOut,
    Cancelled,
    Failed(String),
}

impl From<String> for CloudError {
    fn from(message: String) -> Self {
        CloudError::Failed(message)
    }
}

enum TokenError {
    /// SmartThings answered without a token, so the grant is no longer valid.
    Refused(String),
    Network(String),
}

/// Lets the panel cancel or reopen a browser login that is waiting.
#[derive(Default)]
pub struct LoginControl {
    cancel: AtomicBool,
    url: Mutex<Option<String>>,
}

impl LoginControl {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn reopen(&self) {
        let url = self.url.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(url) = url {
            platform::open_url(&url);
        }
    }

    fn set_url(&self, url: Option<String>) {
        *self.url.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = url;
    }
}

pub struct Cloud {
    agent: Agent,
    session: Option<Session>,
    session_file: PathBuf,
    control: Arc<LoginControl>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                index += 1;
            }
            (byte, None) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(key, value)| format!("{key}={}", encode(value))).collect::<Vec<_>>().join("&")
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

impl Cloud {
    pub fn new(session_file: PathBuf) -> Self {
        let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
        let agent = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        let session = fs::read(&session_file)
            .ok()
            .and_then(|sealed| platform::unprotect(&sealed))
            .and_then(|json| serde_json::from_slice(&json).ok());
        Self { agent, session, session_file, control: Arc::default() }
    }

    pub fn control(&self) -> Arc<LoginControl> {
        Arc::clone(&self.control)
    }

    pub fn logout(&mut self) {
        self.session = None;
        let _ = fs::remove_file(&self.session_file);
    }

    pub fn devices(&mut self) -> Result<Vec<Value>, CloudError> {
        let mut devices = Vec::new();
        let mut next = Some(format!("{API_URL}/devices"));
        while let Some(url) = next {
            let mut page = self.request(&url, None)?;
            next = page["_links"]["next"]["href"].as_str().map(str::to_owned);
            if let Value::Array(items) = page["items"].take() {
                devices.extend(items);
            }
        }
        Ok(devices)
    }

    /// Attributes of the device's main component.
    pub fn status(&mut self, device_id: &str) -> Result<Value, CloudError> {
        let mut status = self.request(&format!("{API_URL}/devices/{device_id}/status"), None)?;
        Ok(status["components"]["main"].take())
    }

    pub fn command(&mut self, device_id: &str, command: &Command) -> Result<(), CloudError> {
        let body = json!({ "commands": [{
            "component": "main",
            "capability": command.capability,
            "command": command.command,
            "arguments": command.arguments,
        }] });
        self.request(&format!("{API_URL}/devices/{device_id}/commands"), Some(&body)).map(drop)
    }

    /// GET without a body, POST with one.
    fn request(&mut self, url: &str, body: Option<&Value>) -> Result<Value, CloudError> {
        for attempt in 0..2 {
            let token = self.access_token()?;
            let authorization = format!("Bearer {token}");
            let response = match body {
                Some(body) => self.agent.post(url).header("Authorization", &authorization).send_json(body),
                None => self.agent.get(url).header("Authorization", &authorization).call(),
            };
            let mut response = response.map_err(|error| error.to_string())?;
            let status = response.status();
            if status == 401 {
                if attempt == 0 {
                    // The token was revoked early; the next attempt renews it.
                    if let Some(session) = &mut self.session {
                        session.expires_at = 0;
                    }
                    continue;
                }
                self.logout();
                return Err(CloudError::SignedOut);
            }
            if !status.is_success() {
                return Err(CloudError::Failed(format!("SmartThings respondeu {status} em {url}")));
            }
            return Ok(response.body_mut().read_json::<Value>().unwrap_or(Value::Null));
        }
        Err(CloudError::SignedOut)
    }

    /// Never opens the browser: without a valid session the panel asks to sign in.
    fn access_token(&mut self) -> Result<String, CloudError> {
        let Some(session) = &self.session else { return Err(CloudError::SignedOut) };
        if session.expires_at > now_secs() + REFRESH_MARGIN_SECS {
            return Ok(session.access_token.clone());
        }
        let refresh_token = session.refresh_token.clone();
        match self.token_request(REFRESH_URL, &[("grant_type", "refresh_token"), ("client_id", CLIENT_ID), ("refresh_token", &refresh_token)]) {
            Ok(session) => Ok(self.keep(session)),
            Err(TokenError::Refused(message)) => {
                log::line(format_args!("SmartThings recusou renovar o login: {message}"));
                self.logout();
                Err(CloudError::SignedOut)
            }
            Err(TokenError::Network(message)) => Err(CloudError::Failed(message)),
        }
    }

    fn keep(&mut self, session: Session) -> String {
        let token = session.access_token.clone();
        if let Some(sealed) = serde_json::to_vec(&session).ok().and_then(|json| platform::protect(&json))
            && let Err(error) = fs::write(&self.session_file, sealed)
        {
            log::line(format_args!("não deu para guardar o login: {error}"));
        }
        self.session = Some(session);
        token
    }

    fn token_request(&self, url: &str, form: &[(&str, &str)]) -> Result<Session, TokenError> {
        let mut response = self.agent.post(url).send_form(form.iter().copied()).map_err(|error| TokenError::Network(error.to_string()))?;
        let status = response.status();
        let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
        let (Some(access), Some(refresh)) = (body["access_token"].as_str(), body["refresh_token"].as_str()) else {
            let message = format!("SmartThings respondeu {status} sem token: {}", body["error"]);
            return Err(if status.is_client_error() { TokenError::Refused(message) } else { TokenError::Network(message) });
        };
        Ok(Session {
            access_token: access.to_owned(),
            refresh_token: refresh.to_owned(),
            expires_at: now_secs() + body["expires_in"].as_u64().unwrap_or(0),
        })
    }

    /// Opens the Samsung login in the browser and waits for its redirect, up to 10 minutes.
    pub fn login(&mut self) -> Result<(), CloudError> {
        self.control.cancel.store(false, Ordering::Relaxed);
        let verifier = random_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let (port, listeners) = bind_login_port()?;
        let redirect = format!("http://localhost:{port}/finish");
        let authorize = query(&[
            ("scope", "controller:stCli"),
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("redirect_uri", &redirect),
            ("client_type", "USER_LEVEL"),
        ]);
        let url = format!("{OAUTH_URL}/authorize?{authorize}");
        self.control.set_url(Some(url.clone()));
        platform::open_url(&url);
        let code = wait_for_code(&listeners, &self.control.cancel);
        self.control.set_url(None);
        log::line(format_args!("login: retorno do navegador {}", if code.is_ok() { "com código" } else { "sem código" }));
        let session = self
            .token_request(
                &format!("{OAUTH_URL}/token"),
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", CLIENT_ID),
                    ("code_verifier", &verifier),
                    ("code", &code?),
                    ("redirect_uri", &redirect),
                ],
            )
            .map_err(|(TokenError::Refused(message) | TokenError::Network(message))| CloudError::Failed(message))?;
        log::line(format_args!("login: token recebido, válido por {} s", session.expires_at.saturating_sub(now_secs())));
        self.keep(session);
        Ok(())
    }
}

/// Browsers may resolve localhost to IPv4 or IPv6, so both loopbacks listen.
fn bind_login_port() -> Result<(u16, Vec<TcpListener>), String> {
    for port in LOGIN_PORTS {
        let Ok(ipv4) = TcpListener::bind(("127.0.0.1", port)) else { continue };
        let mut listeners = vec![ipv4];
        listeners.extend(TcpListener::bind(("::1", port)));
        for listener in &listeners {
            listener.set_nonblocking(true).map_err(|error| error.to_string())?;
        }
        return Ok((port, listeners));
    }
    Err("as portas do login (61973 a 61975) estão ocupadas".into())
}

fn wait_for_code(listeners: &[TcpListener], cancel: &AtomicBool) -> Result<String, CloudError> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return Err(CloudError::Cancelled);
        }
        for listener in listeners {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = answer_callback(stream) {
                        return result.map_err(CloudError::Failed);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(CloudError::Failed(error.to_string())),
            }
        }
        sleep(Duration::from_millis(100));
    }
    Err(CloudError::Failed("o login no navegador não terminou em 10 minutos".into()))
}

/// Returns None for requests that are not the OAuth redirect, such as the favicon.
fn answer_callback(mut stream: TcpStream) -> Option<Result<String, String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let target = request.lines().next()?.split(' ').nth(1)?;
    let Some(params) = target.strip_prefix("/finish?") else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    };
    let param = |name: &str| {
        params.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('=').map(decode))
    };
    let result = match (param("code"), param("error")) {
        (Some(code), None) => Ok(code),
        (_, error) => Err(format!(
            "login recusado: {} {}",
            error.unwrap_or_default(),
            param("error_description").unwrap_or_default()
        )),
    };
    let (title, message) = if result.is_ok() {
        ("Conta conectada", "Pode fechar esta aba. O Painel do Ar já está mostrando o seu ar-condicionado.")
    } else {
        ("Não deu para entrar", "Volte ao Painel do Ar e tente de novo.")
    };
    let page = format!(
        "<!doctype html><html lang=\"pt-BR\"><meta charset=\"utf-8\"><title>{title}</title>\
         <body style=\"font-family:Bahnschrift,'Segoe UI',sans-serif;background:#101317;color:#eef1f4;\
         display:grid;place-content:center;height:100vh;margin:0;text-align:center\">\
         <h1 style=\"font-weight:600\">{title}</h1><p style=\"color:#8b939c\">{message}</p></body></html>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Network checks: cargo test smartthings -- --ignored
    #[test]
    #[ignore]
    fn reaches_smartthings_over_https() {
        let cloud = Cloud::new(std::env::temp_dir().join("ar-tray-teste-sessao.bin"));
        let response = cloud.agent.get(format!("{API_URL}/devices")).call();
        assert!(response.is_ok(), "{response:?}");
        assert_eq!(response.unwrap().status(), 401);
    }

    #[test]
    #[ignore]
    fn a_bad_login_code_is_refused_without_failing() {
        let cloud = Cloud::new(std::env::temp_dir().join("ar-tray-teste-sessao.bin"));
        let form = [("grant_type", "authorization_code"), ("client_id", CLIENT_ID), ("code_verifier", "x"), ("code", "falso"), ("redirect_uri", "http://localhost:61973/finish")];
        assert!(matches!(cloud.token_request(&format!("{OAUTH_URL}/token"), &form), Err(TokenError::Refused(_))));
    }

    #[test]
    fn encodes_and_decodes_query_values() {
        assert_eq!(encode("controller:stCli"), "controller%3AstCli");
        assert_eq!(encode("http://localhost:61973/finish"), "http%3A%2F%2Flocalhost%3A61973%2Ffinish");
        assert_eq!(decode("a%2Fb+c"), "a/b c");
        assert_eq!(decode("abc%"), "abc%");
        assert_eq!(decode("abc%4"), "abc%4");
        assert_eq!(decode("%41"), "A");
    }
}
