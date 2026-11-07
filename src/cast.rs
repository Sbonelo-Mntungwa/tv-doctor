//! Cast v2 (port 8009), which is how YouTube and Spotify get launched.
//!
//! Cast v2 is TLS carrying length-prefixed protobuf frames whose payload is
//! JSON. Rather than take on a TLS dependency, this pipes frames through
//! `openssl s_client`, which is already on the machine. The protobuf messages
//! involved are small enough to encode by hand.
//!
//! No pairing is needed, unlike the Android TV remote protocol in remote.rs,
//! which is why launching apps goes this way.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::proto::{field_string, field_varint, read_varint};

pub const CAST_PORT: u16 = 8009;
const CONNECT_NS: &str = "urn:x-cast:com.google.cast.tp.connection";
const RECEIVER_NS: &str = "urn:x-cast:com.google.cast.receiver";

/// label, Cast application id, DIAL name (empty when the app has none).
pub const APPS: &[(&str, &str, &str)] = &[
    ("YouTube", "233637DE", ""),
    ("Netflix", "CA5E8412", "Netflix"),
    ("Spotify", "CC32E753", ""),
    ("DStv", "", ""),
];

pub fn app_entry(label: &str) -> Option<(&'static str, &'static str, &'static str)> {
    APPS.iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(label))
        .copied()
}

// ---------------------------------------------------------------------------
// frames
// ---------------------------------------------------------------------------
/// One CastMessage, length prefixed as the wire format wants.
fn frame(destination: &str, namespace: &str, payload: &str) -> Vec<u8> {
    let mut message = Vec::new();
    message.extend(field_varint(1, 0)); // protocol_version CASTV2_1_0
    message.extend(field_string(2, "sender-0"));
    message.extend(field_string(3, destination));
    message.extend(field_string(4, namespace));
    message.extend(field_varint(5, 0)); // payload_type STRING
    message.extend(field_string(6, payload));

    let mut out = (message.len() as u32).to_be_bytes().to_vec();
    out.extend(message);
    out
}

/// The JSON payload (field 6) of one CastMessage.
fn payload_of(body: &[u8]) -> Option<String> {
    let mut index = 0;
    while index < body.len() {
        let key = read_varint(body, &mut index)?;
        let (number, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                read_varint(body, &mut index)?;
            }
            2 => {
                let length = read_varint(body, &mut index)? as usize;
                let end = index + length;
                let chunk = body.get(index..end)?;
                index = end;
                if number == 6 {
                    return Some(String::from_utf8_lossy(chunk).to_string());
                }
            }
            _ => return None,
        }
    }
    None
}

/// Every complete frame in the buffer, and how many bytes were consumed.
fn parse_frames(buffer: &[u8]) -> (Vec<String>, usize) {
    let mut payloads = Vec::new();
    let mut offset = 0;
    while offset + 4 <= buffer.len() {
        let length = u32::from_be_bytes([
            buffer[offset],
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
        ]) as usize;
        if offset + 4 + length > buffer.len() {
            break;
        }
        if let Some(payload) = payload_of(&buffer[offset + 4..offset + 4 + length]) {
            payloads.push(payload);
        }
        offset += 4 + length;
    }
    (payloads, offset)
}

// ---------------------------------------------------------------------------
// the TLS pipe
// ---------------------------------------------------------------------------
pub fn openssl_path() -> Result<String, String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = format!("{}/openssl", dir);
            if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false) {
                return Ok(candidate);
            }
        }
    }
    for candidate in ["/opt/homebrew/bin/openssl", "/usr/bin/openssl"] {
        if std::fs::metadata(candidate).map(|m| m.is_file()).unwrap_or(false) {
            return Ok(candidate.to_string());
        }
    }
    Err("openssl not found, which TV Doctor needs for TLS".to_string())
}

