//! Talking to the TV through adb.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::catalog::KEY_APPS;
use crate::config;
use crate::net::{self, ADB_PORT};
use crate::ui;

/// Where adb lives on this machine.
pub fn adb_path() -> Result<String, String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = format!("{}/adb", dir);
            if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false) {
                return Ok(candidate);
            }
        }
    }
    for candidate in [
        "/opt/homebrew/bin/adb",
        "/usr/local/bin/adb",
        "/usr/bin/adb",
    ] {
        if std::fs::metadata(candidate).map(|m| m.is_file()).unwrap_or(false) {
            return Ok(candidate.to_string());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let candidate = format!("{}/Library/Android/sdk/platform-tools/adb", home);
        if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false) {
            return Ok(candidate);
        }
    }
    Err("adb not found. Install it with:  brew install --cask android-platform-tools"
        .to_string())
}

/// Run a command, optionally giving up after a while. stdout and stderr are
/// drained by their own threads so a chatty command cannot fill a pipe and
/// deadlock.
pub fn run(program: &str, args: &[String], timeout: Option<Duration>) -> (i32, String, String) {
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return (-1, String::new(), err.to_string()),
    };

    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = out_pipe.read_to_string(&mut buf);
        buf
    });
    let err_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = err_pipe.read_to_string(&mut buf);
        buf
    });

    let deadline = timeout.map(|t| Instant::now() + t);
    let mut code = -1;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                if let Some(deadline) = deadline {
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    let stdout = out_reader.join().unwrap_or_default();
    let mut stderr = err_reader.join().unwrap_or_default();
    if timed_out {
        stderr.push_str("\ntvdoctor: gave up waiting for adb");
    }
    (code, stdout, stderr)
}

/// serial -> state, from `adb devices`.
pub fn devices() -> Vec<(String, String)> {
    let Ok(adb) = adb_path() else {
        return Vec::new();
    };
    let (_, out, _) = run(&adb, &["devices".to_string()], Some(Duration::from_secs(20)));
    out.lines()
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(serial), Some(state)) => {
                    Some((serial.to_string(), state.to_string()))
                }
                _ => None,
            }
        })
        .collect()
}

fn device_state(serial: &str) -> Option<String> {
    devices()
        .into_iter()
        .find(|(s, _)| s == serial)
        .map(|(_, state)| state)
}

/// Connect adb to the TV. Returns the serial on success.
pub fn connect(ip: &str) -> Result<String, String> {
    let adb = adb_path()?;
    let serial = format!("{}:{}", ip, ADB_PORT);
    let mut last = String::new();
    for attempt in 0..3 {
        let (_, out, err) = run(
            &adb,
            &["connect".to_string(), serial.clone()],
            Some(Duration::from_secs(30)),
        );
        last = format!("{}{}", out.trim(), err.trim());
        if last.contains("connected to") {
            thread::sleep(Duration::from_millis(1000));
            match device_state(&serial).as_deref() {
                Some("device") => return Ok(serial),
                Some("unauthorized") => {
                    return Err("unauthorized: accept the 'Allow debugging from this \
                                computer?' prompt on the TV screen, then run \
                                tvdoctor connect again"
                        .to_string())
                }
                _ => {}
            }
        }
        if attempt < 2 {
            thread::sleep(Duration::from_millis(1500));
        }
    }
    Err(if last.is_empty() {
        format!("could not connect to {}", ip)
    } else {
        last
    })
}

/// A connected television.
pub struct Tv {
    pub serial: String,
    pub adb: String,
    pub dry_run: bool,
    packages: std::cell::RefCell<Option<Vec<String>>>,
}

impl Tv {
    pub fn new(serial: String, dry_run: bool) -> Result<Self, String> {
        Ok(Tv {
            serial,
            adb: adb_path()?,
            dry_run,
            packages: std::cell::RefCell::new(None),
        })
    }

    fn call(&self, args: &[String], timeout: Option<Duration>) -> (i32, String, String) {
        let mut full = vec!["-s".to_string(), self.serial.clone()];
        full.extend_from_slice(args);
        run(&self.adb, &full, timeout)
    }

    /// Run a shell command on the TV and return what it printed.
    pub fn sh(&self, command: &str) -> String {
        self.sh_timeout(command, Duration::from_secs(90))
    }

    pub fn sh_timeout(&self, command: &str, timeout: Duration) -> String {
        let (_, out, err) =
            self.call(&["shell".to_string(), command.to_string()], Some(timeout));
        format!("{}{}", out, err).trim().to_string()
    }

