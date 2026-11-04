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
use crate::config;
use crate::net::{self, ADB_PORT};

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
        ("apps", vec!["apps"]),
        ("full", vec!["full"]),
        ("reboot", vec!["reboot", "--yes"]),
        ("restore", vec!["restore", "--yes"]),
        ("launch-youtube", vec!["launch", "youtube"]),
        ("launch-netflix", vec!["launch", "netflix"]),
        ("launch-spotify", vec!["launch", "spotify"]),
        ("launch-dstv", vec!["launch", "dstv"]),
        ("stop-youtube", vec!["stop", "youtube"]),
        ("stop-netflix", vec!["stop", "netflix"]),
        ("stop-spotify", vec!["stop", "spotify"]),
        ("force-stop", vec!["force-stop"]),
        ("force-stop-dstv", vec!["force-stop", "dstv"]),
    ])
}

#[derive(Default)]
struct Job {
    lines: Vec<String>,
    running: bool,
    label: String,
    /// The child's pid while it runs, so the Stop button can end it.
    pid: Option<u32>,
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
    let (name, model) = match net::dial_describe(&ip) {
        Some((name, maker, model)) => (name, format!("{} {}", maker, model).trim().to_string()),
        None => (String::new(), String::new()),
    };
    // Only worth asking when the TV is quiet, and it is what separates a set in
    // standby from a blocked app.
    let lan = if awake { true } else { net::lan_reachable() };

    let json = format!(
        "{{\"ip\":\"{}\",\"awake\":{},\"adb\":{},\"name\":\"{}\",\"model\":\"{}\",\
         \"bundle\":{},\"lan\":{},\"down\":{}}}",
        escape_json(&ip),
        awake,
        adb,
        escape_json(&name),
        escape_json(&model),
        from_bundle(),
        lan,
        down_confirmed && !awake
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

fn handle(mut stream: TcpStream, shared: Shared, cache: Cache, scan: ScanShared) {
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
            let body = format!("{{\"ok\":true,\"tv\":{}}}", tv);
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
                thread::spawn(move || handle(stream, shared, cache, scan));
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
