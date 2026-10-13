//! Talking to the unit over the home network: Samsung's OCF interface, CoAP (RFC 7252) over DTLS 1.2.
//!
//! The unit's factory access list trusts Samsung's cloud service identity, a UUID that appears in the
//! subject of the public certificate of connect-v2.samsungiotcloud.com. A certificate made here with
//! that UUID is accepted; the unit does not check who signed it. Method from the LocalThings and
//! SmartThings-Local projects.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use rtc_dtls::cipher_suite::CipherSuiteId;
use rtc_dtls::config::ConfigBuilder;
use rtc_dtls::crypto::{Certificate, CryptoPrivateKey};
use rtc_dtls::crypto_provider::RTCCryptoProvider;
use rtc_dtls::endpoint::{Endpoint, EndpointEvent};
use rtc_shared::TransportProtocol;
use serde_json::Value;

use crate::cbor;

const CLOUD_UUID: &str = "ab0b0ac4-aae9-4958-a04d-8ec36fe1b2f9";
/// Where the units seen so far open their secure port. It can move when the unit restarts.
pub const USUAL_PORTS: std::ops::RangeInclusive<u16> = 49152..=49160;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// With a resend every 400 ms, no answer in 2 s means the session is gone and is worth reopening.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(2000);
const RESEND_EVERY: Duration = Duration::from_millis(400);

// CoAP constants.
const CONFIRMABLE: u8 = 0;
const ACK: u8 = 2;
const GET: u8 = 1;
const POST: u8 = 2;
const OPT_URI_PATH: u16 = 11;
const OPT_CONTENT_FORMAT: u16 = 12;
const OPT_ACCEPT: u16 = 17;
const OPT_BLOCK2: u16 = 23;
const CF_CBOR: u8 = 60;
/// 1024-byte blocks (SZX 6), what the unit expects.
const BLOCK_SZX: u8 = 6;

fn rand_u16() -> u16 {
    let mut bytes = [0u8; 2];
    let _ = getrandom::fill(&mut bytes);
    u16::from_le_bytes(bytes)
}

/// Our certificate, made once per run.
pub struct Identity {
    provider: Arc<dyn RTCCryptoProvider>,
    certificate: Certificate,
}

impl Identity {
    pub fn new() -> Result<Self, String> {
        let provider = rtc_dtls::crypto_provider::default_provider().map_err(|error| error.to_string())?;
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|error| error.to_string())?;
        let mut name = rcgen::DistinguishedName::new();
        name.push(rcgen::DnType::CountryName, "KR");
        name.push(rcgen::DnType::OrganizationName, "Samsung Electronics");
        name.push(rcgen::DnType::OrganizationalUnitName, format!("uuid:{CLOUD_UUID}"));
        name.push(rcgen::DnType::CommonName, format!("urn:uuid:{CLOUD_UUID}"));
        params.distinguished_name = name;
        params.not_before = rcgen::date_time_ymd(2025, 1, 1);
        params.not_after = rcgen::date_time_ymd(2045, 1, 1);
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|error| error.to_string())?;
        let certificate = params.self_signed(&key).map_err(|error| error.to_string())?;
        let private_key = CryptoPrivateKey::from_key_pair(&key, provider.crypto()).map_err(|error| error.to_string())?;
        Ok(Self { certificate: Certificate { certificate: vec![certificate.der().to_owned()], private_key }, provider })
    }
}

pub struct Response {
    /// CoAP class and detail: 0x45 is 2.05 Content, 0x44 is 2.04 Changed.
    pub code: u8,
    pub body: Value,
}

impl Response {
    pub fn ok(&self) -> bool {
        self.code >> 5 == 2
    }
}

/// One DTLS session with a unit.
pub struct Link {
    socket: UdpSocket,
    endpoint: Endpoint,
    pub remote: SocketAddr,
    message_id: u16,
    token: u32,
    received: Vec<BytesMut>,
}

