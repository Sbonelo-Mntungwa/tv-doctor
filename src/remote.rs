//! The Android TV remote protocol (v2), which makes this Mac a remote control.
//!
//! This is what the Google TV phone app speaks, and it works on TVs where ADB
//! is locked away. Two TLS ports:
//!
//! - 6467 pairs. The TV shows a six character code, and both sides prove they
//!   saw it by hashing it together with their public keys.
//! - 6466 is the remote itself: key presses, app links, and the TV reporting
//!   its power, volume and foreground app.
//!
//! The TV knows us by a client certificate made once and kept in the config
//! folder. As with Cast, TLS goes through `openssl s_client` rather than a
//! dependency, and the protobuf messages are encoded by hand.

use std::fs;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::cast::openssl_path;
use crate::config::config_dir;
use crate::proto::{self, field_bytes, field_string, field_varint, get_bytes, get_int, varint};

pub const REMOTE_PORT: u16 = 6466;
pub const PAIR_PORT: u16 = 6467;

/// The feature bits the phone app announces. The TV only sends power, volume
/// and app updates to a client that asks for them.
const FEATURES: u64 = 622;

/// Key name, Android key code. Names are what the CLI and the app window use.
pub const KEYS: &[(&str, u32)] = &[
    ("up", 19),
    ("down", 20),
    ("left", 21),
    ("right", 22),
    ("ok", 23),
    ("back", 4),
    ("home", 3),
    ("menu", 82),
    ("settings", 176),
    ("power", 26),
    ("volup", 24),
    ("voldown", 25),
    ("mute", 164),
    ("play", 85),
    ("rewind", 89),
    ("forward", 90),
    ("info", 165),
    ("input", 178),
    ("guide", 172),
    ("chup", 166),
    ("chdown", 167),
    ("delete", 67),
    ("0", 7),
    ("1", 8),
    ("2", 9),
    ("3", 10),
    ("4", 11),
    ("5", 12),
    ("6", 13),
    ("7", 14),
    ("8", 15),
    ("9", 16),
];

pub fn key_code(name: &str) -> Option<u32> {
    let name = name.to_ascii_lowercase();
    let name = match name.as_str() {
        "enter" | "select" | "center" => "ok",
        "esc" | "escape" => "back",
        other => other,
    };
    KEYS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// How a key is pressed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Press {
    Short,
    /// Held down for this long, for TVs that do something different on a long
    /// press of Back or Power.
    Hold(Duration),
}

/// One step of a key sequence, as typed on the command line or sent by the app.
#[derive(Clone, PartialEq, Debug)]
pub enum Step {
    Key(u32, Press),
    Wait(Duration),
}

/// Parse one token: a key name, a run of digits (each pressed in turn),
/// `hold:NAME` or `hold:NAME:SECONDS`, or `wait:SECONDS`.
pub fn parse_step(token: &str) -> Result<Vec<Step>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(rest) = token.strip_prefix("wait:") {
        let secs = parse_seconds(rest)?;
        return Ok(vec![Step::Wait(secs)]);
    }
    if let Some(rest) = token.strip_prefix("hold:") {
        let (name, secs) = match rest.split_once(':') {
            Some((name, secs)) => (name, parse_seconds(secs)?),
            None => (rest, Duration::from_secs(2)),
        };
        let code = key_code(name).ok_or(format!("unknown key '{}'", name))?;
        return Ok(vec![Step::Key(code, Press::Hold(secs))]);
    }
    if token.len() > 1 && token.chars().all(|c| c.is_ascii_digit()) {
        return Ok(token
            .chars()
            .map(|c| Step::Key(key_code(&c.to_string()).unwrap(), Press::Short))
            .collect());
    }
    let code = key_code(token).ok_or(format!(
        "unknown key '{}'. Run tvdoctor remote keys for the list",
        token
    ))?;
    Ok(vec![Step::Key(code, Press::Short)])
}

