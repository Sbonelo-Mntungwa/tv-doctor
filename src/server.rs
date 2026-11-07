//! The desktop app: a local web interface served on 127.0.0.1.
//!
//! Each button runs this same binary as a subcommand and streams its output
//! into the log pane, so the app and the command line can never drift apart.
//! Only names from ACTIONS are ever run, so nothing a browser sends can turn
//! into an arbitrary command.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::cmds::Opts;
use crate::nurse;
use crate::config;
use crate::net::{self, ADB_PORT};
use crate::remote;

const PAGE: &str = include_str!("app.html");

/// Button name -> the arguments this binary is run with. A whitelist, so a
/// request can never name anything else.
fn actions() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        ("status", vec!["status"]),
        ("remote-check", vec!["remote-check"]),
        ("routes", vec!["routes"]),
        ("guide", vec!["guide"]),
        ("report", vec!["report"]),
        ("clean", vec!["clean"]),
        ("optimize", vec!["optimize"]),
        ("bloat", vec!["bloat"]),
        ("bloat-auto", vec!["bloat", "--auto", "--yes"]),
        ("update", vec!["update"]),
        ("full", vec!["full"]),
        ("reboot", vec!["reboot", "--yes"]),
        ("restore", vec!["restore", "--yes"]),
        ("nurse-status", vec!["nurse", "status"]),
        ("nurse-report", vec!["nurse", "report"]),
        ("nurse-free", vec!["nurse", "free"]),
    ])
}

#[derive(Default)]
struct Job {
    lines: Vec<String>,
    running: bool,
    label: String,
    /// The child's pid while it runs, so the Stop button can end it.
    pid: Option<u32>,
    /// The child's stdin, so the window can answer its questions.
    stdin: Option<std::process::ChildStdin>,
}

type Shared = Arc<Mutex<Job>>;

/// The last known TV state as JSON, refreshed on a timer.
///
/// Probing inside the request made /api/state take as long as twelve seconds
/// once the gateway check was added, long enough for the window's header to
/// time out. The browser now always gets an immediate answer, at the cost of
/// it being a few seconds old.
type Cache = Arc<Mutex<String>>;

/// The home screen's network search, run off the request path so the page can
/// show progress and poll for the answer.
#[derive(Default)]
struct Scan {
    running: bool,
    finished: bool,
    subnet: String,
    /// False when nothing answered and the router did not either, which points
    /// at macOS Local Network permission rather than a TV in standby.
    lan: bool,
    results: Vec<net::Found>,
}

type ScanShared = Arc<Mutex<Scan>>;

/// The remote control: one connection to the TV kept open while the window
/// is, so a button press does not pay for a TLS handshake each time.
#[derive(Default)]
struct RemoteSlot {
    /// The TV the session or pairing belongs to. Changing TV drops both.
    ip: String,
    session: Option<Arc<remote::Remote>>,
    pairing: Option<Arc<remote::Pairing>>,
    connecting: bool,
    error: String,
}

type RemoteShared = Arc<Mutex<RemoteSlot>>;

/// The live session for the remembered TV, if there is one.
fn live_session(slot: &mut RemoteSlot, ip: &str) -> Option<Arc<remote::Remote>> {
    if slot.ip != ip {
        if let Some(old) = slot.session.take() {
            old.close();
        }
        if let Some(old) = slot.pairing.take() {
            old.cancel();
        }
        slot.ip = ip.to_string();
        slot.error.clear();
    }
    match &slot.session {
        Some(session) if session.alive() => Some(Arc::clone(session)),
        _ => {
            slot.session = None;
            None
        }
    }
}

/// Connect outside the lock, so status requests keep answering meanwhile.
fn connect_remote(remote_slot: &RemoteShared, ip: &str) -> Result<Arc<remote::Remote>, String> {
    {
        let mut slot = remote_slot.lock().unwrap();
        if let Some(session) = live_session(&mut slot, ip) {
            return Ok(session);
        }
        slot.connecting = true;
    }
    let result = remote::Remote::connect(ip).map(Arc::new);
    let mut slot = remote_slot.lock().unwrap();
    slot.connecting = false;
    match &result {
        Ok(session) if slot.ip == ip => {
            slot.session = Some(Arc::clone(session));
            slot.error.clear();
        }
        Ok(_) => {}
        Err(message) => slot.error = message.clone(),
    }
    result
}

