//! TV Nurse: the companion app on the TV, and every treatment done through it.
//!
//! On a TV that will not allow ADB, the treatment commands (report, clean,
//! optimise, tune, bloat, restore, apps, update, full, reboot) land here
//! instead of in cmds.rs. An ordinary Android app can read a lot but change
//! little, so where only Settings can make a change, TV Nurse opens the right
//! page, the person presses the button with the remote, and TV Nurse checks
//! that it took. Those steps ask with a line starting "??", which the app
//! window turns into Continue, Skip and Stop buttons.

use std::time::Duration;

use crate::catalog::{is_protected, KEY_APPS, KNOWN_BLOAT};
use crate::cmds::Opts;
use crate::config;
use crate::net;
use crate::remote;
use crate::ui::{self, bad, hdr, human, info, kv, ok, warn};

pub const PORT: u16 = 8790;

/// What to say when the hourly code has changed.
pub const EXPIRED: &str = "TV Nurse's code has changed (it changes every hour). \
     Open TV Nurse on the TV and enter the new code: tvdoctor nurse pair CODE";
/// TV Nurse's own package, which is never listed or touched.
pub const PACKAGE: &str = "com.tvdoctor.helper";

// ---------------------------------------------------------------------------
// the pairing code
// ---------------------------------------------------------------------------
fn code_path() -> std::path::PathBuf {
    config::config_dir().join("nurse-codes")
}

/// The pairing code saved for the TV at `ip`.
pub fn saved_code(ip: &str) -> Option<String> {
    std::fs::read_to_string(code_path())
        .ok()?
        .lines()
        .find_map(|line| {
            let (addr, code) = line.split_once(' ')?;
            (addr == ip).then(|| code.trim().to_string())
        })
}

pub fn save_code(ip: &str, code: &str) {
    let mut lines: Vec<String> = std::fs::read_to_string(code_path())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with(&format!("{} ", ip)) && !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    lines.push(format!("{} {}", ip, code));
    let _ = std::fs::create_dir_all(config::config_dir());
    let _ = std::fs::write(code_path(), lines.join("\n") + "\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(code_path(), std::fs::Permissions::from_mode(0o600));
    }
}

/// Drop the saved code for `ip`. Done as soon as TV Nurse turns it down, so an
/// expired code is not sent again and again: TV Nurse locks everyone out for a
/// minute after five wrong codes.
pub fn forget_code(ip: &str) {
    let lines: Vec<String> = std::fs::read_to_string(code_path())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with(&format!("{} ", ip)) && !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    let _ = std::fs::write(code_path(), lines.join("\n") + if lines.is_empty() { "" } else { "\n" });
}

/// Where TV Doctor stands with TV Nurse on one TV.
pub enum Link {
    /// Nothing answers on the TV Nurse port.
    Absent,
    /// TV Nurse answers but has no valid code from us: never paired, or the
    /// hourly code has changed. `locked` when too many wrong codes were tried.
    NeedsCode { locked: bool },
    /// Paired, with this many seconds left on the code.
    Paired { expires_in: u64 },
}

impl Link {
    pub fn word(&self) -> &'static str {
        match self {
            Link::Absent => "absent",
            Link::NeedsCode { .. } => "code",
            Link::Paired { .. } => "paired",
        }
    }
}

/// Ask TV Nurse at `ip` where things stand, forgetting a code it turns down.
pub fn link(ip: &str, timeout_ms: u64) -> Link {
    let code = saved_code(ip);
    let Some(body) = call(ip, "GET", "/status", code.as_deref(), timeout_ms) else {
        return Link::Absent;
    };
    if flag(&body, "paired") {
        return Link::Paired {
            expires_in: number(&body, "codeExpiresInS").unwrap_or(0),
        };
    }
    if code.is_some() {
        forget_code(ip);
    }
    Link::NeedsCode {
        locked: flag(&body, "lockedOut"),
    }
}

/// One request to TV Nurse. The reply body, or None when it is not answering.
pub fn call(ip: &str, method: &str, path: &str, code: Option<&str>, timeout_ms: u64) -> Option<String> {
    let headers: Vec<(&str, &str)> = code.map(|c| vec![("X-Nurse-Code", c)]).unwrap_or_default();
    let reply = net::http_request_with(method, ip, PORT, path, &headers, timeout_ms)?;
    let (_, body) = reply.split_once("\r\n\r\n")?;
    Some(body.to_string())
}

// ---------------------------------------------------------------------------
// reading its answers
// ---------------------------------------------------------------------------
/// `"key":{...}` out of a JSON object. TV Nurse's nested objects are flat.
fn json_object<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let start = text.find(&format!("\"{}\":{{", key))?;
    let rest = &text[start..];
    let end = rest.find('}')?;
    Some(&rest[..=end])
}

fn number(text: &str, key: &str) -> Option<u64> {
    net::json_field(text, key)?.parse().ok()
}

fn flag(text: &str, key: &str) -> bool {
    net::json_field(text, key).as_deref() == Some("true")
}

/// The answer to GET /status.
pub struct Status {
    body: String,
}

impl Status {
    pub fn field(&self, key: &str) -> String {
        net::json_field(&self.body, key).unwrap_or_default()
    }
    pub fn version(&self) -> String {
        self.field("version")
    }
    pub fn paired(&self) -> bool {
        flag(&self.body, "paired")
    }
    pub fn usage_access(&self) -> bool {
        flag(&self.body, "usageAccess")
    }
    pub fn energy_exempt(&self) -> bool {
        flag(&self.body, "energyExempt")
    }
    pub fn storage(&self) -> Option<(u64, u64)> {
        let s = json_object(&self.body, "storage")?;
        Some((number(s, "total")?, number(s, "free")?))
    }
    pub fn memory(&self) -> Option<(u64, u64)> {
        let m = json_object(&self.body, "memory")?;
        Some((number(m, "total")?, number(m, "available")?))
    }
    pub fn memory_low(&self) -> bool {
        json_object(&self.body, "memory").map(|m| flag(m, "low")).unwrap_or(false)
    }
    /// Window, transition and animator scales. None before TV Nurse 0.3.
    pub fn animations(&self) -> Option<[f32; 3]> {
        let a = json_object(&self.body, "animations")?;
        let get = |k: &str| net::json_field(a, k)?.parse::<f32>().ok();
        Some([get("window")?, get("transition")?, get("animator")?])
    }
}

