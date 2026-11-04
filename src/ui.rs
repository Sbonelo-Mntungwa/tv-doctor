//! Terminal output helpers. Colour is dropped when stdout is not a terminal.

use std::io::{self, IsTerminal, Write};

pub fn tty() -> bool {
    io::stdout().is_terminal()
}

pub fn c(code: &str, text: &str) -> String {
    if tty() {
        format!("\x1b[{}m{}\x1b[0m", code, text)
    } else {
        text.to_string()
    }
}

pub fn hdr(title: &str) {
    println!();
    println!("{}", c("1;36", title));
    println!("{}", c("36", &"-".repeat(title.chars().count())));
}

pub fn kv(key: &str, value: &str) {
    println!("  {:<27} {}", format!("{}:", key), value);
}

pub fn ok(text: &str) {
    println!("  {}   {}", c("32", "OK"), text);
}

pub fn warn(text: &str) {
    println!("  {} {}", c("33", "WARN"), text);
}

pub fn bad(text: &str) {
    println!("  {} {}", c("31", "FAIL"), text);
}

pub fn info(text: &str) {
    println!("  {}   {}", c("34", ".."), text);
}

pub fn dry(text: &str) {
    println!("  {} {}", c("35", "dry-run"), text);
}

/// Bytes as something a person can read.
pub fn human(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value.abs() < 1024.0 || unit == "TB" {
            return format!("{:.1} {}", value, unit);
        }
        value /= 1024.0;
    }
    unreachable!()
}

/// Ask a question. An empty answer or a closed stdin counts as "no", so the
/// tool never loops forever when it is run without a terminal.
pub fn prompt(message: &str) -> String {
    print!("{}", message);
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => "q".to_string(),
        Ok(_) => line.trim().to_string(),
    }
}

pub fn confirm(message: &str) -> bool {
    let answer = prompt(&format!("  {} [y/N] ", message));
    matches!(answer.to_lowercase().as_str(), "y" | "yes")
}