fn opt_json<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string())
}

fn remote_json(remote_slot: &RemoteShared) -> String {
    let ip = config::load().ip.unwrap_or_default();
    let paired = !ip.is_empty() && remote::is_paired(&ip);
    let mut slot = remote_slot.lock().unwrap();
    let session = live_session(&mut slot, &ip);

    // Reconnect in the background whenever the page asks and we are paired.
    if paired && session.is_none() && !slot.connecting && slot.pairing.is_none() {
        slot.connecting = true;
        let shared = Arc::clone(remote_slot);
        let target = ip.clone();
        thread::spawn(move || {
            {
                shared.lock().unwrap().connecting = false;
            }
            let _ = connect_remote(&shared, &target);
        });
    }

    let state = session.as_ref().map(|s| s.state()).unwrap_or_default();
    format!(
        "{{\"paired\":{},\"connected\":{},\"connecting\":{},\"pairing\":{},\"on\":{},\
         \"volume\":{},\"max\":{},\"muted\":{},\"app\":\"{}\",\"error\":\"{}\"}}",
        paired,
        session.is_some(),
        slot.connecting,
        slot.pairing.is_some(),
        opt_json(state.on),
        opt_json(state.volume),
        opt_json(state.volume_max),
        opt_json(state.muted),
        escape_json(&state.app),
        escape_json(&slot.error)
    )
}

fn json_error(stream: &mut TcpStream, message: &str) {
    let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(message));
    respond(stream, "200 OK", "application/json", &body);
}

/// One request header's value, by name, ignoring case.
fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
}

/// True when a request comes from TV Doctor's own page.
///
/// Any web page open in the browser can send requests to 127.0.0.1, so the
/// API needs more than "it came from this Mac". The page adds an X-TVDoctor
/// header to every call: a browser will not let another site add a custom
/// header to a request here without asking this server first, and this server
/// never says yes. The Host check stops a site that points its own name at
/// 127.0.0.1 (DNS rebinding).
fn from_our_page(request: &str) -> bool {
    let host_ok = header(request, "Host")
        .map(|h| h.starts_with("127.0.0.1:") || h.starts_with("localhost:"))
        .unwrap_or(false);
    host_ok && header(request, "X-TVDoctor") == Some("1")
}

/// Commands that may be typed into the Monitor.
const TYPED: &[&str] = &[
    "status", "guide", "remote-check", "check", "routes", "launch", "stop", "force-stop", "home",
    "discover", "connect", "report", "clean", "optimize", "optimise", "tune", "bloat", "restore",
    "update", "apps", "full", "reboot", "remote", "nurse", "helper", "enable-adb",
];

/// A line typed into the Monitor, as arguments for this binary. Only known
/// commands, and arguments made of ordinary characters; the binary is run
/// directly, never through a shell.
fn parse_typed(line: &str) -> Result<Vec<String>, String> {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if words.first() == Some(&"tvdoctor") {
        words.remove(0);
    }
    let Some(first) = words.first() else {
        return Err("type a command, or help".to_string());
    };
    if matches!(*first, "help" | "--help" | "-h" | "?") {
        return Ok(vec!["--help".to_string()]);
    }
    if *first == "shell" {
        return Err("shell is not run from the window. Use the terminal".to_string());
    }
    if matches!(*first, "app" | "gui" | "menu") {
        return Err("that opens another window. Use the terminal".to_string());
    }
    if !TYPED.contains(first) {
        return Err(format!("unknown command '{}'. Type help for the list", first));
    }
    if words.len() > 16 {
        return Err("that is a lot of words. Try a shorter command".to_string());
    }
    for word in &words {
        let fine = word.len() <= 200
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._,:/=-?&%+@".contains(c));
        if !fine {
            return Err(format!("'{}' has characters TV Doctor does not accept", word));
        }
    }
    Ok(words.iter().map(|w| w.to_string()).collect())
}