    /// A shell command that changes something. Honours --dry-run.
    pub fn sh_write(&self, command: &str, label: &str, timeout: Duration) -> String {
        if self.dry_run {
            ui::dry(label);
            return String::new();
        }
        self.sh_timeout(command, timeout)
    }

    pub fn getprop(&self, name: &str) -> String {
        self.sh(&format!("getprop {}", name))
    }

    pub fn reboot(&self) -> Result<(), String> {
        if self.dry_run {
            ui::dry("adb reboot");
            return Ok(());
        }
        let (_, _, err) = self.call(&["reboot".to_string()], Some(Duration::from_secs(30)));
        if err.contains("error:") {
            return Err(err.trim().to_string());
        }
        Ok(())
    }

    /// Every installed package, cached for the life of the run.
    pub fn packages(&self) -> Vec<String> {
        if let Some(cached) = self.packages.borrow().clone() {
            return cached;
        }
        let list = self.package_list("");
        *self.packages.borrow_mut() = Some(list.clone());
        list
    }

    /// `pm list packages <flag>`: "-3" user installed, "-d" disabled, "-s" system.
    pub fn package_list(&self, flag: &str) -> Vec<String> {
        let raw = self.sh(&format!("pm list packages {}", flag));
        let mut out: Vec<String> = raw
            .lines()
            .filter_map(|line| line.strip_prefix("package:"))
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn version_of(&self, pkg: &str) -> String {
        let out = self.sh(&format!("dumpsys package {} | grep -m1 versionName", pkg));
        out.lines()
            .find_map(|line| line.trim().strip_prefix("versionName=").map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Value after the first '=' of a dumpsys line, e.g. lastUpdateTime.
    pub fn package_field(&self, pkg: &str, field: &str) -> Option<String> {
        let out = self.sh(&format!("dumpsys package {} | grep -m1 {}", pkg, field));
        out.split_once('=').map(|(_, v)| v.trim().to_string())
    }

    /// Which package id each of the four apps actually has on this TV.
    pub fn resolve_key_apps(&self) -> Vec<(String, Option<String>)> {
        let installed = self.packages();
        KEY_APPS
            .iter()
            .map(|(label, fragments, known)| {
                let found = known
                    .iter()
                    .find(|k| installed.iter().any(|p| p == *k))
                    .map(|k| k.to_string())
                    .or_else(|| {
                        installed
                            .iter()
                            .find(|p| {
                                let low = p.to_lowercase();
                                fragments.iter().any(|f| low.contains(f))
                            })
                            .cloned()
                    });
                (label.to_string(), found)
            })
            .collect()
    }
}

/// Work out which TV to act on, connecting if it is not connected yet.
///
/// With `require` false, a TV that cannot be reached is reported as None
/// instead of ending the run.
pub fn get_tv(ip_override: Option<&str>, dry_run: bool, require: bool) -> Result<Option<Tv>, String> {
    let cfg = config::load();
    let ip = match ip_override.map(str::to_string).or_else(|| cfg.ip.clone()) {
        Some(ip) if !ip.is_empty() => ip,
        _ => {
            let found = net::discover(None);
            let tvs: Vec<_> = found
                .into_iter()
                .filter(|f| f.cast || f.remote)
                .collect();
            if tvs.is_empty() {
                return Err("no Android TV found on this network. Is the TV on and awake?"
                    .to_string());
            }
            if tvs.len() > 1 {
                println!("  more than one candidate found, using the first:");
                for f in &tvs {
                    println!("    {}  {}", f.ip, f.describe());
                }
            }
            tvs[0].ip.clone()
        }
    };

    let serial = format!("{}:{}", ip, ADB_PORT);
    if device_state(&serial).as_deref() != Some("device") {
        if !net::port_open(&ip, ADB_PORT, 1200) {
            if let Some((name, maker, model)) = net::dial_describe(&ip) {
                println!(
                    "  found {} {} ({}) at {}",
                    maker.trim(),
                    model.trim(),
                    name.trim(),
                    ip
                );
            }
            if !require {
                return Ok(None);
            }
            println!();
            println!("{}", crate::catalog::ENABLE_ADB_HELP);
            println!();
            return Err(format!("cannot reach ADB on {}:{}", ip, ADB_PORT));
        }
        match connect(&ip) {
            Ok(_) => {}
            Err(message) => {
                if !require {
                    return Ok(None);
                }
                return Err(message);
            }
        }
    }

    config::remember_ip(&ip);
    Ok(Some(Tv::new(serial, dry_run)?))
}