/// One app from GET /report.
#[derive(Clone, Default)]
pub struct App {
    pub package: String,
    pub label: String,
    pub version: String,
    pub system: bool,
    pub enabled: bool,
    pub launchable: bool,
    pub updated: Option<u64>,
    pub cache: Option<u64>,
    pub data: Option<u64>,
    pub size: Option<u64>,
    pub last_used: Option<u64>,
}

/// The apps array of /report: flat objects, one per app.
pub fn parse_apps(body: &str) -> Vec<App> {
    let Some(start) = body.find("\"apps\":[") else {
        return Vec::new();
    };
    body[start + 8..]
        .split('{')
        .skip(1)
        .map(|chunk| {
            let object = chunk.split('}').next().unwrap_or("");
            let get = |k: &str| net::json_field(object, k).unwrap_or_default();
            let num = |k: &str| net::json_field(object, k).and_then(|v| v.parse::<u64>().ok());
            let size = match (num("appBytes"), num("dataBytes"), num("cacheBytes")) {
                (Some(a), Some(d), Some(c)) => Some(a + d + c),
                _ => None,
            };
            App {
                package: get("package"),
                label: get("label"),
                version: get("version"),
                system: get("system") == "true",
                enabled: get("enabled") == "true",
                launchable: get("launchable") == "true",
                updated: num("updated"),
                cache: num("cacheBytes"),
                data: num("dataBytes"),
                size,
                last_used: num("lastUsed"),
            }
        })
        .filter(|a| !a.package.is_empty() && a.package != PACKAGE)
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn date(ms: u64) -> String {
    if ms == 0 {
        return "never".to_string();
    }
    let (y, m, d, _, _, _) = config::civil_from_epoch(ms / 1000);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// ---------------------------------------------------------------------------
// the connection
// ---------------------------------------------------------------------------
/// TV Nurse on a TV, reachable and paired.
pub struct Nurse {
    pub ip: String,
    code: String,
}

impl Nurse {
    /// TV Nurse at `ip`, when it answers and accepts the saved code.
    pub fn reach(ip: &str, timeout_ms: u64) -> Option<Nurse> {
        let code = saved_code(ip)?;
        let body = call(ip, "GET", "/status", Some(&code), timeout_ms)?;
        if !flag(&body, "paired") {
            forget_code(ip);
            return None;
        }
        Some(Nurse {
            ip: ip.to_string(),
            code,
        })
    }

    fn request(&self, method: &str, path: &str, timeout_ms: u64) -> Result<String, String> {
        let reply = net::http_request_with(
            method,
            &self.ip,
            PORT,
            path,
            &[("X-Nurse-Code", self.code.as_str())],
            timeout_ms,
        )
        .ok_or(format!("TV Nurse stopped answering on {}:{}", self.ip, PORT))?;
        let body = reply.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        // The hourly code ran out partway through.
        if reply.starts_with("HTTP/1.1 401") {
            forget_code(&self.ip);
            return Err(EXPIRED.to_string());
        }
        if body.contains("\"ok\":false") {
            return Err(net::json_field(&body, "error").unwrap_or_else(|| "TV Nurse refused".to_string()));
        }
        Ok(body)
    }

    pub fn status(&self) -> Result<Status, String> {
        Ok(Status {
            body: self.request("GET", "/status", 8000)?,
        })
    }

    /// Whether usage access is granted, and every app.
    pub fn apps(&self) -> Result<(bool, Vec<App>), String> {
        let body = self.request("GET", "/report", 40000)?;
        Ok((flag(&body, "usageAccess"), parse_apps(&body)))
    }

    pub fn app(&self, pkg: &str) -> Result<Option<App>, String> {
        Ok(self.apps()?.1.into_iter().find(|a| a.package == pkg))
    }

    /// Close background apps. (free before, free after).
    pub fn free_memory(&self) -> Result<(u64, u64), String> {
        let body = self.request("POST", "/free", 40000)?;
        Ok((number(&body, "before").unwrap_or(0), number(&body, "after").unwrap_or(0)))
    }

    pub fn open_app_page(&self, pkg: &str) -> Result<(), String> {
        self.request("POST", &format!("/open?package={}", pkg), 10000).map(|_| ())
    }

    pub fn launch(&self, pkg: &str) -> Result<(), String> {
        self.request("POST", &format!("/launch?package={}", pkg), 10000).map(|_| ())
    }

    pub fn open_store(&self, pkg: &str) -> Result<(), String> {
        self.request("POST", &format!("/store?package={}", pkg), 10000).map(|_| ())
    }

    pub fn open_settings(&self, page: &str) -> Result<(), String> {
        self.request("POST", &format!("/settings?page={}", page), 10000).map(|_| ())
    }
}

/// Press Home with the remote, when this Mac is paired as one. Leaves the TV
/// tidy after a guided step without making the remote a requirement.
fn press_home(ip: &str) {
    if remote::is_paired(ip) {
        if let Ok(session) = remote::Remote::connect(ip) {
            let _ = session.press(3, remote::Press::Short);
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

// ---------------------------------------------------------------------------
// asking the person at the remote
// ---------------------------------------------------------------------------
#[derive(PartialEq)]
enum Answer {
    Done,
    Skip,
    Stop,
}

/// One step for the person at the TV. The "??" is what the app window looks
/// for to show its Continue, Skip and Stop buttons.
fn ask(what: &str) -> Answer {
    let answer = ui::prompt(&format!("  ?? {}  [Enter: done, s: skip, q: stop] ", what));
    match answer.trim().to_lowercase().as_str() {
        "q" | "quit" | "stop" => Answer::Stop,
        "s" | "skip" => Answer::Skip,
        _ => Answer::Done,
    }
}

// ---------------------------------------------------------------------------
// the commands
// ---------------------------------------------------------------------------
/// Run a treatment command through TV Nurse. Called when ADB is not there.
pub fn run(command: &str, nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    info(&format!("no ADB on this TV, so this goes through TV Nurse at {}", nurse.ip));
    match command {
        "report" => report(nurse),
        "clean" => clean(nurse, opts),
        "optimize" | "optimise" => optimize(nurse, opts),
        "tune" => tune(nurse, opts),
        "bloat" => bloat(nurse, opts),
        "restore" => restore(nurse, opts),
        "update" => update(nurse, opts),
        "apps" => apps(nurse, opts),
        "full" => full(nurse, opts),
        "reboot" => reboot(nurse, opts),
        "shell" => Err("a raw shell needs ADB. TV Nurse is an ordinary app and cannot run one".to_string()),
        other => Err(format!("TV Nurse has no '{}'", other)),
    }
}

fn print_vitals(status: &Status) {
    kv("Device", &format!("{} {}", status.field("maker"), status.field("model")));
    kv("Android", &format!("{} (API {})", status.field("android"), status.field("sdk")));
    if let Some((total, free)) = status.storage() {
        let pct = if total > 0 { free * 100 / total } else { 0 };
        kv("Storage free", &format!("{} of {} ({}%)", human(free), human(total), pct));
        if pct < 15 {
            warn("storage is nearly full, which slows Android TV down a lot");
        }
    }
    if let Some((total, available)) = status.memory() {
        let pct = if total > 0 { available * 100 / total } else { 0 };
        kv("Memory free", &format!("{} of {} ({}%)", human(available), human(total), pct));
        if pct < 35 || status.memory_low() {
            warn("memory is the bottleneck. tvdoctor clean closes background apps");
        }
    }
    if let Some(scales) = status.animations() {
        let off = scales.iter().all(|s| *s == 0.0);
        kv(
            "Menu animations",
            &if off {
                "off".to_string()
            } else {
                format!("window {}x, transition {}x, animator {}x", scales[0], scales[1], scales[2])
            },
        );
    }
    if let Some(uptime) = status.field("uptimeMs").parse::<u64>().ok() {
        let hours = uptime / 3_600_000;
        kv("Up for", &format!("{}d {}h", hours / 24, hours % 24));
    }
}

/// The apps TV Doctor calls the four main ones, as installed on this TV.
fn key_apps(apps: &[App]) -> Vec<(&'static str, Option<App>)> {
    KEY_APPS
        .iter()
        .map(|(label, fragments, packages)| {
            let found = packages
                .iter()
                .find_map(|p| apps.iter().find(|a| a.package == *p))
                .or_else(|| {
                    apps.iter().find(|a| {
                        let low = a.package.to_lowercase();
                        fragments.iter().any(|f| low.contains(f))
                    })
                })
                .cloned();
            (*label, found)
        })
        .collect()
}

fn key_packages(apps: &[App]) -> Vec<String> {
    key_apps(apps)
        .into_iter()
        .filter_map(|(_, app)| app.map(|a| a.package))
        .collect()
}

/// Launchable apps nobody has opened in six months.
fn idle_apps(apps: &[App]) -> Vec<&App> {
    let cutoff = now_ms().saturating_sub(180 * 24 * 3_600_000);
    let keep = config::load().keep;
    let key = key_packages(apps);
    let mut idle: Vec<&App> = apps
        .iter()
        .filter(|a| a.enabled && a.launchable && a.last_used.unwrap_or(0) < cutoff)
        .filter(|a| !is_protected(&a.package, &keep) && !key.contains(&a.package))
        .collect();
    idle.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    idle
}

pub fn report(nurse: &Nurse) -> Result<(), String> {
    let status = nurse.status()?;
    hdr("Vital signs (TV Nurse)");
    print_vitals(&status);

    let (usage, apps) = nurse.apps()?;
    kv("Apps installed", &apps.len().to_string());
    kv("Switched off", &apps.iter().filter(|a| !a.enabled).count().to_string());

    hdr("Your four apps");
    for (label, app) in key_apps(&apps) {
        match app {
            None => warn(&format!("{:<8} not installed", label)),
            Some(a) => {
                let state = if a.enabled { "" } else { ", switched off" };
                let cache = a.cache.map(|c| format!(", cache {}", human(c))).unwrap_or_default();
                ok(&format!("{:<8} v{}{}{}", label, a.version, cache, state));
            }
        }
    }

    if !usage {
        println!();
        warn("allow Usage access on the TV Nurse screen to see sizes, caches and last use");
        println!();
        return Ok(());
    }

    let mut by_cache: Vec<&App> = apps.iter().filter(|a| a.cache.unwrap_or(0) > 0).collect();
    by_cache.sort_by(|a, b| b.cache.cmp(&a.cache));
    hdr("Biggest caches");
    for app in by_cache.iter().take(10) {
        println!("  {:>10}  {:<28} {}", human(app.cache.unwrap_or(0)), app.label, ui::c("2", &app.package));
    }
    let total: u64 = by_cache.iter().filter_map(|a| a.cache).sum();
    kv("All caches together", &human(total));

    let mut by_size: Vec<&App> = apps.iter().filter(|a| a.size.is_some()).collect();
    by_size.sort_by(|a, b| b.size.cmp(&a.size));
    hdr("Biggest apps");
    for app in by_size.iter().take(8) {
        println!("  {:>10}  {:<28} {}", human(app.size.unwrap_or(0)), app.label, ui::c("2", &app.package));
    }

    hdr("Not opened in 6 months");
    let idle = idle_apps(&apps);
    if idle.is_empty() {
        ok("every app with a home-screen tile has been used recently");
    }
    for app in &idle {
        let size = app.size.map(human).unwrap_or_default();
        let kind = if app.system { "built-in " } else { "" };
        println!("  {:<30} {:>10}  {}{}", app.label, size, kind, ui::c("2", &app.package));
    }
    println!();
    Ok(())
}

/// Close background apps and say how much came back.
fn free_memory(nurse: &Nurse) -> Result<(), String> {
    info("closing apps running in the background");
    if config::dry_run() {
        ui::dry("would ask TV Nurse to close background apps");
        return Ok(());
    }
    let (before, after) = nurse.free_memory()?;
    kv("Free before", &human(before));
    kv("Free after", &human(after));
    if after > before {
        ok(&format!("freed {}. Apps start again when you open them", human(after - before)));
    } else {
        info("nothing much was running in the background");
    }
    Ok(())
}

pub fn clean(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    hdr("Cleaning (TV Nurse)");
    free_memory(nurse)?;

    let (usage, apps) = nurse.apps()?;
    let targets: Vec<String> = opts
        .deep
        .as_deref()
        .map(|d| d.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if !targets.is_empty() {
        deep_clean(nurse, &apps, &targets, opts)?;
    }

    hdr("App caches");
    if !usage {
        warn("allow Usage access on the TV Nurse screen, so caches can be measured and cleared");
        println!();
        return Ok(());
    }
    let mut big: Vec<App> = apps
        .iter()
        .filter(|a| a.cache.unwrap_or(0) >= 20 * 1024 * 1024)
        .cloned()
        .collect();
    big.sort_by(|a, b| b.cache.cmp(&a.cache));
    big.truncate(8);
    if big.is_empty() {
        ok("no app has a cache over 20 MB. Nothing worth clearing");
        println!();
        return Ok(());
    }
    clear_caches(nurse, &big)
}

/// `tvdoctor nurse clean PKG...`: clear the caches of the apps named, whatever
/// their size.
fn clean_named(nurse: &Nurse, packages: &[String]) -> Result<(), String> {
    hdr("Clearing caches (TV Nurse)");
    let (usage, apps) = nurse.apps()?;
    if !usage {
        return Err("allow Usage access on the TV Nurse screen, so caches can be measured".to_string());
    }
    let mut chosen = Vec::new();
    for pkg in packages {
        match apps.iter().find(|a| &a.package == pkg) {
            Some(app) => chosen.push(app.clone()),
            None => warn(&format!("{} is not installed", pkg)),
        }
    }
    if chosen.is_empty() {
        println!();
        return Ok(());
    }
    clear_caches(nurse, &chosen)
}

/// Open each app's page for the person to press Clear cache, and check it.
fn clear_caches(nurse: &Nurse, big: &[App]) -> Result<(), String> {
    for app in big {
        println!("  {:>10}  {}", human(app.cache.unwrap_or(0)), app.label);
    }
    if config::dry_run() {
        ui::dry("would open each app's page for Clear cache");
        return Ok(());
    }
    println!();
    info("the TV shows each app's page. Press Clear cache, not Clear data");

    let mut cleared = 0u64;
    for app in big {
        let before = app.cache.unwrap_or(0);
        nurse.open_app_page(&app.package)?;
        match ask(&format!("{}: press Clear cache on the TV ({})", app.label, human(before))) {
            Answer::Stop => break,
            Answer::Skip => continue,
            Answer::Done => {}
        }
        let after = nurse.app(&app.package)?.and_then(|a| a.cache).unwrap_or(before);
        if after < before {
            cleared += before - after;
            ok(&format!("{}: {} cleared", app.label, human(before - after)));
        } else {
            warn(&format!("{}: the cache did not shrink. Was Clear cache pressed?", app.label));
        }
    }
    press_home(&nurse.ip);
    println!();
    kv("Cache cleared", &human(cleared));
    println!();
    Ok(())
}

/// Clear an app's data: signs it out. Only with --yes, never protected apps.
fn deep_clean(nurse: &Nurse, apps: &[App], targets: &[String], opts: &Opts) -> Result<(), String> {
    hdr("Deep clean");
    warn("deep clean wipes app data. You will have to sign in again.");
    let keep = config::load().keep;
    for pkg in targets {
        if is_protected(pkg, &keep) {
            bad(&format!("{} is protected, refusing to clear it", pkg));
            continue;
        }
        let Some(app) = apps.iter().find(|a| &a.package == pkg) else {
            warn(&format!("{} is not installed", pkg));
            continue;
        };
        if !opts.yes || config::dry_run() {
            warn(&format!("would clear {} (re-run with --yes)", pkg));
            continue;
        }
        nurse.open_app_page(pkg)?;
        match ask(&format!("{}: press Clear data, then OK", app.label)) {
            Answer::Stop => break,
            Answer::Skip => continue,
            Answer::Done => {}
        }
        let after = nurse.app(pkg)?.and_then(|a| a.data);
        match (app.data, after) {
            (Some(before), Some(after)) if after < before => {
                config::journal("clear", pkg);
                ok(&format!("cleared {}", app.label));
            }
            (Some(_), Some(_)) => warn(&format!("{}: its data did not shrink", app.label)),
            _ => {
                config::journal("clear", pkg);
                ok(&format!("{}: done (allow Usage access to have this checked)", app.label));
            }
        }
    }
    Ok(())
}

/// Walk the person through setting the three animation scales to `target`,
/// checking each time. True when TV Nurse sees them set.
fn set_animations(nurse: &Nurse, target: f32) -> Result<bool, String> {
    let wanted = if target == 0.0 { "Animation off" } else { "Animation scale 1x" };
    let now = nurse.status()?.animations();
    let Some(now) = now else {
        warn("this TV Nurse is too old to read the animation settings. Update it");
        return Ok(false);
    };
    if now.iter().all(|s| *s == target) {
        ok(&format!("animations are already {}", if target == 0.0 { "off" } else { "at 1x" }));
        return Ok(true);
    }
    if config::dry_run() {
        ui::dry(&format!("would open Developer options to set the animations to {}", wanted));
        return Ok(false);
    }
    nurse.open_settings("developer")?;
    info("the TV is showing Developer options. Scroll down to Drawing, and set");
    info(&format!("Window animation scale, Transition animation scale and Animator duration scale to {}", wanted));
    for _ in 0..3 {
        if ask(&format!("set all three to {}", wanted)) != Answer::Done {
            return Ok(false);
        }
        let Some(scales) = nurse.status()?.animations() else {
            return Ok(false);
        };
        if scales.iter().all(|s| *s == target) {
            ok(&format!("animations {}", if target == 0.0 { "off" } else { "back to 1x" }));
            press_home(&nurse.ip);
            return Ok(true);
        }
        warn(&format!(
            "not yet: window {}x, transition {}x, animator {}x",
            scales[0], scales[1], scales[2]
        ));
    }
    Ok(false)
}

pub fn optimize(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    hdr("Optimising (TV Nurse)");
    if !opts.keep_animations && set_animations(nurse, 0.0)? {
        config::journal("animations", "0.0");
    }
    println!();
    info("recompiling apps, the other half of optimise, needs ADB.");
    info("No ordinary app is allowed to do it, so TV Nurse cannot.");
    println!();
    Ok(())
}

pub fn tune(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    hdr("Tuning (TV Nurse)");
    if let Some(value) = &opts.animations {
        let target = if value == "off" { 0.0 } else { 1.0 };
        if set_animations(nurse, target)? {
            config::journal("animations", if value == "off" { "0.0" } else { "1.0" });
        }
    }
    if let Some(limit) = opts.background_limit {
        let choice = if limit < 0 {
            "Standard limit".to_string()
        } else {
            format!("At most {} processes", limit)
        };
        if config::dry_run() {
            ui::dry(&format!("would open Developer options for Background process limit: {}", choice));
        } else {
            nurse.open_settings("developer")?;
            info("the TV is showing Developer options. Under Apps, find Background process limit");
            if ask(&format!("set it to {}", choice)) == Answer::Done {
                // Android does not let an app read this one back.
                config::journal("background_limit", &limit.to_string());
                ok(&format!("background process limit: {} (TV Nurse cannot check this one)", choice));
                press_home(&nurse.ip);
            }
        }
    }
    if opts.animations.is_none() && opts.background_limit.is_none() {
        let status = nurse.status()?;
        if let Some(s) = status.animations() {
            kv("Window animation", &format!("{}x", s[0]));
            kv("Transition animation", &format!("{}x", s[1]));
            kv("Animator duration", &format!("{}x", s[2]));
        }
        info("change with --animations on|off or --background-limit N");
    }
    println!();
    Ok(())
}

/// Switch one app off with the person at the remote. True when it took.
fn guided_disable(nurse: &Nurse, app: &App) -> Result<Option<bool>, String> {
    nurse.open_app_page(&app.package)?;
    match ask(&format!("{}: press Disable, then OK", app.label)) {
        Answer::Stop => return Ok(None),
        Answer::Skip => return Ok(Some(false)),
        Answer::Done => {}
    }
    let now = nurse.app(&app.package)?;
    Ok(Some(now.map(|a| !a.enabled).unwrap_or(false)))
}

pub fn bloat(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    let (usage, apps) = nurse.apps()?;
    let keep = config::load().keep;
    let key = key_packages(&apps);

    let known: Vec<(&App, &str)> = KNOWN_BLOAT
        .iter()
        .filter_map(|(pkg, desc)| apps.iter().find(|a| &a.package == pkg).map(|a| (a, *desc)))
        .collect();

    if opts.auto || opts.disable.is_some() {
        let mut targets: Vec<String> = Vec::new();
        if opts.auto {
            targets.extend(known.iter().filter(|(a, _)| a.enabled).map(|(a, _)| a.package.clone()));
        }
        if let Some(list) = &opts.disable {
            targets.extend(list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        }
        targets.dedup();

        hdr("Switching off (TV Nurse)");
        let mut stopped = false;
        for pkg in targets {
            if stopped {
                break;
            }
            if key.contains(&pkg) {
                bad(&format!("{} is one of your four main apps, refusing", pkg));
                continue;
            }
            if is_protected(&pkg, &keep) {
                bad(&format!("{} is protected (system critical), refusing", pkg));
                continue;
            }
            let Some(app) = apps.iter().find(|a| a.package == pkg) else {
                warn(&format!("{} is not installed", pkg));
                continue;
            };
            if !app.enabled {
                info(&format!("{} is already switched off", app.label));
                continue;
            }
            if !opts.yes || config::dry_run() {
                warn(&format!("would switch off {}  (re-run with --yes)", app.label));
                continue;
            }
            match guided_disable(nurse, app)? {
                None => stopped = true,
                Some(true) => {
                    config::journal("disable", &pkg);
                    ok(&format!("switched off {}", app.label));
                }
                Some(false) => warn(&format!("{} is still on", app.label)),
            }
        }
        press_home(&nurse.ip);
        println!();
        return Ok(());
    }

    hdr("Known extras on this TV");
    if known.is_empty() {
        ok("none of the usual extras are installed");
    }
    for (app, desc) in &known {
        let state = if app.enabled { "on " } else { "off" };
        println!("  [{}]  {:<44} {}", state, app.package, desc);
    }

    if usage {
        hdr("Not opened in 6 months");
        let idle = idle_apps(&apps);
        if idle.is_empty() {
            ok("every app with a home-screen tile has been used recently");
        }
        for app in idle {
            let size = app.size.map(human).unwrap_or_default();
            println!("  {:<30} {:>10}  {}", app.label, size, ui::c("2", &app.package));
        }
    } else {
        println!();
        info("allow Usage access on the TV Nurse screen to also list apps nobody uses");
    }

    let disabled: Vec<&App> = apps.iter().filter(|a| !a.enabled).collect();
    if !disabled.is_empty() {
        hdr("Already switched off");
        for app in disabled {
            println!("  {:<30} {}", app.label, ui::c("2", &app.package));
        }
    }
    println!();
    info("switch the extras off:  tvdoctor bloat --auto --yes");
    info("or chosen ones:         tvdoctor bloat --disable PKG,PKG --yes");
    println!();
    Ok(())
}

pub fn restore(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    let entries: Vec<config::Entry> = config::read_journal()
        .into_iter()
        .filter(|e| matches!(e.action.as_str(), "disable" | "uninstall" | "animations" | "background_limit"))
        .collect();
    if entries.is_empty() {
        info("nothing in the journal to restore");
        return Ok(());
    }
    hdr("Journal");
    for entry in &entries {
        println!("  {}  {:<16} {}", entry.ts, entry.action, entry.target);
    }
    if !opts.yes || config::dry_run() {
        println!();
        info("re-run with --yes to undo these");
        return Ok(());
    }

    hdr("Restoring (TV Nurse)");
    let mut all_done = true;
    let mut animations_done = false;
    for entry in &entries {
        match entry.action.as_str() {
            "disable" => {
                let Some(app) = nurse.app(&entry.target)? else {
                    warn(&format!("{} is no longer installed", entry.target));
                    continue;
                };
                if app.enabled {
                    ok(&format!("{} is already on", app.label));
                    continue;
                }
                nurse.open_app_page(&app.package)?;
                match ask(&format!("{}: press Enable", app.label)) {
                    Answer::Stop => {
                        all_done = false;
                        break;
                    }
                    Answer::Skip => {
                        all_done = false;
                        continue;
                    }
                    Answer::Done => {}
                }
                if nurse.app(&app.package)?.map(|a| a.enabled).unwrap_or(false) {
                    ok(&format!("switched {} back on", app.label));
                } else {
                    all_done = false;
                    warn(&format!("{} is still off", app.label));
                }
            }
            "animations" if !animations_done => {
                animations_done = true;
                if !set_animations(nurse, 1.0)? {
                    all_done = false;
                }
            }
            "background_limit" => {
                nurse.open_settings("developer")?;
                if ask("set Background process limit to Standard limit") != Answer::Done {
                    all_done = false;
                }
            }
            "uninstall" => {
                all_done = false;
                warn(&format!(
                    "{} was removed with ADB, and only ADB can put it back",
                    entry.target
                ));
            }
            _ => {}
        }
    }
    press_home(&nurse.ip);
    if all_done {
        config::archive_journal();
        ok("everything is back as it was");
    } else {
        info("some steps were skipped, so the journal is kept for next time");
    }
    println!();
    Ok(())
}

pub fn apps(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    let (_, all) = nurse.apps()?;
    hdr("DStv, YouTube, Netflix, Spotify");
    for (label, app) in key_apps(&all) {
        let Some(app) = app else {
            bad(&format!("{:<8} not installed on this TV", label));
            continue;
        };
        println!();
        kv("App", label);
        kv("Package", &app.package);
        kv("Version", &app.version);
        kv("State", if app.enabled { "enabled" } else { "disabled" });
        if let Some(updated) = app.updated {
            kv("Last updated", &date(updated));
        }
        if let Some(used) = app.last_used {
            kv("Last used", &date(used));
        }
        if let Some(size) = app.size {
            kv("Size", &format!("{} (cache {})", human(size), human(app.cache.unwrap_or(0))));
        }
        if let Some(wanted) = &opts.launch {
            if label.to_lowercase().contains(&wanted.to_lowercase()) {
                nurse.launch(&app.package)?;
                ok(&format!("opened {} on the TV", label));
            }
        }
    }
    if opts.restart {
        println!();
        info("TV Nurse cannot force-stop one app, but it can close everything in the background");
        free_memory(nurse)?;
    }
    println!();
    Ok(())
}

pub fn update(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    let status = nurse.status()?;
    hdr("System software");
    kv("Android version", &status.field("android"));
    kv("Model", &format!("{} {}", status.field("maker"), status.field("model")));
    println!();
    info("A TV this age will not get a new Android version. Check anyway:");
    info("  Settings -> Device Preferences -> About -> System update");

    let (_, all) = nurse.apps()?;
    hdr("Your four apps");
    let found = key_apps(&all);
    for (label, app) in &found {
        match app {
            None => warn(&format!("{:<8} not installed", label)),
            Some(a) => ok(&format!(
                "{:<8} v{:<18} last updated {}",
                label,
                a.version,
                a.updated.map(date).unwrap_or_else(|| "?".to_string())
            )),
        }
    }

    if opts.open && !config::dry_run() {
        if nurse.open_settings("update").is_ok() {
            ask("this is the system update screen. Install anything it offers");
        }
        hdr("Play Store pages");
        for (label, app) in &found {
            let Some(app) = app else { continue };
            nurse.open_store(&app.package)?;
            if ask(&format!("{}: press Update if the Play Store offers it", label)) == Answer::Stop {
                break;
            }
        }
        press_home(&nurse.ip);
    } else {
        println!();
        info("run with --open to put each app's Play Store page on the TV,");
        info("then press Update with the remote where it is offered.");
    }
    println!();
    Ok(())
}

pub fn full(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    hdr("Full service (TV Nurse)");
    info("report -> clean -> optimise -> app check");
    report(nurse)?;
    clean(nurse, opts)?;
    optimize(nurse, opts)?;
    apps(nurse, opts)?;
    hdr("Finished");
    ok("restart the TV to finish:  tvdoctor reboot --yes");
    println!();
    Ok(())
}

/// No app may restart the TV. TV Nurse opens Settings > About, where Android
/// TV keeps its own Restart, and the person presses it. Holding the power
/// button with the remote is the fallback: some TVs (this Hisense among them)
/// ignore a held power button from a remote.
pub fn reboot(nurse: &Nurse, opts: &Opts) -> Result<(), String> {
    hdr("Restarting the TV");
    if !opts.yes {
        warn("this will restart the TV. Re-run with --yes");
        return Ok(());
    }
    if config::dry_run() {
        ui::dry("would open Settings > About for Restart");
        return Ok(());
    }
    match nurse.open_settings("about") {
        Ok(()) => {
            ok("the TV is showing Settings > About");
            if ask("select Restart near the bottom, then Restart again to confirm") == Answer::Done {
                ok("the TV comes back in a minute or two, and TV Nurse starts with it");
            }
        }
        Err(_) => hold_power(&nurse.ip)?,
    }
    println!();
    Ok(())
}

/// The fallback: hold the power button for the TV's own power menu.
fn hold_power(ip: &str) -> Result<(), String> {
    if !remote::is_paired(ip) {
        return Err(
            "this TV Nurse cannot open Settings > About. Update TV Nurse, or pair the remote: \
             tvdoctor remote pair"
                .to_string(),
        );
    }
    let session = remote::Remote::connect(ip)?;
    session.press(26, remote::Press::Hold(Duration::from_millis(3000)))?;
    std::thread::sleep(Duration::from_millis(300));
    ok("held the power button. If the TV shows a power menu, choose Restart");
    info("if nothing appeared, restart from Settings > Device Preferences > About > Restart");
    ask("choose Restart on the TV");
    Ok(())
}

/// One of the four main apps by name (DStv, YouTube, Netflix, Spotify), or a
/// package name as it is.
fn resolve_app(nurse: &Nurse, wanted: &str) -> Result<(String, String), String> {
    if wanted.contains('.') {
        return Ok((wanted.to_string(), wanted.to_string()));
    }
    let (_, apps) = nurse.apps()?;
    key_apps(&apps)
        .into_iter()
        .find(|(label, _)| label.eq_ignore_ascii_case(wanted))
        .and_then(|(label, app)| app.map(|a| (label.to_string(), a.package)))
        .ok_or(format!("{} is not installed on this TV", wanted))
}

/// Open an app through TV Nurse, which can start anything installed, DStv
/// included. Cast and DIAL never could.
pub fn open_app(nurse: &Nurse, wanted: &str) -> Result<(), String> {
    let (label, pkg) = resolve_app(nurse, wanted)?;
    hdr(&format!("Opening {}", label));
    kv("Route", "TV Nurse");
    if config::dry_run() {
        ui::dry(&format!("would open {}", pkg));
        return Ok(());
    }
    nurse.launch(&pkg)?;
    ok(&format!("{} is starting on the TV", label));
    println!();
    Ok(())
}

/// Close an app. No ordinary app may force-stop the one on screen, so the TV
/// goes Home first, which sends the app to the background, and then TV Nurse
/// closes what is in the background, the app included.
pub fn close_app(nurse: &Nurse, wanted: &str) -> Result<(), String> {
    let (label, _) = resolve_app(nurse, wanted)?;
    hdr(&format!("Closing {}", label));
    kv("Route", "Home, then TV Nurse closes the background");
    if config::dry_run() {
        ui::dry("would go Home and close the background apps");
        return Ok(());
    }
    if remote::is_paired(&nurse.ip) {
        press_home(&nurse.ip);
    } else {
        // No remote: the launcher is an app like any other.
        let _ = nurse.launch("com.google.android.tvlauncher");
    }
    std::thread::sleep(Duration::from_millis(1200));
    let (before, after) = nurse.free_memory()?;
    ok(&format!("{} is closed", label));
    if after > before {
        kv("Memory back", &human(after - before));
    }
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// tvdoctor nurse ...
// ---------------------------------------------------------------------------
pub fn command(opts: &Opts) -> Result<(), String> {
    let ip = crate::cmds::remembered_ip(opts)?;
    let sub = opts.rest.first().map(|s| s.as_str()).unwrap_or("status");
    if sub == "status" {
        return status_command(&ip);
    }
    if sub == "pair" {
        return pair(&ip, opts.rest.get(1).map(|s| s.as_str()).or(opts.code.as_deref()));
    }
    let nurse = need(&ip)?;
    match sub {
        "report" => report(&nurse),
        "free" => {
            hdr("Freeing memory (TV Nurse)");
            free_memory(&nurse)?;
            println!();
            Ok(())
        }
        "clean" if opts.rest.len() > 1 => clean_named(&nurse, &opts.rest[1..]),
        "clean" => clean(&nurse, opts),
        "open" | "launch" | "store" => {
            let pkg = opts
                .rest
                .get(1)
                .ok_or(format!("name an app, e.g. tvdoctor nurse {} com.netflix.ninja", sub))?;
            match sub {
                "open" => nurse.open_app_page(pkg)?,
                "launch" => nurse.launch(pkg)?,
                _ => nurse.open_store(pkg)?,
            }
            ok(&format!("done: {} {}", sub, pkg));
            Ok(())
        }
        other => Err(format!(
            "unknown nurse command '{}'. Try: status, pair, report, free, clean, open, launch, store",
            other
        )),
    }
}

/// TV Nurse for `ip`, or an error saying exactly what is missing.
pub fn need(ip: &str) -> Result<Nurse, String> {
    if let Some(nurse) = Nurse::reach(ip, 6000) {
        return Ok(nurse);
    }
    Err(match link(ip, 4000) {
        Link::Absent => format!("TV Nurse is not answering on {}:{}. Open TV Nurse on the TV", ip, PORT),
        Link::NeedsCode { locked: true } => {
            "TV Nurse is locked for a minute after too many wrong codes. Wait, then: tvdoctor nurse pair CODE"
                .to_string()
        }
        _ => EXPIRED.to_string(),
    })
}

fn status_command(ip: &str) -> Result<(), String> {
    hdr("TV Nurse");
    kv("TV", ip);
    let code = saved_code(ip);
    let Some(body) = call(ip, "GET", "/status", code.as_deref(), 6000) else {
        warn(&format!("TV Nurse is not answering on {}:{}", ip, PORT));
        println!();
        println!("  Is it installed? See the TV Nurse section of the README. If it is,");
        println!("  open TV Nurse on the TV once, and set Energy optimisation to off so");
        println!("  Android leaves it running.");
        println!();
        return Ok(());
    };
    let status = Status { body };
    ok(&format!("TV Nurse {} is answering", status.version()));
    match (code.is_some(), status.paired()) {
        (_, true) => {
            let minutes = (status.field("codeExpiresInS").parse::<u64>().unwrap_or(0) + 59) / 60;
            ok(&format!("paired. The code changes in {} min, then TV Doctor asks again", minutes));
        }
        (true, false) => {
            forget_code(ip);
            warn("the code has changed (it changes every hour). Run: tvdoctor nurse pair CODE");
        }
        (false, false) => warn("not paired. Open TV Nurse on the TV and run: tvdoctor nurse pair CODE"),
    }
    if status.usage_access() {
        ok("usage access: app sizes, caches and last-used dates");
    } else {
        warn("no usage access yet: allow it on the TV Nurse screen");
    }
    if status.energy_exempt() {
        ok("energy optimisation off: Android leaves TV Nurse running");
    } else {
        warn("energy optimisation is on: Android may stop TV Nurse in the background");
    }
    print_vitals(&status);
    println!();
    Ok(())
}

fn pair(ip: &str, code: Option<&str>) -> Result<(), String> {
    let code: String = code
        .ok_or("give the code from the TV Nurse screen, e.g. tvdoctor nurse pair 482913")?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if code.len() != 6 {
        return Err("the code on the TV Nurse screen is six digits".to_string());
    }
    let body = call(ip, "GET", "/status", Some(&code), 6000)
        .ok_or(format!("TV Nurse is not answering on {}:{}", ip, PORT))?;
    if !flag(&body, "paired") {
        return Err(if flag(&body, "lockedOut") {
            "TV Nurse is locked for a minute after too many wrong codes. Wait, then try again".to_string()
        } else {
            "TV Nurse did not accept that code. Check the TV Nurse screen".to_string()
        });
    }
    save_code(ip, &code);
    let minutes = (number(&body, "codeExpiresInS").unwrap_or(3600) + 59) / 60;
    ok(&format!("paired with TV Nurse for the next {} min. The code changes every hour", minutes));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"{"usageAccess":true,"apps":[
        {"package":"com.netflix.ninja","label":"Netflix","version":"9.1","system":false,
         "updatedSystem":false,"enabled":true,"launchable":true,"installed":1,"updated":1700000000000,
         "appBytes":100,"dataBytes":20,"cacheBytes":5,"lastUsed":1700000000000,"foregroundMs":9},
        {"package":"com.tvdoctor.helper","label":"TV Nurse","version":"0.3.0","system":false,
         "enabled":true,"launchable":true},
        {"package":"com.google.android.music","label":"Play Music","version":"1","system":true,
         "enabled":false,"launchable":true,"lastUsed":0}]}"#;

    #[test]
    fn report_parses_and_hides_itself() {
        let apps = parse_apps(REPORT);
        assert_eq!(apps.len(), 2);
        let netflix = &apps[0];
        assert_eq!(netflix.label, "Netflix");
        assert_eq!(netflix.size, Some(125));
        assert_eq!(netflix.cache, Some(5));
        assert!(netflix.enabled && netflix.launchable && !netflix.system);
        assert!(!apps[1].enabled && apps[1].system);
        assert_eq!(apps[1].size, None);
    }

    #[test]
    fn status_reads_nested_values() {
        let status = Status {
            body: r#"{"version":"0.3.0","paired":true,"usageAccess":false,
                "storage":{"total":100,"free":40},"memory":{"total":10,"available":3,"low":false},
                "animations":{"window":"1.0","transition":"0.5","animator":"0.0"}}"#
                .to_string(),
        };
        assert!(status.paired() && !status.usage_access());
        assert_eq!(status.storage(), Some((100, 40)));
        assert_eq!(status.memory(), Some((10, 3)));
        assert_eq!(status.animations(), Some([1.0, 0.5, 0.0]));
    }

    #[test]
    fn key_apps_found_by_package_or_fragment() {
        let apps = parse_apps(REPORT);
        let found = key_apps(&apps);
        let netflix = found.iter().find(|(l, _)| *l == "Netflix").unwrap();
        assert_eq!(netflix.1.as_ref().unwrap().package, "com.netflix.ninja");
        assert!(found.iter().find(|(l, _)| *l == "DStv").unwrap().1.is_none());
    }
}