/// Percent-decoding for the one query value that needs it, the key token.
fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// The /24 that `net::discover(None)` sweeps, for the home screen to name.
fn local_subnet() -> String {
    let ip = net::local_ipv4();
    let mut parts: Vec<&str> = ip.split('.').collect();
    parts.pop();
    parts.join(".")
}

fn found_json(found: &net::Found) -> String {
    format!(
        "{{\"ip\":\"{}\",\"name\":\"{}\",\"model\":\"{}\",\"cast\":{},\"remote\":{},\"adb\":{}}}",
        escape_json(&found.ip),
        escape_json(&found.friendly_name),
        escape_json(&found.describe()),
        found.cast,
        found.remote,
        found.adb
    )
}

fn start_scan(scan: &ScanShared) {
    {
        let mut state = scan.lock().unwrap();
        if state.running {
            return;
        }
        *state = Scan {
            running: true,
            subnet: local_subnet(),
            lan: true,
            ..Default::default()
        };
    }
    let scan = Arc::clone(scan);
    thread::spawn(move || {
        let results = net::discover(None);
        let lan = !results.is_empty() || net::lan_reachable();
        let mut state = scan.lock().unwrap();
        state.results = results;
        state.lan = lan;
        state.running = false;
        state.finished = true;
    });
}

fn scan_json(scan: &ScanShared) -> String {
    let state = scan.lock().unwrap();
    let results = state
        .results
        .iter()
        .map(found_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"running\":{},\"finished\":{},\"subnet\":\"{}\",\"lan\":{},\"bundle\":{},\"results\":[{}]}}",
        state.running,
        state.finished,
        escape_json(&state.subnet),
        state.lan,
        from_bundle(),
        results
    )
}

fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Strip the ANSI colour the commands emit when they think they have a terminal.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            while let Some(next) = chars.next() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn start_job(shared: &Shared, label: &str, args: &[&str]) {
    {
        let mut job = shared.lock().unwrap();
        if job.running {
            return;
        }
        job.running = true;
        job.label = label.to_string();
        job.lines.push(format!("$ tvdoctor {}", args.join(" ")));
    }

    let shared = Arc::clone(shared);
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    thread::spawn(move || {
        let binary = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "tvdoctor".to_string());

        match Command::new(binary)
            .args(&owned)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Err(error) => {
                let mut job = shared.lock().unwrap();
                job.lines.push(format!("could not start the command: {}", error));
                job.running = false;
            }
            Ok(mut child) => {
                if let Ok(mut job) = shared.lock() {
                    job.pid = Some(child.id());
                    job.stdin = child.stdin.take();
                }
                if let Some(stdout) = child.stdout.take() {
                    let collector = Arc::clone(&shared);
                    let reader = thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            let mut job = collector.lock().unwrap();
                            job.lines.push(strip_ansi(&line));
                        }
                    });
                    let _ = reader.join();
                }
                if let Some(stderr) = child.stderr.take() {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        let mut job = shared.lock().unwrap();
                        job.lines.push(strip_ansi(&line));
                    }
                }
                let _ = child.wait();
                let mut job = shared.lock().unwrap();
                job.lines.push(String::new());
                job.running = false;
                job.pid = None;
                job.stdin = None;
            }
        }
    });
}

/// True when Finder started us through TVDoctor.app. macOS gates local network
/// access per app, so a bundle launch that cannot see the TV usually means the
/// permission has not been granted rather than that the TV is off.
fn from_bundle() -> bool {
    std::env::var("TVDOCTOR_LAUNCHED_FROM").map(|v| v == "bundle").unwrap_or(false)
}

