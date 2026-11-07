//! Remembered TV address, the extra keep list, and the undo journal.
//!
//! Small enough that hand rolled JSON beats a dependency.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Set from --dry-run. Keeps the journal from recording changes that were only
/// printed.
static DRY_RUN: AtomicBool = AtomicBool::new(false);

pub fn dry_run() -> bool {
    DRY_RUN.load(Ordering::Relaxed)
}

pub fn set_dry_run(value: bool) {
    DRY_RUN.store(value, Ordering::Relaxed);
}

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("tvdoctor")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn journal_path() -> PathBuf {
    config_dir().join("journal.jsonl")
}

#[derive(Default, Clone)]
pub struct Config {
    pub ip: Option<String>,
    pub keep: Vec<String>,
}

/// Pull the string value of "key" out of flat JSON.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let colon = rest.find(':')? + 1;
    let rest = &rest[colon..];
    let open = rest.find('"')? + 1;
    let rest = &rest[open..];
    let close = rest.find('"')?;
    Some(rest[..close].to_string())
}

/// Pull the strings out of the array at "key".
fn json_string_array(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{}\"", key);
    let Some(start) = text.find(&needle) else {
        return Vec::new();
    };
    let rest = &text[start + needle.len()..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };
    let body = &rest[open + 1..open + close];
    let mut out = Vec::new();
    let mut chars = body.chars().peekable();
    let mut current: Option<String> = None;
    while let Some(ch) = chars.next() {
        match (ch, current.is_some()) {
            ('"', false) => current = Some(String::new()),
            ('"', true) => {
                out.push(current.take().unwrap());
            }
            (_, true) => current.as_mut().unwrap().push(ch),
            _ => {}
        }
    }
    let _ = chars;
    out
}

pub fn load() -> Config {
    let Ok(text) = fs::read_to_string(config_path()) else {
        return Config::default();
    };
    Config {
        ip: json_string(&text, "ip").filter(|ip| !ip.is_empty()),
        keep: json_string_array(&text, "keep"),
    }
}

pub fn save(cfg: &Config) {
    let _ = fs::create_dir_all(config_dir());
    let keep = cfg
        .keep
        .iter()
        .map(|k| format!("\"{}\"", k))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "{{\n  \"ip\": \"{}\",\n  \"keep\": [{}]\n}}\n",
        cfg.ip.clone().unwrap_or_default(),
        keep
    );
    let _ = fs::write(config_path(), body);
}

pub fn remember_ip(ip: &str) {
    let mut cfg = load();
    cfg.ip = Some(ip.to_string());
    save(&cfg);
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

pub struct Entry {
    pub ts: String,
    pub action: String,
    pub target: String,
}

fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil_from_epoch(secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        y, m, d, hh, mm, ss
    )
}

/// Days-from-epoch to a civil date, after Howard Hinnant's algorithm.
pub fn civil_from_epoch(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

pub fn journal(action: &str, target: &str) {
    if dry_run() {
        return;
    }
    let _ = fs::create_dir_all(config_dir());
    let line = format!(
        "{{\"ts\": \"{}\", \"action\": \"{}\", \"target\": \"{}\"}}\n",
        now_utc(),
        action,
        target
    );
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn read_journal() -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(journal_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            Some(Entry {
                ts: json_string(line, "ts")?,
                action: json_string(line, "action")?,
                target: json_string(line, "target").unwrap_or_default(),
            })
        })
        .collect()
}

/// Move the journal aside once it has been applied, so restore is not repeated.
pub fn archive_journal() {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let target = journal_path().with_extension(format!("jsonl.restored-{}", secs));
    let _ = fs::rename(journal_path(), target);
}
