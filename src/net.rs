//! Finding the TV: port probes, the Cast/DIAL description, and a subnet sweep.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

pub const ADB_PORT: u16 = 5555;
pub const CAST_PORT: u16 = 8008;
pub const REMOTE_PORT: u16 = 6466;

#[derive(Clone, Default)]
pub struct Found {
    pub ip: String,
    pub cast: bool,
    pub remote: bool,
    pub adb: bool,
    pub friendly_name: String,
    pub manufacturer: String,
    pub model: String,
}

impl Found {
    pub fn describe(&self) -> String {
        let make = format!("{} {}", self.manufacturer, self.model)
            .trim()
            .to_string();
        if make.is_empty() {
            "unknown device".to_string()
        } else {
            make
        }
    }
}

/// This laptop's address on the network it routes through.
pub fn local_ipv4() -> String {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return "127.0.0.1".to_string();
    };
    if sock.connect("10.255.255.255:1").is_err() {
        return "127.0.0.1".to_string();
    }
    sock.local_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

pub fn port_open(ip: &str, port: u16, timeout_ms: u64) -> bool {
    let Ok(addr) = ip.parse::<IpAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddr::new(addr, port),
        Duration::from_millis(timeout_ms),
    )
    .is_ok()
}

/// The laptop's default gateway, as the routing table reports it.
pub fn default_gateway() -> Option<String> {
    let output = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.trim().strip_prefix("gateway:"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Can this process reach the local network at all?
///
/// This is the difference between a television in standby and a macOS Local
/// Network permission that has not been granted. The router answers on at
/// least one of these whenever the network is really available, so failing all
/// of them means our packets are not leaving the machine.
pub fn lan_reachable() -> bool {
    let Some(gateway) = default_gateway() else {
        // Nothing to compare against, so do not accuse macOS of anything.
        return true;
    };
    [80u16, 443, 53]
        .iter()
        .any(|port| port_open(&gateway, *port, 1200))
}

/// Minimal HTTP/1.1 GET.
pub fn http_get(ip: &str, port: u16, path: &str, timeout_ms: u64) -> Option<String> {
    http_request("GET", ip, port, path, timeout_ms)
}

/// Launch an app on the TV through DIAL. No ADB needed. Returns the status line.
pub fn dial_launch(ip: &str, app: &str) -> Option<String> {
    let reply = http_request("POST", ip, CAST_PORT, &format!("/apps/{}", app), 8000)?;
    reply.lines().next().map(|l| l.trim().to_string())
}

/// The session token for a running DIAL app, from <link rel="run" href="...">.
/// It is a fresh UUID per launch, so it has to be read rather than assumed.
pub fn dial_run_token(ip: &str, app: &str) -> Option<String> {
    let body = http_get(ip, CAST_PORT, &format!("/apps/{}", app), 5000)?;
    let link_start = body.find("rel=\"run\"")?;
    let tail = &body[link_start..];
    let href_start = tail.find("href=\"")? + 6;
    let rest = &tail[href_start..];
    let end = rest.find('"')?;
    let token = rest[..end].trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Stop a running app through DIAL. Only works where the app's DIAL record says
/// allowStop="true".
pub fn dial_stop(ip: &str, app: &str) -> Option<String> {
    let token = dial_run_token(ip, app)?;
    let path = format!("/apps/{}/{}", app, token);
    let reply = http_request("DELETE", ip, CAST_PORT, &path, 8000)?;
    reply.lines().next().map(|l| l.trim().to_string())
}

/// Minimal HTTP/1.1 request with no body.
pub fn http_request(
    method: &str,
    ip: &str,
    port: u16,
    path: &str,
    timeout_ms: u64,
) -> Option<String> {
    http_request_with(method, ip, port, path, &[], timeout_ms)
}

/// The same, with extra headers.
pub fn http_request_with(
    method: &str,
    ip: &str,
    port: u16,
    path: &str,
    headers: &[(&str, &str)],
    timeout_ms: u64,
) -> Option<String> {
    let addr: IpAddr = ip.parse().ok()?;
    let timeout = Duration::from_millis(timeout_ms);
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::new(addr, port), timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    // HTTP/1.1, not 1.0: the TV's Cast server closes a 1.0 request with an
    // empty reply. Connection: close asks it to hang up once it has answered,
    // and body_complete below stops us waiting if it does not.
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{}: {}\r\n", name, value))
        .collect();
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: tvdoctor/1.0\r\n\
         Accept: */*\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n",
        method, path, ip, port, extra
    );
    stream.write_all(request.as_bytes()).ok()?;

    // Read until the peer closes, we have the whole body, or we run out of
    // time. read_to_end is no good here: the TV holds the connection open
    // after answering, so waiting for EOF loses everything already read.
    let deadline = Instant::now() + timeout;
    let mut body: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if body_complete(&body) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if body.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&body).to_string())
}

/// A string, number or boolean field out of flat JSON, returned as text.
pub fn json_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let colon = rest.find(':')? + 1;
    let value = rest[colon..].trim_start();
    if let Some(quoted) = value.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some(quoted[..end].to_string());
    }
    let end = value
        .find(|c| c == ',' || c == '}' || c == '\n')
        .unwrap_or(value.len());
    Some(value[..end].trim().to_string())
}

/// The Cast endpoint's own status blob.
pub fn eureka(ip: &str) -> Option<String> {
    http_get(ip, CAST_PORT, "/setup/eureka_info?options=detail", 5000)
}

/// What DIAL knows about an app.
///
/// The third case matters: a query that never got an answer is not evidence
/// that the app has no receiver, and reporting it as such sends you looking for
/// the wrong problem.
pub enum DialApp {
    Registered { name: String, state: String },
    NotRegistered,
    NoAnswer,
}

/// Ask DIAL about one app, keeping "did not answer" separate from "no".
pub fn dial_app_state(ip: &str, app: &str) -> DialApp {
    let Some(body) = http_get(ip, CAST_PORT, &format!("/apps/{}", app), 4000) else {
        return DialApp::NoAnswer;
    };
    match xml_tag(&body, "name") {
        Some(name) => DialApp::Registered {
            name,
            state: xml_tag(&body, "state").unwrap_or_default(),
        },
        // A 404, or a 200 with nothing useful in it.
        None => DialApp::NotRegistered,
    }
}

/// Ask DIAL about one app: (name, state). state is "running", "stopped" or
/// "installable" depending on whether the TV has it.
pub fn dial_app(ip: &str, app: &str) -> Option<(String, String)> {
    let body = http_get(ip, CAST_PORT, &format!("/apps/{}", app), 4000)?;
    let name = xml_tag(&body, "name")?;
    let state = xml_tag(&body, "state").unwrap_or_default();
    Some((name, state))
}

/// True once the headers name a Content-Length and that many bytes have
/// arrived, so we can stop instead of waiting on a connection the TV keeps open.
fn body_complete(buffer: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buffer);
    let Some(split) = text.find("\r\n\r\n").map(|i| i + 4) else {
        return false;
    };
    let length = text[..split]
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
    match length {
        Some(length) => buffer.len() >= split + length,
        None => false,
    }
}

fn xml_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim().to_string())
}

/// Ask the Cast endpoint who it is.
pub fn dial_describe(ip: &str) -> Option<(String, String, String)> {
    let xml = http_get(ip, CAST_PORT, "/ssdp/device-desc.xml", 4000)?;
    Some((
        xml_tag(&xml, "friendlyName").unwrap_or_default(),
        xml_tag(&xml, "manufacturer").unwrap_or_default(),
        xml_tag(&xml, "modelName").unwrap_or_default(),
    ))
}

fn probe_host(ip: String) -> Option<Found> {
    let cast = port_open(&ip, CAST_PORT, 700);
    let remote = port_open(&ip, REMOTE_PORT, 700);
    if !cast && !remote {
        return None;
    }
    let mut found = Found {
        adb: port_open(&ip, ADB_PORT, 700),
        ip,
        cast,
        remote,
        ..Default::default()
    };
    if cast {
        if let Some((name, maker, model)) = dial_describe(&found.ip) {
            found.friendly_name = name;
            found.manufacturer = maker;
            found.model = model;
        }
    }
    Some(found)
}

/// Sweep a /24 for anything that answers like an Android TV.
pub fn discover(subnet: Option<&str>) -> Vec<Found> {
    let base = match subnet {
        Some(s) => s.trim_end_matches('.').to_string(),
        None => {
            let ip = local_ipv4();
            let mut parts: Vec<&str> = ip.split('.').collect();
            parts.pop();
            parts.join(".")
        }
    };

    let mut handles = Vec::new();
    for host in 1..255u8 {
        let ip = format!("{}.{}", base, host);
        let handle = thread::Builder::new()
            .stack_size(192 * 1024)
            .spawn(move || probe_host(ip))
            .ok();
        if let Some(handle) = handle {
            handles.push(handle);
        }
    }

    let mut results: Vec<Found> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();
    results.sort_by(|a, b| octet(&a.ip).cmp(&octet(&b.ip)));
    results
}

fn octet(ip: &str) -> u32 {
    ip.rsplit('.').next().and_then(|s| s.parse().ok()).unwrap_or(0)
}