/// The header chips: which TV, and what it will let us do.
///
/// `down_confirmed` says whether a quiet television has been quiet for more
/// than one round. macOS resolves Local Network permission on an app's first
/// connection, so the first probe after launch often fails on a TV that is
/// perfectly awake. Returns the JSON and whether the TV answered.
fn tv_state(down_confirmed: bool) -> (String, bool) {
    let cfg = config::load();
    let ip = cfg.ip.unwrap_or_default();
    if ip.is_empty() {
        return (
            "{\"ip\":\"\",\"awake\":false,\"adb\":false,\"name\":\"\",\
             \"model\":\"\",\"bundle\":false,\"lan\":true,\"down\":false}"
                .to_string(),
            false,
        );
    }
    // This TV answers slowly, 50ms or worse on a wired LAN, and slower again
    // just after booting. Short probes reported it asleep when it was not.
    // One slow answer is not evidence of a sleeping television. This TV drops
    // the odd probe when it is busy, and a flapping header that calls it asleep
    // is worse than a state that is a few seconds old. Costs nothing on the
    // happy path, and this runs off the request path anyway.
    let awake = net::port_open(&ip, net::CAST_PORT, 2500)
        || net::port_open(&ip, net::CAST_PORT, 2500);
    let adb = net::port_open(&ip, ADB_PORT, 2000);
    // On a TV without ADB, the treatments go through TV Nurse instead.
    let nursed = !adb && awake && nurse::Nurse::reach(&ip, 2500).is_some();
    let (name, model) = match net::dial_describe(&ip) {
        Some((name, maker, model)) => (name, format!("{} {}", maker, model).trim().to_string()),
        None => (String::new(), String::new()),
    };
    // Only worth asking when the TV is quiet, and it is what separates a set in
    // standby from a blocked app.
    let lan = if awake { true } else { net::lan_reachable() };

    let json = format!(
        "{{\"ip\":\"{}\",\"awake\":{},\"adb\":{},\"name\":\"{}\",\"model\":\"{}\",\
         \"bundle\":{},\"lan\":{},\"down\":{},\"nurse\":{}}}",
        escape_json(&ip),
        awake,
        adb,
        escape_json(&name),
        escape_json(&model),
        from_bundle(),
        lan,
        down_confirmed && !awake,
        nursed
    );
    (json, awake)
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=') {
            if name == key {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        status,
        content_type,
        body.as_bytes().len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn handle(
    mut stream: TcpStream,
    shared: Shared,
    cache: Cache,
    scan: ScanShared,
    remote_slot: RemoteShared,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buffer = [0u8; 4096];
    let read = match stream.read(&mut buffer) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let Some(first_line) = request.lines().next() else {
        return;
    };
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    if path.starts_with("/api/") && !from_our_page(&request) {
        respond(&mut stream, "403 Forbidden", "text/plain", "only TV Doctor's own page may do that");
        return;
    }

    match path {
        "/" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),

        "/api/state" => {
            let tv = cache.lock().map(|c| c.clone()).unwrap_or_default();
            let job = shared.lock().unwrap();
            let body = format!(
                "{{\"running\":{},\"label\":\"{}\",\"lines\":{},\"tv\":{}}}",
                job.running,
                escape_json(&job.label),
                job.lines.len(),
                tv
            );
            drop(job);
            respond(&mut stream, "200 OK", "application/json", &body);
        }

        "/api/scan" => {
            start_scan(&scan);
            respond(&mut stream, "200 OK", "application/json", &scan_json(&scan));
        }

        "/api/scan/status" => {
            respond(&mut stream, "200 OK", "application/json", &scan_json(&scan));
        }

        // Pick a TV from the home screen: remember it, then probe it once so
        // the page can say whether it answered before entering the platform.
        "/api/connect" => {
            let ip = query_param(target, "ip").unwrap_or_default();
            if ip.parse::<std::net::Ipv4Addr>().is_err() {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"ok\":false,\"error\":\"that is not an IP address\"}",
                );
                return;
            }
            config::remember_ip(&ip);
            let (tv, _) = tv_state(false);
            if let Ok(mut slot) = cache.lock() {
                *slot = tv.clone();
            }
            // When ADB is off, the connecting screen checks for TV Nurse next.
            let link = nurse::link(&ip, 3000);
            let body = format!("{{\"ok\":true,\"tv\":{},\"nurse\":\"{}\"}}", tv, link.word());
            respond(&mut stream, "200 OK", "application/json", &body);
        }

        "/api/log" => {
            let from: usize = query_param(target, "from")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let job = shared.lock().unwrap();
            let slice = if from < job.lines.len() {
                &job.lines[from..]
            } else {
                &[][..]
            };
            let lines = slice
                .iter()
                .map(|l| format!("\"{}\"", escape_json(l)))
                .collect::<Vec<_>>()
                .join(",");
            let body = format!(
                "{{\"from\":{},\"next\":{},\"running\":{},\"lines\":[{}]}}",
                from,
                job.lines.len(),
                job.running,
                lines
            );
            drop(job);
            respond(&mut stream, "200 OK", "application/json", &body);
        }

        "/api/run" => {
            let wanted = query_param(target, "cmd").unwrap_or_default();
            let table = actions();
            match table.get(wanted.as_str()) {
                Some(args) => {
                    start_job(&shared, &wanted, args);
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
                None => respond(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"ok\":false,\"error\":\"unknown action\"}",
                ),
            }
        }

        "/api/remote" => {
            respond(&mut stream, "200 OK", "application/json", &remote_json(&remote_slot));
        }

        // Ask the TV to pair. It answers by putting a code on the screen.
        "/api/remote/pair" => {
            let ip = config::load().ip.unwrap_or_default();
            if ip.is_empty() {
                return json_error(&mut stream, "pick a TV first");
            }
            {
                let mut slot = remote_slot.lock().unwrap();
                let _ = live_session(&mut slot, &ip);
                if let Some(old) = slot.pairing.take() {
                    old.cancel();
                }
            }
            match remote::start_pairing(&ip) {
                Ok(pairing) => {
                    let mut slot = remote_slot.lock().unwrap();
                    slot.pairing = Some(Arc::new(pairing));
                    slot.error.clear();
                    drop(slot);
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
                Err(message) => json_error(&mut stream, &message),
            }
        }

        "/api/remote/code" => {
            let code = query_param(target, "c").unwrap_or_default();
            let ip = config::load().ip.unwrap_or_default();
            let pairing = remote_slot.lock().unwrap().pairing.clone();
            let Some(pairing) = pairing else {
                return json_error(&mut stream, "no pairing in progress. Press Pair again");
            };
            match pairing.finish(&code) {
                Ok(()) => {
                    remote_slot.lock().unwrap().pairing = None;
                    let _ = connect_remote(&remote_slot, &ip);
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
                Err(message) => {
                    // A typo keeps the pairing open; anything else ends it.
                    if !(message.contains("does not match") || message.contains("six characters")) {
                        remote_slot.lock().unwrap().pairing = None;
                    }
                    json_error(&mut stream, &message);
                }
            }
        }

        "/api/remote/cancel" => {
            if let Some(pairing) = remote_slot.lock().unwrap().pairing.take() {
                pairing.cancel();
            }
            respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
        }

        // One key, or a short sequence. Every token is checked against the key
        // table, so nothing else can be sent to the TV from the browser.
        "/api/remote/key" => {
            let token = url_decode(&query_param(target, "k").unwrap_or_default());
            let steps = match remote::parse_step(&token) {
                Ok(steps) if !steps.is_empty() && token.len() <= 24 => steps,
                Ok(_) => return json_error(&mut stream, "no key given"),
                Err(message) => return json_error(&mut stream, &message),
            };
            let ip = config::load().ip.unwrap_or_default();
            // Some TVs, this Hisense among them, ignore the Settings key from a
            // remote. TV Nurse can open Settings directly, so it goes first.
            if token == "settings" {
                if let Some(n) = nurse::Nurse::reach(&ip, 3000) {
                    if n.open_settings("main").is_ok() {
                        return respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                    }
                }
            }
            if ip.is_empty() || !remote::is_paired(&ip) {
                return json_error(&mut stream, "pair the remote first");
            }
            match connect_remote(&remote_slot, &ip).and_then(|session| session.run(&steps)) {
                Ok(()) => respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}"),
                Err(message) => {
                    remote_slot.lock().unwrap().session = None;
                    json_error(&mut stream, &message)
                }
            }
        }

        // TV Nurse, the companion app on the TV: is it there, is it paired.
        "/api/nurse" => {
            let ip = config::load().ip.unwrap_or_default();
            let json = if ip.is_empty() {
                "{\"answering\":false}".to_string()
            } else {
                match nurse::link(&ip, 3000) {
                    nurse::Link::Absent => "{\"answering\":false}".to_string(),
                    nurse::Link::NeedsCode { locked } => {
                        format!("{{\"answering\":true,\"paired\":false,\"locked\":{}}}", locked)
                    }
                    nurse::Link::Paired { expires_in } => {
                        let status = nurse::Nurse::reach(&ip, 3000).and_then(|n| n.status().ok());
                        let (version, usage, energy) = status
                            .map(|s| (s.version(), s.usage_access(), s.energy_exempt()))
                            .unwrap_or_default();
                        format!(
                            "{{\"answering\":true,\"paired\":true,\"version\":\"{}\",\
                             \"usage\":{},\"energy\":{},\"expires\":{}}}",
                            escape_json(&version),
                            usage,
                            energy,
                            expires_in
                        )
                    }
                }
            };
            respond(&mut stream, "200 OK", "application/json", &json);
        }

        "/api/nurse/pair" => {
            let ip = config::load().ip.unwrap_or_default();
            let code: String = query_param(target, "code")
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if code.len() != 6 {
                return json_error(&mut stream, "the code on the TV Nurse screen is six digits");
            }
            match nurse::call(&ip, "GET", "/status", Some(&code), 5000) {
                None => json_error(&mut stream, "TV Nurse is not answering"),
                Some(body) if net::json_field(&body, "paired").as_deref() == Some("true") => {
                    nurse::save_code(&ip, &code);
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
                Some(body) if net::json_field(&body, "lockedOut").as_deref() == Some("true") => {
                    json_error(&mut stream, "Too many wrong codes. Wait a minute and try again")
                }
                Some(_) => json_error(&mut stream, "TV Nurse did not accept that code"),
            }
        }

        "/api/quit" => {
            respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
            println!("  closed from the app window. Goodbye.");
            std::process::exit(0);
        }

        "/api/cancel" => {
            let pid = shared.lock().unwrap().pid;
            match pid {
                Some(pid) => {
                    // Children first: a cancelled command may be sitting on an
                    // adb or openssl call that would otherwise be orphaned.
                    let _ = Command::new("pkill")
                        .args(["-TERM", "-P", &pid.to_string()])
                        .status();
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                    if let Ok(mut job) = shared.lock() {
                        job.lines.push(String::new());
                        job.lines.push("WARN stopped at your request".to_string());
                    }
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
                None => respond(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    "{\"ok\":false,\"error\":\"nothing is running\"}",
                ),
            }
        }

        // A command typed into the Monitor.
        "/api/exec" => {
            let line = url_decode(&query_param(target, "line").unwrap_or_default());
            match parse_typed(&line) {
                Err(message) => json_error(&mut stream, &message),
                Ok(_) if shared.lock().unwrap().running => {
                    json_error(&mut stream, "something is still running. Wait, or press Stop")
                }
                Ok(args) => {
                    let refs: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
                    start_job(&shared, "typed", &refs);
                    respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
                }
            }
        }

        // An answer to the running command's question: Enter, s or q, y/n, or
        // a short typed answer such as a pairing code.
        "/api/input" => {
            let typed = query_param(target, "text").map(|t| url_decode(&t));
            let answer = query_param(target, "a").unwrap_or_default();
            let owned;
            let line = match (typed, answer.as_str()) {
                (Some(text), _) => {
                    let text = text.trim().to_string();
                    if text.len() > 32 || !text.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ') {
                        return json_error(&mut stream, "answers are short: letters, digits and spaces");
                    }
                    owned = text;
                    owned.as_str()
                }
                (None, "" | "done") => "",
                (None, "s" | "skip") => "s",
                (None, "q" | "stop") => "q",
                (None, "y" | "yes") => "y",
                (None, "n" | "no") => "n",
                _ => return json_error(&mut stream, "unknown answer"),
            };
            let mut job = shared.lock().unwrap();
            let sent = match job.stdin.as_mut() {
                Some(stdin) => stdin
                    .write_all(format!("{}\n", line).as_bytes())
                    .and_then(|_| stdin.flush())
                    .is_ok(),
                None => false,
            };
            if sent {
                let shown = match line {
                    "" => "done",
                    _ if line.chars().all(|c| c.is_ascii_digit()) && line.len() >= 4 => "(code sent)",
                    "s" => "skip",
                    "q" => "stop",
                    other => other,
                };
                job.lines.push(format!("  >> {}", shown));
            }
            drop(job);
            if sent {
                respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
            } else {
                json_error(&mut stream, "nothing is waiting for an answer");
            }
        }

        "/api/clear" => {
            let mut job = shared.lock().unwrap();
            job.lines.clear();
            drop(job);
            respond(&mut stream, "200 OK", "application/json", "{\"ok\":true}");
        }

        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

/// Serve the app and open it in the browser.
pub fn serve(opts: &Opts, port: u16, open_browser: bool) -> Result<(), String> {
    let _ = opts;
    let url = format!("http://127.0.0.1:{}", port);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // Already running, most likely a second double click. Show that
            // window rather than complaining.
            println!("  TV Doctor is already running at {}", url);
            if open_browser {
                let _ = Command::new("open").arg(&url).status();
            }
            return Ok(());
        }
        Err(error) => {
            return Err(format!("could not listen on 127.0.0.1:{}: {}", port, error))
        }
    };

    println!();
    println!("  TV Doctor is running at {}", url);
    println!("  Leave this window open while you use it. Press Ctrl+C to stop.");
    println!();

    if open_browser {
        let _ = Command::new("open").arg(&url).status();
    }

    let shared: Shared = Arc::new(Mutex::new(Job::default()));
    // A placeholder until the first probe lands, so the home screen opens at
    // once instead of waiting on a television that may be in standby.
    let remembered = config::load().ip.unwrap_or_default();
    let cache: Cache = Arc::new(Mutex::new(format!(
        "{{\"ip\":\"{}\",\"awake\":false,\"adb\":false,\"name\":\"\",\"model\":\"\",\
         \"bundle\":{},\"lan\":true,\"down\":false,\"pending\":true}}",
        escape_json(&remembered),
        from_bundle()
    )));
    let scan: ScanShared = Arc::new(Mutex::new(Scan::default()));
    let remote_slot: RemoteShared = Arc::new(Mutex::new(RemoteSlot::default()));

    // Keep the TV state fresh off the request path, and only call the set quiet
    // once it has missed twice in a row.
    let refresher = Arc::clone(&cache);
    thread::spawn(move || {
        let mut misses: u32 = 0;
        loop {
            let (latest, awake) = tv_state(misses >= 2);
            misses = if awake { 0 } else { misses.saturating_add(1) };
            if let Ok(mut slot) = refresher.lock() {
                *slot = latest;
            }
            thread::sleep(Duration::from_secs(4));
        }
    });

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                let cache = Arc::clone(&cache);
                let scan = Arc::clone(&scan);
                let remote_slot = Arc::clone(&remote_slot);
                thread::spawn(move || handle(stream, shared, cache, scan, remote_slot));
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_commands_are_checked() {
        assert_eq!(parse_typed("help").unwrap(), vec!["--help"]);
        assert_eq!(parse_typed("tvdoctor nurse free").unwrap(), vec!["nurse", "free"]);
        assert_eq!(
            parse_typed("remote link market://details?id=com.netflix.ninja").unwrap(),
            vec!["remote", "link", "market://details?id=com.netflix.ninja"]
        );
        assert!(parse_typed("").is_err());
        assert!(parse_typed("shell rm -rf /").is_err());
        assert!(parse_typed("app").is_err());
        assert!(parse_typed("curl evil").is_err());
        assert!(parse_typed("nurse open com.x;reboot").is_err());
        assert!(parse_typed("remote press `id`").is_err());
    }

    #[test]
    fn only_our_page_gets_in() {
        let ours = "POST /api/run HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nX-TVDoctor: 1\r\n\r\n";
        let other_site = "POST /api/run HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: https://evil.example\r\n\r\n";
        let rebound = "POST /api/run HTTP/1.1\r\nHost: evil.example:8787\r\nX-TVDoctor: 1\r\n\r\n";
        assert!(from_our_page(ours));
        assert!(!from_our_page(other_site));
        assert!(!from_our_page(rebound));
    }
}