/// Send frames and collect replies until one contains `until`, or time runs out.
///
/// openssl ignores end of input and holds the connection open, so the child is
/// killed once we have what we came for.
fn exchange(
    ip: &str,
    frames: &[Vec<u8>],
    until: &str,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let openssl = openssl_path()?;
    let mut child = Command::new(openssl)
        .args([
            "s_client",
            "-quiet",
            "-connect",
            &format!("{}:{}", ip, CAST_PORT),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start openssl: {}", e))?;

    let mut stdin = child.stdin.take().ok_or("no stdin on openssl")?;
    let mut stdout = child.stdout.take().ok_or("no stdout on openssl")?;

    let shared: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let collector = Arc::clone(&shared);
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut buffer) = collector.lock() {
                        buffer.extend_from_slice(&chunk[..read]);
                    }
                }
            }
        }
    });

    for payload in frames {
        if stdin.write_all(payload).is_err() {
            break;
        }
    }
    let _ = stdin.flush();

    let deadline = Instant::now() + timeout;
    let mut replies: Vec<String> = Vec::new();
    let mut consumed = 0usize;
    while Instant::now() < deadline {
        {
            let buffer = shared.lock().map_err(|_| "cast reader failed")?;
            let (payloads, used) = parse_frames(&buffer[consumed..]);
            if used > 0 {
                consumed += used;
                for payload in payloads {
                    if !payload.contains("\"PING\"") {
                        replies.push(payload);
                    }
                }
            }
        }
        if !until.is_empty() && replies.iter().any(|r| r.contains(until)) {
            break;
        }
        thread::sleep(Duration::from_millis(60));
    }

    let _ = child.kill();
    let _ = child.wait();
    drop(stdin);
    Ok(replies)
}

fn connect_frame() -> Vec<u8> {
    frame("receiver-0", CONNECT_NS, "{\"type\":\"CONNECT\"}")
}

/// Which of the Cast app ids this TV can actually run.
pub fn availability(ip: &str, app_ids: &[&str]) -> Result<Vec<(String, bool)>, String> {
    let list = app_ids
        .iter()
        .map(|id| format!("\"{}\"", id))
        .collect::<Vec<_>>()
        .join(",");
    let payload = format!(
        "{{\"type\":\"GET_APP_AVAILABILITY\",\"requestId\":2,\"appId\":[{}]}}",
        list
    );
    let replies = exchange(
        ip,
        &[
            connect_frame(),
            frame("receiver-0", RECEIVER_NS, &payload),
        ],
        "GET_APP_AVAILABILITY",
        Duration::from_secs(8),
    )?;

    // No reply at all means the TV did not answer, which is different from the
    // app not being there. Saying "unavailable" here would be a guess.
    let body = replies
        .iter()
        .find(|r| r.contains("availability"))
        .cloned()
        .ok_or("the TV did not answer the Cast availability request")?;
    Ok(app_ids
        .iter()
        .map(|id| {
            let key = format!("\"{}\":\"APP_AVAILABLE\"", id);
            (id.to_string(), body.contains(&key))
        })
        .collect())
}

/// What is on screen now: (display name, app id, session id, is idle screen).
pub fn status(ip: &str) -> Result<Option<(String, String, String, bool)>, String> {
    let replies = exchange(
        ip,
        &[
            connect_frame(),
            frame(
                "receiver-0",
                RECEIVER_NS,
                "{\"type\":\"GET_STATUS\",\"requestId\":9}",
            ),
        ],
        "\"requestId\":9",
        Duration::from_secs(8),
    )?;

    let Some(body) = replies.iter().find(|r| r.contains("applications")) else {
        return Ok(None);
    };
    let name = crate::net::json_field(body, "displayName").unwrap_or_default();
    let app_id = crate::net::json_field(body, "appId").unwrap_or_default();
    let session = crate::net::json_field(body, "sessionId").unwrap_or_default();
    let idle = crate::net::json_field(body, "isIdleScreen")
        .map(|v| v == "true")
        .unwrap_or(false);
    if name.is_empty() {
        return Ok(None);
    }
    Ok(Some((name, app_id, session, idle)))
}

/// Start an app by its Cast id. Returns its display name.
pub fn launch(ip: &str, app_id: &str) -> Result<String, String> {
    let payload = format!(
        "{{\"type\":\"LAUNCH\",\"requestId\":1,\"appId\":\"{}\"}}",
        app_id
    );
    let replies = exchange(
        ip,
        &[connect_frame(), frame("receiver-0", RECEIVER_NS, &payload)],
        "\"requestId\":1",
        Duration::from_secs(12),
    )?;

    if let Some(body) = replies.iter().find(|r| r.contains("applications")) {
        return Ok(crate::net::json_field(body, "displayName").unwrap_or_default());
    }
    if let Some(body) = replies.iter().find(|r| r.contains("LAUNCH_ERROR")) {
        return Err(format!("the TV refused the launch: {}", body));
    }
    Err("the TV did not confirm the launch".to_string())
}

/// Stop whatever Cast session is running, which hands back its memory.
pub fn stop(ip: &str, session_id: &str) -> Result<(), String> {
    let payload = format!(
        "{{\"type\":\"STOP\",\"requestId\":3,\"sessionId\":\"{}\"}}",
        session_id
    );
    exchange(
        ip,
        &[connect_frame(), frame("receiver-0", RECEIVER_NS, &payload)],
        "\"requestId\":3",
        Duration::from_secs(10),
    )?;
    Ok(())
}
