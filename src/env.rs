//! Per-user settings from a `.env` file, so nothing about a particular TV or
//! network has to live in the source.
//!
//! Looked for, in order, and the first value found for a key wins:
//!
//! 1. the real process environment
//! 2. `.env` in the current directory
//! 3. `.env` in the project folder, found from the binary's location, which is
//!    how `TV Doctor.app` (run from Finder, with no useful working directory)
//!    picks it up
//! 4. `~/.config/tvdoctor/.env`
//!
//! Recognised keys (see `.env.example`):
//!
//! - `TVDOCTOR_IP`     the TV's address, used when `--ip` is not given
//! - `TVDOCTOR_KEEP`   extra packages to protect, comma separated
//! - `TVDOCTOR_SUBNET` the subnet `discover` sweeps, e.g. `192.168.1`
//! - `TVDOCTOR_PORT`   the port the app window listens on (default 8787)

use std::fs;
use std::path::PathBuf;

fn candidates() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from(".env")];
    if let Ok(exe) = std::env::current_exe() {
        // Walk up from target/release/tvdoctor or
        // dist/TV Doctor.app/Contents/MacOS/tvdoctor to the project folder.
        if let Some(project) = exe
            .ancestors()
            .skip(1)
            .take(5)
            .find(|dir| dir.join("Cargo.toml").is_file())
        {
            out.push(project.join(".env"));
        }
    }
    out.push(crate::config::config_dir().join(".env"));
    out
}

fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value = &value[1..value.len() - 1];
    } else if let Some(hash) = value.find(" #") {
        value = value[..hash].trim_end();
    }
    Some((key.to_string(), value.to_string()))
}

/// Read every `.env` candidate into the process environment, never replacing
/// a variable that is already set. Call once, first thing in `main`, before
/// any threads start.
pub fn load() {
    for path in candidates() {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (key, value) in text.lines().filter_map(parse_line) {
            if std::env::var_os(&key).is_none() {
                std::env::set_var(key, value);
            }
        }
    }
}

/// A non-empty environment value.
pub fn get(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_lines() {
        assert_eq!(parse_line("# c"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(
            parse_line("TVDOCTOR_IP=192.168.1.20"),
            Some(("TVDOCTOR_IP".into(), "192.168.1.20".into()))
        );
        assert_eq!(
            parse_line("export TVDOCTOR_KEEP=\"a, b\""),
            Some(("TVDOCTOR_KEEP".into(), "a, b".into()))
        );
        assert_eq!(
            parse_line("TVDOCTOR_PORT=9000  # window"),
            Some(("TVDOCTOR_PORT".into(), "9000".into()))
        );
    }
}