fn parse_seconds(text: &str) -> Result<Duration, String> {
    let secs: f64 = text
        .parse()
        .map_err(|_| format!("'{}' is not a number of seconds", text))?;
    if !(0.0..=30.0).contains(&secs) {
        return Err("hold and wait take 0 to 30 seconds".to_string());
    }
    Ok(Duration::from_millis((secs * 1000.0) as u64))
}

// ---------------------------------------------------------------------------
// our certificate, and the TV we paired with
// ---------------------------------------------------------------------------
fn cert_path() -> std::path::PathBuf {
    config_dir().join("remote-cert.pem")
}

fn key_path() -> std::path::PathBuf {
    config_dir().join("remote-key.pem")
}

fn paired_path() -> std::path::PathBuf {
    config_dir().join("remote-paired")
}

/// Make the client certificate the TV will know us by, once.
fn ensure_cert() -> Result<(), String> {
    if cert_path().exists() && key_path().exists() {
        return Ok(());
    }
    let _ = fs::create_dir_all(config_dir());
    let status = Command::new(openssl_path()?)
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "7300"])
        .args(["-subj", "/CN=atvremote/O=TV Doctor"])
        .arg("-keyout")
        .arg(key_path())
        .arg("-out")
        .arg(cert_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("could not run openssl: {}", e))?;
    if !status.success() {
        return Err("openssl could not make the remote certificate".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(key_path(), fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// True when this Mac has paired with the TV at `ip`.
pub fn is_paired(ip: &str) -> bool {
    fs::read_to_string(paired_path())
        .map(|text| text.lines().any(|line| line.trim() == ip))
        .unwrap_or(false)
}

fn remember_paired(ip: &str) {
    let mut ips: Vec<String> = fs::read_to_string(paired_path())
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l != ip)
        .collect();
    ips.push(ip.to_string());
    let _ = fs::write(paired_path(), ips.join("\n") + "\n");
}

pub fn forget_paired(ip: &str) {
    let ips: Vec<String> = fs::read_to_string(paired_path())
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l != ip)
        .collect();
    let _ = fs::write(paired_path(), ips.join("\n") + if ips.is_empty() { "" } else { "\n" });
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return hex_decode(&format!("0{}", text));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// An RSA public key as the pairing hash wants it: modulus and exponent as
/// big-endian bytes with no leading zeros.
struct PublicKey {
    modulus: Vec<u8>,
    exponent: Vec<u8>,
}

fn strip_zeros(mut bytes: Vec<u8>) -> Vec<u8> {
    while bytes.len() > 1 && bytes[0] == 0 {
        bytes.remove(0);
    }
    bytes
}

/// Read the RSA key out of a PEM certificate with `openssl x509`.
fn public_key(pem: &str) -> Result<PublicKey, String> {
    let openssl = openssl_path()?;
    let run = |flag: &str| -> Result<String, String> {
        let mut child = Command::new(&openssl)
            .args(["x509", "-noout", flag])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not run openssl: {}", e))?;
        child
            .stdin
            .take()
            .ok_or("no stdin on openssl")?
            .write_all(pem.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    };

    let modulus_line = run("-modulus")?;
    let modulus = modulus_line
        .trim()
        .strip_prefix("Modulus=")
        .and_then(hex_decode)
        .ok_or("could not read the certificate's modulus")?;

    // "Exponent: 65537 (0x10001)"
    let text = run("-text")?;
    let exponent: u64 = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Exponent:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .ok_or("could not read the certificate's exponent")?;

    Ok(PublicKey {
        modulus: strip_zeros(modulus),
        exponent: strip_zeros(exponent.to_be_bytes().to_vec()),
    })
}

/// The TV's own certificate, which goes into the pairing hash.
fn server_cert(ip: &str) -> Result<String, String> {
    let mut child = Command::new(openssl_path()?)
        .args(["s_client", "-connect", &format!("{}:{}", ip, PAIR_PORT)])
        .arg("-cert")
        .arg(cert_path())
        .arg("-key")
        .arg(key_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run openssl: {}", e))?;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let _ = child.wait();

    let begin = out
        .find("-----BEGIN CERTIFICATE-----")
        .ok_or("the TV's pairing service did not answer. Is the TV on?")?;
    let end_marker = "-----END CERTIFICATE-----";
    let end = out[begin..]
        .find(end_marker)
        .ok_or("the TV sent a broken certificate")?
        + begin
        + end_marker.len();
    Ok(out[begin..end].to_string() + "\n")
}

/// The secret both sides derive from the code. The TV's code is six hex
/// characters: the first two are a check byte, the last four the nonce.
fn pairing_secret(client: &PublicKey, server: &PublicKey, code: &str) -> Result<Vec<u8>, String> {
    let code = code.trim().to_ascii_uppercase();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("the code is six characters, 0-9 and A-F".to_string());
    }
    let nonce = hex_decode(&code[2..]).ok_or("the code is not hexadecimal")?;
    let check = u8::from_str_radix(&code[..2], 16).map_err(|_| "the code is not hexadecimal")?;

    let mut input = Vec::new();
    input.extend(&client.modulus);
    input.extend(&client.exponent);
    input.extend(&server.modulus);
    input.extend(&server.exponent);
    input.extend(&nonce);
    let hash = sha256(&input);
    if hash[0] != check {
        return Err("that code does not match the one on the TV".to_string());
    }
    Ok(hash.to_vec())
}

// ---------------------------------------------------------------------------
// the TLS pipe
// ---------------------------------------------------------------------------
/// A long lived `openssl s_client` carrying varint-length-prefixed messages.
struct Tls {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    buffer: Mutex<Vec<u8>>,
    alive: AtomicBool,
}

impl Tls {
    fn open(ip: &str, port: u16) -> Result<Arc<Tls>, String> {
        ensure_cert()?;
        let mut child = Command::new(openssl_path()?)
            .args(["s_client", "-quiet", "-connect", &format!("{}:{}", ip, port)])
            .arg("-cert")
            .arg(cert_path())
            .arg("-key")
            .arg(key_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start openssl: {}", e))?;
        let stdin = child.stdin.take().ok_or("no stdin on openssl")?;
        let mut stdout = child.stdout.take().ok_or("no stdout on openssl")?;

        let tls = Arc::new(Tls {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            buffer: Mutex::new(Vec::new()),
            alive: AtomicBool::new(true),
        });
        let reader = Arc::clone(&tls);
        thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if let Ok(mut buffer) = reader.buffer.lock() {
                            buffer.extend_from_slice(&chunk[..read]);
                        }
                    }
                }
            }
            reader.alive.store(false, Ordering::SeqCst);
        });
        Ok(tls)
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn send(&self, message: &[u8]) -> Result<(), String> {
        if !self.alive() {
            return Err("the connection to the TV closed".to_string());
        }
        let mut framed = varint(message.len() as u64);
        framed.extend_from_slice(message);
        let mut stdin = self.stdin.lock().map_err(|_| "remote writer failed")?;
        stdin
            .write_all(&framed)
            .and_then(|_| stdin.flush())
            .map_err(|_| "the connection to the TV closed".to_string())
    }

    /// The next whole message, waiting up to `timeout`. None on timeout or
    /// once the connection has closed and nothing is left.
    fn next(&self, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(mut buffer) = self.buffer.lock() {
                let mut index = 0;
                if let Some(length) = proto::read_varint(&buffer, &mut index) {
                    let end = index + length as usize;
                    if buffer.len() >= end {
                        let message = buffer[index..end].to_vec();
                        buffer.drain(..end);
                        return Some(message);
                    }
                }
            }
            if !self.alive() || Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn close(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.alive.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// pairing (port 6467)
// ---------------------------------------------------------------------------
const STATUS_OK: u64 = 200;

fn pairing_message(body_field: u64, body: &[u8]) -> Vec<u8> {
    let mut message = field_varint(1, 2); // protocol_version
    message.extend(field_varint(2, STATUS_OK));
    message.extend(field_bytes(body_field, body));
    message
}

/// HEXADECIMAL encoding, six symbols.
fn encoding() -> Vec<u8> {
    let mut encoding = field_varint(1, 3);
    encoding.extend(field_varint(2, 6));
    encoding
}

/// Wait for the reply carrying `field`, and fail on any error status.
fn expect(tls: &Tls, field: u64, step: &str) -> Result<(), String> {
    let reply = tls.next(Duration::from_secs(15)).ok_or(format!(
        "the TV stopped answering during pairing ({})",
        step
    ))?;
    let parsed = proto::fields(&reply).ok_or("the TV sent something unreadable")?;
    match get_int(&parsed, 2) {
        Some(STATUS_OK) => {}
        Some(402) => return Err("the TV rejected the code".to_string()),
        Some(status) => return Err(format!("the TV refused pairing (status {})", status)),
        None => return Err("the TV sent a reply with no status".to_string()),
    }
    if get_bytes(&parsed, field).is_none() && !parsed.iter().any(|(n, _)| *n == field) {
        return Err(format!("unexpected reply from the TV during {}", step));
    }
    Ok(())
}

/// A pairing in progress: the TV is showing its code and waiting.
pub struct Pairing {
    tls: Arc<Tls>,
    ip: String,
    client: PublicKey,
    server: PublicKey,
}

/// Ask the TV to pair. When this returns, the code is on the TV screen.
pub fn start_pairing(ip: &str) -> Result<Pairing, String> {
    ensure_cert()?;
    let client = public_key(&fs::read_to_string(cert_path()).map_err(|e| e.to_string())?)?;
    let server = public_key(&server_cert(ip)?)?;

    let tls = Tls::open(ip, PAIR_PORT)?;
    let result = (|| {
        let mut request = field_string(1, "atvremote");
        request.extend(field_string(2, "TV Doctor"));
        tls.send(&pairing_message(10, &request))?;
        expect(&tls, 11, "request")?;

        let mut options = field_bytes(1, &encoding());
        options.extend(field_varint(3, 1)); // preferred role: input
        tls.send(&pairing_message(20, &options))?;
        expect(&tls, 20, "options")?;

        let mut configuration = field_bytes(1, &encoding());
        configuration.extend(field_varint(2, 1)); // client role: input
        tls.send(&pairing_message(30, &configuration))?;
        expect(&tls, 31, "configuration")
    })();

    if let Err(error) = result {
        tls.close();
        return Err(error);
    }
    Ok(Pairing {
        tls,
        ip: ip.to_string(),
        client,
        server,
    })
}

impl Pairing {
    /// Send the code from the TV screen. A typo is caught here, before the TV
    /// sees it, and the pairing stays open for another try.
    pub fn finish(&self, code: &str) -> Result<(), String> {
        let secret = pairing_secret(&self.client, &self.server, code)?;
        self.tls.send(&pairing_message(40, &field_bytes(1, &secret)))?;
        expect(&self.tls, 41, "code")?;
        remember_paired(&self.ip);
        self.tls.close();
        Ok(())
    }

    pub fn cancel(&self) {
        self.tls.close();
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        self.tls.close();
    }
}

// ---------------------------------------------------------------------------
// the remote (port 6466)
// ---------------------------------------------------------------------------
/// What the TV has told us about itself.
#[derive(Clone, Default)]
pub struct State {
    pub ready: bool,
    pub on: Option<bool>,
    pub volume: Option<u64>,
    pub volume_max: Option<u64>,
    pub muted: Option<bool>,
    /// Package name of the app in front, when the TV says.
    pub app: String,
    pub vendor: String,
    pub model: String,
}

pub struct Remote {
    tls: Arc<Tls>,
    state: Arc<Mutex<State>>,
}

fn configure_reply() -> Vec<u8> {
    let mut device = field_string(1, "TV Doctor");
    device.extend(field_string(2, "TV Doctor"));
    device.extend(field_varint(3, 1));
    device.extend(field_string(4, "1"));
    device.extend(field_string(5, "atvremote"));
    device.extend(field_string(6, "1.0.0"));
    let mut configure = field_varint(1, FEATURES);
    configure.extend(field_bytes(2, &device));
    field_bytes(1, &configure)
}

/// Answer the TV and keep `state` current, until the connection closes.
fn session(tls: Arc<Tls>, state: Arc<Mutex<State>>) {
    loop {
        let Some(message) = tls.next(Duration::from_millis(500)) else {
            if !tls.alive() {
                break;
            }
            continue;
        };
        let Some(parsed) = proto::fields(&message) else {
            continue;
        };
        for (field, value) in &parsed {
            let proto::Value::Bytes(body) = value else {
                continue;
            };
            let inner = proto::fields(body).unwrap_or_default();
            match field {
                // remote_configure: the TV introduces itself, we answer in kind.
                1 => {
                    if let Some(info) = get_bytes(&inner, 2).and_then(proto::fields) {
                        if let Ok(mut s) = state.lock() {
                            s.model = text(get_bytes(&info, 1));
                            s.vendor = text(get_bytes(&info, 2));
                        }
                    }
                    let _ = tls.send(&configure_reply());
                }
                // remote_set_active: after this, key presses are accepted.
                2 => {
                    let _ = tls.send(&field_bytes(2, &field_varint(1, FEATURES)));
                    if let Ok(mut s) = state.lock() {
                        s.ready = true;
                    }
                }
                // ping: an unanswered ping gets us disconnected.
                8 => {
                    let value = get_int(&inner, 1).unwrap_or(0);
                    let _ = tls.send(&field_bytes(9, &field_varint(1, value)));
                }
                // ime_key_inject carries the foreground app.
                20 => {
                    if let Some(app) = get_bytes(&inner, 1).and_then(proto::fields) {
                        let package = text(get_bytes(&app, 12));
                        if !package.is_empty() {
                            if let Ok(mut s) = state.lock() {
                                s.app = package;
                            }
                        }
                    }
                }
                // remote_start: power.
                40 => {
                    if let Ok(mut s) = state.lock() {
                        s.on = Some(get_int(&inner, 1).unwrap_or(0) != 0);
                    }
                }
                // set_volume_level
                50 => {
                    if let Ok(mut s) = state.lock() {
                        s.volume_max = get_int(&inner, 6).or(s.volume_max);
                        s.volume = Some(get_int(&inner, 7).unwrap_or(0));
                        s.muted = Some(get_int(&inner, 8).unwrap_or(0) != 0);
                    }
                }
                _ => {}
            }
        }
    }
    if let Ok(mut s) = state.lock() {
        s.ready = false;
    }
}

fn text(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default()
}

impl Remote {
    /// Connect and finish the handshake. Fails if this Mac is not paired.
    pub fn connect(ip: &str) -> Result<Remote, String> {
        let tls = Tls::open(ip, REMOTE_PORT)?;
        let state = Arc::new(Mutex::new(State::default()));
        {
            let tls = Arc::clone(&tls);
            let state = Arc::clone(&state);
            thread::spawn(move || session(tls, state));
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if state.lock().map(|s| s.ready).unwrap_or(false) {
                return Ok(Remote { tls, state });
            }
            if !tls.alive() {
                return Err(
                    "the TV closed the remote connection. Pair this Mac first: tvdoctor remote pair"
                        .to_string(),
                );
            }
            if Instant::now() >= deadline {
                tls.close();
                return Err("the TV did not finish the remote handshake".to_string());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn alive(&self) -> bool {
        self.tls.alive() && self.state.lock().map(|s| s.ready).unwrap_or(false)
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// direction: 1 starts a long press, 2 ends it, 3 is a short press.
    fn inject(&self, code: u32, direction: u64) -> Result<(), String> {
        let mut key = field_varint(1, code as u64);
        key.extend(field_varint(2, direction));
        self.tls.send(&field_bytes(10, &key))
    }

    pub fn press(&self, code: u32, press: Press) -> Result<(), String> {
        match press {
            Press::Short => self.inject(code, 3),
            Press::Hold(length) => {
                self.inject(code, 1)?;
                thread::sleep(length);
                self.inject(code, 2)
            }
        }
    }

    pub fn run(&self, steps: &[Step]) -> Result<(), String> {
        for step in steps {
            match step {
                Step::Key(code, press) => {
                    self.press(*code, *press)?;
                    // The TV drops presses that arrive back to back.
                    thread::sleep(Duration::from_millis(180));
                }
                Step::Wait(length) => thread::sleep(*length),
            }
        }
        Ok(())
    }

    /// Open a link on the TV, such as a Play Store page or an app's own URL.
    pub fn open_link(&self, link: &str) -> Result<(), String> {
        self.tls.send(&field_bytes(90, &field_string(1, link)))
    }

    pub fn close(&self) {
        self.tls.close();
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.tls.close();
    }
}

// ---------------------------------------------------------------------------
// SHA-256, for the pairing secret
// ---------------------------------------------------------------------------
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());

    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(add);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn sha256_known_answers() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Two blocks of padding.
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn pairing_secret_checks_the_first_byte() {
        let client = PublicKey { modulus: vec![0xC1; 4], exponent: vec![1, 0, 1] };
        let server = PublicKey { modulus: vec![0x5E; 4], exponent: vec![1, 0, 1] };
        let mut input = Vec::new();
        input.extend(&client.modulus);
        input.extend(&client.exponent);
        input.extend(&server.modulus);
        input.extend(&server.exponent);
        input.extend([0xAB, 0xCD]);
        let check = sha256(&input)[0];

        let good = format!("{:02X}ABCD", check);
        assert_eq!(pairing_secret(&client, &server, &good).unwrap(), sha256(&input).to_vec());
        assert_eq!(
            pairing_secret(&client, &server, &good.to_lowercase()).unwrap(),
            sha256(&input).to_vec()
        );

        let bad = format!("{:02X}ABCD", check.wrapping_add(1));
        assert!(pairing_secret(&client, &server, &bad).is_err());
        assert!(pairing_secret(&client, &server, "12345").is_err());
        assert!(pairing_secret(&client, &server, "12345G").is_err());
    }

    #[test]
    fn exponent_has_no_leading_zeros() {
        assert_eq!(strip_zeros(65537u64.to_be_bytes().to_vec()), vec![1, 0, 1]);
        assert_eq!(hex_decode("10001").unwrap(), vec![1, 0, 1]);
    }

    #[test]
    fn steps_parse() {
        assert_eq!(parse_step("ok").unwrap(), vec![Step::Key(23, Press::Short)]);
        assert_eq!(parse_step("Enter").unwrap(), vec![Step::Key(23, Press::Short)]);
        assert_eq!(
            parse_step("1969").unwrap(),
            vec![
                Step::Key(8, Press::Short),
                Step::Key(16, Press::Short),
                Step::Key(13, Press::Short),
                Step::Key(16, Press::Short),
            ]
        );
        assert_eq!(
            parse_step("hold:back:15").unwrap(),
            vec![Step::Key(4, Press::Hold(Duration::from_secs(15)))]
        );
        assert_eq!(parse_step("wait:0.5").unwrap(), vec![Step::Wait(Duration::from_millis(500))]);
        assert!(parse_step("launch-missiles").is_err());
        assert!(parse_step("hold:back:99").is_err());
    }
}