impl Link {
    pub fn open(remote: SocketAddr, identity: &Identity) -> Result<Link, String> {
        let config = ConfigBuilder::default()
            .with_crypto_provider(Arc::clone(&identity.provider))
            .with_certificates(vec![identity.certificate.clone()])
            .with_cipher_suites(vec![CipherSuiteId::Tls_Ecdhe_Ecdsa_With_Aes_128_Gcm_Sha256])
            // The unit's certificate chains to Samsung's OCF CA, which is not public; the access list
            // and the home network are what this relies on.
            .with_insecure_skip_verify(true)
            .build(true, Some(remote))
            .map_err(|error| error.to_string())?;
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|error| error.to_string())?;
        let local = socket.local_addr().map_err(|error| error.to_string())?;
        let mut endpoint = Endpoint::new(local, TransportProtocol::UDP, None);
        endpoint.connect(Instant::now(), remote, Arc::new(config), None).map_err(|error| error.to_string())?;
        let mut link = Link { socket, endpoint, remote, message_id: rand_u16(), token: rand_u16() as u32, received: Vec::new() };
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            if link.pump(deadline)? {
                return Ok(link);
            }
            if Instant::now() >= deadline {
                return Err(format!("{remote} não completou a conexão segura"));
            }
        }
    }

    /// Sends what is queued and waits for one datagram or a timer. True once the handshake is done.
    fn pump(&mut self, deadline: Instant) -> Result<bool, String> {
        let mut handshake_done = false;
        self.drain_transmit()?;
        let now = Instant::now();
        let timer = self.endpoint.poll_timeout(&self.remote).unwrap_or(deadline).min(deadline);
        let wait = timer.saturating_duration_since(now).max(Duration::from_millis(5));
        self.socket.set_read_timeout(Some(wait)).map_err(|error| error.to_string())?;
        let mut buffer = [0u8; 2048];
        match self.socket.recv_from(&mut buffer) {
            Ok((length, from)) if from == self.remote => {
                let events = self
                    .endpoint
                    .read(Instant::now(), self.remote, None, BytesMut::from(&buffer[..length]))
                    .map_err(|error| error.to_string())?;
                for event in events {
                    match event {
                        EndpointEvent::HandshakeComplete => handshake_done = true,
                        EndpointEvent::ApplicationData(data) => self.received.push(data),
                        EndpointEvent::PeerClosed => return Err("o ar fechou a conexão".into()),
                        _ => {}
                    }
                }
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                self.endpoint.handle_timeout(self.remote, Instant::now()).map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
        self.drain_transmit()?;
        Ok(handshake_done)
    }

    fn drain_transmit(&mut self) -> Result<(), String> {
        while let Some(transmit) = self.endpoint.poll_transmit() {
            self.socket.send_to(&transmit.message, self.remote).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn get(&mut self, path: &str) -> Result<Response, String> {
        self.request(GET, path, None, &|| false).map(|response| response.expect("never stopped"))
    }

    /// A GET that gives up between blocks when `stop` turns true, so a click does not wait for a
    /// long read to finish. None means it stopped.
    pub fn get_until(&mut self, path: &str, stop: &dyn Fn() -> bool) -> Result<Option<Response>, String> {
        self.request(GET, path, None, stop)
    }

    pub fn post(&mut self, path: &str, body: &Value) -> Result<Response, String> {
        self.request(POST, path, Some(cbor::encode(body)), &|| false).map(|response| response.expect("never stopped"))
    }

    fn request(&mut self, method: u8, path: &str, body: Option<Vec<u8>>, stop: &dyn Fn() -> bool) -> Result<Option<Response>, String> {
        self.token = self.token.wrapping_add(1);
        let token = self.token.to_be_bytes();
        let mut payload = Vec::new();
        let code;
        let mut want_block = 0u32;
        loop {
            if stop() {
                return Ok(None);
            }
            self.message_id = self.message_id.wrapping_add(1);
            let request = encode_request(Request {
                method,
                id: self.message_id,
                token: &token,
                path,
                body: body.as_deref(),
                block2: (want_block > 0).then_some(want_block),
            });
            let answer = self.exchange(&request, &token)?;
            payload.extend_from_slice(&answer.payload);
            match answer.block2 {
                // The unit drops a transfer whose later blocks come under a new token, so the token
                // stays the same and only the block number grows.
                Some((number, true)) if number < 4096 => want_block = number + 1,
                _ => {
                    code = answer.code;
                    break;
                }
            }
        }
        let body = if payload.is_empty() { Value::Null } else { cbor::decode(&payload)? };
        Ok(Some(Response { code, body }))
    }

    /// Sends one datagram and waits for the ACK carrying our token. On the home network an answer
    /// takes about 100 ms, so a lost packet is sent again every RESEND_EVERY instead of waiting
    /// seconds. The copy keeps its message id, and every write sets an absolute value, so a repeat
    /// changes nothing.
    fn exchange(&mut self, datagram: &[u8], token: &[u8]) -> Result<Answer, String> {
        self.endpoint.write(Instant::now(), self.remote, datagram).map_err(|error| error.to_string())?;
        self.drain_transmit()?;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        let mut resend_at = Instant::now() + RESEND_EVERY;
        loop {
            while let Some(data) = self.received.pop() {
                if let Some(answer) = parse_answer(&data, token) {
                    return Ok(answer);
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!("o ar {} não respondeu", self.remote));
            }
            if now >= resend_at {
                self.endpoint.write(now, self.remote, datagram).map_err(|error| error.to_string())?;
                self.drain_transmit()?;
                resend_at = now + RESEND_EVERY;
            }
            self.pump(resend_at.min(deadline))?;
        }
    }
}

struct Request<'a> {
    method: u8,
    id: u16,
    token: &'a [u8],
    path: &'a str,
    body: Option<&'a [u8]>,
    block2: Option<u32>,
}

fn encode_request(request: Request<'_>) -> Vec<u8> {
    let mut out = vec![0x40 | (CONFIRMABLE << 4) | request.token.len() as u8, request.method, (request.id >> 8) as u8, request.id as u8];
    out.extend_from_slice(request.token);
    let mut last = 0u16;
    let mut option = |number: u16, value: &[u8], out: &mut Vec<u8>| {
        let delta = number - last;
        last = number;
        let (delta_nibble, delta_ext) = ext(delta);
        let (length_nibble, length_ext) = ext(value.len() as u16);
        out.push((delta_nibble << 4) | length_nibble);
        out.extend_from_slice(&delta_ext);
        out.extend_from_slice(&length_ext);
        out.extend_from_slice(value);
    };
    for segment in request.path.split('/').filter(|segment| !segment.is_empty()) {
        option(OPT_URI_PATH, segment.as_bytes(), &mut out);
    }
    if request.body.is_some() {
        option(OPT_CONTENT_FORMAT, &[CF_CBOR], &mut out);
    }
    option(OPT_ACCEPT, &[CF_CBOR], &mut out);
    if let Some(number) = request.block2 {
        let value = (number << 4) | BLOCK_SZX as u32;
        let bytes = block_bytes(value);
        option(OPT_BLOCK2, &bytes, &mut out);
    }
    if let Some(body) = request.body {
        out.push(0xff);
        out.extend_from_slice(body);
    }
    out
}

/// A CoAP option's 4-bit nibble and the extended bytes that follow, for values up to 65535 + 269.
fn ext(value: u16) -> (u8, Vec<u8>) {
    match value {
        0..=12 => (value as u8, vec![]),
        13..=268 => (13, vec![(value - 13) as u8]),
        _ => (14, (value - 269).to_be_bytes().to_vec()),
    }
}

fn block_bytes(value: u32) -> Vec<u8> {
    if value <= 0xff {
        vec![value as u8]
    } else if value <= 0xffff {
        (value as u16).to_be_bytes().to_vec()
    } else {
        value.to_be_bytes()[1..].to_vec()
    }
}

struct Answer {
    code: u8,
    payload: Vec<u8>,
    /// (block number, more coming) from a BLOCK2 option.
    block2: Option<(u32, bool)>,
}

fn parse_answer(datagram: &[u8], token: &[u8]) -> Option<Answer> {
    if datagram.len() < 4 {
        return None;
    }
    let token_length = (datagram[0] & 0x0f) as usize;
    let kind = (datagram[0] >> 4) & 0x03;
    let code = datagram[1];
    let mut at = 4;
    let message_token = datagram.get(at..at + token_length)?;
    if kind != ACK || message_token != token {
        return None;
    }
    at += token_length;
    let mut option_number = 0u16;
    let mut block2 = None;
    while at < datagram.len() && datagram[at] != 0xff {
        let byte = datagram[at];
        at += 1;
        let mut delta = (byte >> 4) as u16;
        let mut length = (byte & 0x0f) as usize;
        delta = read_ext(delta, datagram, &mut at)?;
        length = read_ext(length as u16, datagram, &mut at)? as usize;
        option_number += delta;
        let value = datagram.get(at..at + length)?;
        at += length;
        if option_number == OPT_BLOCK2 {
            let mut raw = 0u32;
            for byte in value {
                raw = (raw << 8) | *byte as u32;
            }
            block2 = Some((raw >> 4, raw & 0x08 != 0));
        }
    }
    let payload = if at < datagram.len() { datagram[at + 1..].to_vec() } else { Vec::new() };
    Some(Answer { code, payload, block2 })
}

fn read_ext(nibble: u16, datagram: &[u8], at: &mut usize) -> Option<u16> {
    match nibble {
        13 => {
            let value = *datagram.get(*at)? as u16 + 13;
            *at += 1;
            Some(value)
        }
        14 => {
            let value = u16::from_be_bytes(datagram.get(*at..*at + 2)?.try_into().ok()?) + 269;
            *at += 2;
            Some(value)
        }
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_a_get_with_path_and_accept() {
        let request = encode_request(Request { method: GET, id: 0x1234, token: &[0xaa], path: "/oic/d", body: None, block2: None });
        // header + token
        assert_eq!(&request[..5], &[0x41, GET, 0x12, 0x34, 0xaa]);
        // option 11 "oic" (delta 11 -> nibble 11, length 3), then delta 0 "d"
        assert_eq!(&request[5..9], &[0xb3, b'o', b'i', b'c']);
        assert_eq!(&request[9..11], &[0x01, b'd']);
        // accept (17): delta 6 from 11 -> nibble 6, length 1, value 60
        assert_eq!(&request[11..13], &[0x61, 60]);
        assert_eq!(request.len(), 13);
    }

    #[test]
    fn parses_an_ack_with_payload() {
        // ACK, token 1 byte 0xAA, code 2.05, option Content-Format(12)=60, payload 0x01
        let datagram = [0x21, 0x45, 0x00, 0x01, 0xaa, 0xc1, 60, 0xff, 0x01];
        let answer = parse_answer(&datagram, &[0xaa]).expect("matching token");
        assert_eq!(answer.code, 0x45);
        assert_eq!(answer.payload, vec![0x01]);
        assert!(answer.block2.is_none());
        // wrong token is ignored
        assert!(parse_answer(&datagram, &[0xbb]).is_none());
    }

    #[test]
    fn reads_block2_more_flag() {
        // option 23 (delta 23), length 1, value 0x0e -> num 0, szx 6, more set (bit 3)
        let datagram = [0x21, 0x45, 0x00, 0x01, 0xaa, 0xd1, 23 - 13, 0x0e, 0xff, 0x02];
        let answer = parse_answer(&datagram, &[0xaa]).expect("matching token");
        assert_eq!(answer.block2, Some((0, true)));
    }

    #[test]
    fn round_trips_a_post_body() {
        let body = json!({ "x": "25.0" });
        let request = encode_request(Request { method: POST, id: 1, token: &[1, 2], path: "/temperatures/vs/0", body: Some(&cbor::encode(&body)), block2: None });
        let marker = request.iter().position(|byte| *byte == 0xff).expect("payload marker");
        assert_eq!(cbor::decode(&request[marker + 1..]).unwrap(), body);
    }
}
