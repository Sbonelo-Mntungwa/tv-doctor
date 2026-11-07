//! What each command does.

use std::time::Duration;

use crate::adb::{self, Tv};
use crate::cast;
use crate::catalog::{
    is_known_bloat, is_protected, ANIMATION_KEYS, ENABLE_ADB_HELP, KNOWN_BLOAT,
};
use crate::config;
use crate::net::{self, ADB_PORT};
use crate::remote;
use crate::ui::{self, bad, hdr, human, info, kv, ok, warn};

/// Everything the command line can ask for.
#[derive(Default, Clone)]
pub struct Opts {
    pub ip: Option<String>,
    pub dry_run: bool,
    pub yes: bool,
    pub deep: Option<String>,
    pub aggressive: bool,
    pub keep_animations: bool,
    pub animations: Option<String>,
    pub background_limit: Option<i64>,
    pub auto: bool,
    pub disable: Option<String>,
    pub open: bool,
    pub restart: bool,
    pub launch: Option<String>,
    pub subnet: Option<String>,
    pub host: Option<String>,
    pub code: Option<String>,
    pub no_open: bool,
    pub rest: Vec<String>,
}

const QUICK: Duration = Duration::from_secs(90);
const SLOW: Duration = Duration::from_secs(900);
const VERY_SLOW: Duration = Duration::from_secs(3600);

fn comma_list(value: &Option<String>) -> Vec<String> {
    value
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Readings
// ---------------------------------------------------------------------------

/// (total, available) bytes on /data.
fn data_free(tv: &Tv) -> Option<(u64, u64)> {
    let out = tv.sh("df /data");
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 6 && parts[parts.len() - 1] == "/data" {
            let total = parts[1].parse::<u64>();
            let avail = parts[3].parse::<u64>();
            if let (Ok(total), Ok(avail)) = (total, avail) {
                return Some((total * 1024, avail * 1024));
            }
        }
    }
    None
}

/// MemTotal and whatever counts as free, in bytes.
fn meminfo(tv: &Tv) -> (u64, u64) {
    let out = tv.sh("cat /proc/meminfo");
    let read = |key: &str| -> u64 {
        out.lines()
            .find(|line| line.starts_with(&format!("{}:", key)))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(0)
    };
    let total = read("MemTotal");
    let mut free = read("MemAvailable");
    if free == 0 {
        free = read("MemFree");
    }
    (total, free)
}

fn diskstat(out: &str, label: &str) -> Option<u64> {
    out.lines()
        .find(|line| line.trim_start().starts_with(&format!("{}:", label)))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|v| v.trim().parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------
pub fn report(tv: &Tv) {
    hdr("Television");
    kv("ADB serial", &tv.serial);
    kv("Manufacturer", &tv.getprop("ro.product.manufacturer"));
    let mut model = tv.getprop("ro.product.model");
    if model.is_empty() {
        model = tv.getprop("ro.product.device");
    }
    kv("Model", &model);
    kv("Android version", &tv.getprop("ro.build.version.release"));
    kv("API level", &tv.getprop("ro.build.version.sdk"));
    kv("Build", &tv.getprop("ro.build.display.id"));
    kv("Build date", &tv.getprop("ro.build.date"));
    let patch = tv.getprop("ro.build.version.security_patch");
    kv(
        "Security patch",
        if patch.is_empty() { "not reported" } else { &patch },
    );
    if let Some(seconds) = tv
        .sh("cat /proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
    {
        let hours = seconds / 3600.0;
        kv("Uptime", &format!("{:.1} hours", hours));
        if hours > 72.0 {
            warn(&format!(
                "the TV has been up {:.0} days without a restart",
                hours / 24.0
            ));
        }
    }

    hdr("Storage");
    match data_free(tv) {
        Some((total, avail)) => {
            let used_pct = 100.0 * (total - avail) as f64 / total as f64;
            kv("Internal storage", &format!("{} total", human(total)));
            kv(
                "Free",
                &format!("{}  ({:.0}% used)", human(avail), used_pct),
            );
            if avail < 500 * 1024 * 1024 {
                bad("under 500 MB free. This alone will make the TV crawl and \
                     block app updates.");
            } else if avail < 1500 * 1024 * 1024 {
                warn("less than 1.5 GB free. Android TV gets sluggish below this.");
            } else {
                ok("enough free space");
            }
        }
        None => warn("could not read /data usage"),
    }

    let disk = tv.sh("dumpsys diskstats");
    for (label, key) in [
        ("App data", "App Size"),
        ("App caches", "App Cache Size"),
        ("Photos", "Photos Size"),
        ("Videos", "Videos Size"),
    ] {
        if let Some(bytes) = diskstat(&disk, key) {
            kv(label, &human(bytes));
        }
    }

    hdr("Memory");
    let (total, free) = meminfo(tv);
    if total > 0 {
        kv("RAM total", &human(total));
        kv(
            "RAM available",
            &format!("{}  ({:.0}%)", human(free), 100.0 * free as f64 / total as f64),
        );
        let ratio = free as f64 / total as f64;
        if ratio < 0.15 {
            bad("very little free RAM. Apps are being killed and relaunched \
                 constantly, which is what 'slow' usually feels like.");
        } else if ratio < 0.25 {
            warn("free RAM is low");
        } else {
            ok("RAM headroom is fine");
        }
    }
    let procs = tv.sh("ps -A -o NAME 2>/dev/null | wc -l");
    if procs.trim().chars().all(|c| c.is_ascii_digit()) && !procs.trim().is_empty() {
        kv("Processes running", procs.trim());
    }

    hdr("Packages");
    let disabled = tv.package_list("-d");
    kv("Total installed", &tv.packages().len().to_string());
    kv("Installed by you", &tv.package_list("-3").len().to_string());
    kv("Currently disabled", &disabled.len().to_string());

    hdr("The apps that matter");
    for (label, pkg) in tv.resolve_key_apps() {
        match pkg {
            None => warn(&format!("{:<8} not installed", label)),
            Some(pkg) => {
                let state = if disabled.contains(&pkg) {
                    "disabled"
                } else {
                    "enabled"
                };
                let line = format!(
                    "{:<8} {:<46} v{} ({})",
                    label,
                    pkg,
                    tv.version_of(&pkg),
                    state
                );
                if state == "enabled" {
                    ok(&line)
                } else {
                    warn(&line)
                }
            }
        }
    }

    hdr("Performance settings");
    let mut all_default = true;
    for key in ANIMATION_KEYS {
        let value = tv.sh(&format!("settings get global {}", key));
        kv(key, &value);
        if !matches!(value.trim(), "1.0" | "1" | "null" | "") {
            all_default = false;
        }
    }
    if all_default {
        warn("animations are at full speed. Turning them off makes the menus \
              feel noticeably faster (tvdoctor optimize).");
    }

    println!();
    println!(
        "{} tvdoctor clean      (free space, drop caches)",
        ui::c("1", "Next:")
    );
    println!("      tvdoctor optimize   (recompile apps, kill animations)");
    println!("      tvdoctor bloat      (see what is installed and unused)");
    println!("      tvdoctor update     (app and system updates)");
    println!();
}

// ---------------------------------------------------------------------------
// clean
// ---------------------------------------------------------------------------
pub fn clean(tv: &Tv, opts: &Opts) {
    let before = data_free(tv);
    let (_, mem_before) = meminfo(tv);

    hdr("Cleaning");
    if let Some((_, avail)) = before {
        kv("Free before", &human(avail));
    }

    info("clearing system log buffers");
    tv.sh_write("logcat -b all -c", "logcat -b all -c", QUICK);

    info("trimming every app cache on the TV");
    tv.sh_write("pm trim-caches 1000G", "pm trim-caches 1000G", SLOW);

    info("stopping background apps");
    tv.sh_write("am kill-all", "am kill-all", QUICK);

    info("clearing the Play Store download cache");
    tv.sh_write(
        "rm -rf /data/data/com.android.vending/cache/* 2>/dev/null",
        "rm -rf Play Store cache",
        QUICK,
    );

    info("running idle maintenance (filesystem trim)");
    tv.sh_write("sm idle-maint run", "sm idle-maint run", SLOW);

    let targets = comma_list(&opts.deep);
    if !targets.is_empty() {
        hdr("Deep clean");
        warn("deep clean wipes app data. You will have to sign in again.");
        let keep = config::load().keep;
        for pkg in targets {
            if is_protected(&pkg, &keep) {
                bad(&format!("{} is protected, refusing to clear it", pkg));
                continue;
            }
            if !tv.packages().contains(&pkg) {
                warn(&format!("{} is not installed", pkg));
                continue;
            }
            if !opts.yes {
                warn(&format!("would clear {} (re-run with --yes)", pkg));
                continue;
            }
            tv.sh_write(&format!("pm clear {}", pkg), &format!("pm clear {}", pkg), SLOW);
            config::journal("clear", &pkg);
            ok(&format!("cleared {}", pkg));
        }
    }

    std::thread::sleep(Duration::from_secs(2));
    let after = data_free(tv);
    let (_, mem_after) = meminfo(tv);

    hdr("Result");
    if let (Some((_, before)), Some((_, after))) = (before, after) {
        kv("Free before", &human(before));
        kv("Free after", &human(after));
        if after > before {
            ok(&format!("recovered {}", human(after - before)));
        } else {
            info("no space recovered; caches were already small");
        }
    }
    if mem_before > 0 && mem_after > 0 {
        kv("RAM free before", &human(mem_before));
        kv("RAM free after", &human(mem_after));
    }
    println!();
    info("a restart makes the gains stick:  tvdoctor reboot --yes");
    println!();
}

// ---------------------------------------------------------------------------
// optimize and tune
// ---------------------------------------------------------------------------
pub fn optimize(tv: &Tv, opts: &Opts) {
    hdr("Optimising");

    if !opts.keep_animations {
        info("switching off menu animations");
        for key in ANIMATION_KEYS {
            let command = format!("settings put global {} 0.0", key);
            tv.sh_write(&command, &command, QUICK);
        }
        config::journal("animations", "0.0");
        ok("animations off (tvdoctor tune --animations on puts them back)");
    }

    let mode = if opts.aggressive { "speed" } else { "speed-profile" };
    info(&format!(
        "recompiling every app with mode '{}'. On a 6 year old TV this is the \
         single biggest win and can take 10-30 minutes.",
        mode
    ));
    let command = format!("cmd package compile -m {} -f -a", mode);
    let out = tv.sh_write(&command, &command, VERY_SLOW);
    for line in out.lines().filter(|l| !l.trim().is_empty()).rev().take(3) {
        println!("        {}", line.trim());
    }
    ok("recompile finished");

    info("running the background dexopt job");
    tv.sh_write(
        "cmd package bg-dexopt-job",
        "cmd package bg-dexopt-job",
        VERY_SLOW,
    );

    info("running idle maintenance");
    tv.sh_write("sm idle-maint run", "sm idle-maint run", SLOW);

    println!();
    ok("done. Restart the TV to feel it:  tvdoctor reboot --yes");
    println!();
}

pub fn tune(tv: &Tv, opts: &Opts) {
    hdr("Tuning");
    let mut changed = false;

    if let Some(state) = &opts.animations {
        let value = if state == "off" { "0.0" } else { "1.0" };
        for key in ANIMATION_KEYS {
            let command = format!("settings put global {} {}", key, value);
            tv.sh_write(&command, &command, QUICK);
        }
        config::journal("animations", value);
        ok(&format!("animations {}", state));
        changed = true;
    }

    if let Some(limit) = opts.background_limit {
        let command = format!("settings put global background_process_limit {}", limit);
        tv.sh_write(&command, &command, QUICK);
        let readback = tv.sh("settings get global background_process_limit");
        kv("background_process_limit", &readback);
        if readback.trim() != limit.to_string() && !tv.dry_run {
            warn("this TV ignores background_process_limit from the shell; set it \
                  in Developer options instead");
        } else {
            config::journal("background_limit", &limit.to_string());
            ok(&format!("background process limit set to {}", limit));
        }
        changed = true;
    }

    if !changed {
        for key in ANIMATION_KEYS {
            kv(key, &tv.sh(&format!("settings get global {}", key)));
        }
        kv(
            "background_process_limit",
            &tv.sh("settings get global background_process_limit"),
        );
    }
}

// ---------------------------------------------------------------------------
// bloat
// ---------------------------------------------------------------------------
pub fn bloat(tv: &Tv, opts: &Opts) {
    let keep = config::load().keep;
    let key_pkgs: Vec<String> = tv
        .resolve_key_apps()
        .into_iter()
        .filter_map(|(_, pkg)| pkg)
        .collect();
    let installed = tv.packages();
    let disabled = tv.package_list("-d");
    let third = tv.package_list("-3");

    let present: Vec<(&str, &str)> = KNOWN_BLOAT
        .iter()
        .filter(|(pkg, _)| installed.iter().any(|p| p == pkg))
        .map(|(pkg, desc)| (*pkg, *desc))
        .collect();

    if opts.auto || opts.disable.is_some() {
        let mut targets: Vec<String> = Vec::new();
        if opts.auto {
            targets.extend(
                present
                    .iter()
                    .map(|(pkg, _)| pkg.to_string())
                    .filter(|pkg| !disabled.contains(pkg)),
            );
        }
        targets.extend(comma_list(&opts.disable));
        targets.dedup();

        hdr("Disabling");
        for pkg in targets {
            if key_pkgs.contains(&pkg) {
                bad(&format!("{} is one of your four main apps, refusing", pkg));
                continue;
            }
            if is_protected(&pkg, &keep) {
                bad(&format!("{} is protected (system critical), refusing", pkg));
                continue;
            }
            if !installed.contains(&pkg) {
                warn(&format!("{} is not installed", pkg));
                continue;
            }
            if disabled.contains(&pkg) {
                info(&format!("{} is already disabled", pkg));
                continue;
            }
            if !opts.yes {
                warn(&format!("would disable {}  (re-run with --yes)", pkg));
                continue;
            }
            let command = format!("pm disable-user --user 0 {}", pkg);
            let out = tv.sh_write(&command, &command, QUICK);
            if out.contains("new state: disabled") || tv.dry_run {
                config::journal("disable", &pkg);
                ok(&format!("disabled {}", pkg));
            } else {
                let first = out.lines().next().unwrap_or("no output from pm");
                bad(&format!("could not disable {}: {}", pkg, first));
            }
        }
        println!();
        return;
    }

    hdr("Preinstalled extras found on this TV");
    if present.is_empty() {
        ok("none of the usual preinstalled extras are present");
    } else {
        for (pkg, desc) in &present {
            let state = if disabled.iter().any(|d| d == pkg) {
                " (already disabled)"
            } else {
                ""
            };
            println!("  {:<52} {}{}", pkg, desc, state);
        }
        println!();
        info("switch all of these off with:  tvdoctor bloat --auto --yes");
    }

    hdr("Other apps you installed");
    let others: Vec<&String> = third
        .iter()
        .filter(|pkg| {
            !key_pkgs.contains(pkg) && !is_known_bloat(pkg) && !is_protected(pkg, &keep)
        })
        .collect();
    if others.is_empty() {
        ok("nothing else installed");
    } else {
        for pkg in &others {
            let state = if disabled.contains(pkg) { " (disabled)" } else { "" };
            println!("  {:<52} v{}{}", pkg, tv.version_of(pkg), state);
        }
        println!();
        info("disable the ones you never use:");
        info("  tvdoctor bloat --disable <pkg1>,<pkg2> --yes");
    }

    hdr("Protected");
    println!("  Your four apps and every system package are refused by default:");
    for (label, pkg) in tv.resolve_key_apps() {
        println!(
            "    {:<8} {}",
            label,
            pkg.unwrap_or_else(|| "not installed".to_string())
        );
    }
    println!();
    info("everything disabled here is reversible:  tvdoctor restore --yes");
    println!();
}

// ---------------------------------------------------------------------------
// restore
// ---------------------------------------------------------------------------
pub fn restore(tv: &Tv, opts: &Opts) {
    let entries: Vec<config::Entry> = config::read_journal()
        .into_iter()
        .filter(|e| {
            matches!(
                e.action.as_str(),
                "disable" | "uninstall" | "animations" | "background_limit"
            )
        })
        .collect();

    if entries.is_empty() {
        info("nothing in the journal to restore");
        return;
    }

    hdr("Journal");
    for entry in &entries {
        println!("  {}  {:<16} {}", entry.ts, entry.action, entry.target);
    }

    if !opts.yes {
        println!();
        info("re-run with --yes to undo these");
        return;
    }

    hdr("Restoring");
    for entry in &entries {
        match entry.action.as_str() {
            "disable" => {
                let command = format!("pm enable {}", entry.target);
                tv.sh_write(&command, &command, QUICK);
                ok(&format!("re-enabled {}", entry.target));
            }
            "uninstall" => {
                let command = format!("cmd package install-existing {}", entry.target);
                tv.sh_write(&command, &command, SLOW);
                ok(&format!("reinstalled {}", entry.target));
            }
            "animations" => {
                for key in ANIMATION_KEYS {
                    let command = format!("settings put global {} 1.0", key);
                    tv.sh_write(&command, &command, QUICK);
                }
                ok("animations back to default");
            }
            "background_limit" => {
                let command = "settings put global background_process_limit -1";
                tv.sh_write(command, command, QUICK);
                ok("background process limit back to default");
            }
            _ => {}
        }
    }

    if !tv.dry_run {
        config::archive_journal();
    }
    println!();
}

// ---------------------------------------------------------------------------
// update and apps
// ---------------------------------------------------------------------------
pub fn update(tv: &Tv, opts: &Opts) {
    hdr("System software");
    kv("Android version", &tv.getprop("ro.build.version.release"));
    kv("Build", &tv.getprop("ro.build.display.id"));
    kv("Build date", &tv.getprop("ro.build.date"));
    let patch = tv.getprop("ro.build.version.security_patch");
    kv(
        "Security patch",
        if patch.is_empty() { "not reported" } else { &patch },
    );
    println!();
    info("A TV this age will not get a new Android version. Check anyway:");
    info("  Settings -> Device Preferences -> About -> System update");
    if opts.open {
        tv.sh_write(
            "am start -a android.settings.SYSTEM_UPDATE_SETTINGS",
            "open the System update screen",
            QUICK,
        );
        ok("opened the system update screen on the TV");
    }

    hdr("Your four apps");
    for (label, pkg) in tv.resolve_key_apps() {
        match pkg {
            None => warn(&format!("{:<8} not installed", label)),
            Some(pkg) => {
                let stamp = tv
                    .package_field(&pkg, "lastUpdateTime")
                    .unwrap_or_else(|| "?".to_string());
                ok(&format!(
                    "{:<8} v{:<18} last updated {}",
                    label,
                    tv.version_of(&pkg),
                    stamp
                ));
            }
        }
    }

    if opts.open {
        hdr("Opening Play Store pages on the TV");
        info("press Update on the TV with the remote where it offers it");
        for (label, pkg) in tv.resolve_key_apps() {
            let Some(pkg) = pkg else { continue };
            let command = format!(
                "am start -a android.intent.action.VIEW -d \"market://details?id={}\"",
                pkg
            );
            tv.sh_write(&command, &format!("open Play Store page for {}", label), QUICK);
            ok(&format!("opened {}", label));
            if !tv.dry_run {
                std::thread::sleep(Duration::from_secs(6));
            }
        }
    } else {
        println!();
        info("adb cannot press the Update button for you; Play Store does not \
              expose it. Run with --open to put each app's Play Store page on \
              the screen, then press Update with the remote.");
        info("Better: Play Store -> Settings -> Auto-update apps -> over any \
              network, and it keeps itself current.");
    }
    println!();
}

pub fn apps(tv: &Tv, opts: &Opts) {
    let disabled = tv.package_list("-d");
    hdr("DStv, YouTube, Netflix, Spotify");
    for (label, pkg) in tv.resolve_key_apps() {
        let Some(pkg) = pkg else {
            bad(&format!("{:<8} not installed on this TV", label));
            continue;
        };
        println!();
        kv("App", &label);
        kv("Package", &pkg);
        kv("Version", &tv.version_of(&pkg));
        kv(
            "State",
            if disabled.contains(&pkg) { "disabled" } else { "enabled" },
        );
        if let Some(stamp) = tv.package_field(&pkg, "firstInstallTime") {
            kv("Installed", &stamp);
        }
        if opts.restart && !disabled.contains(&pkg) {
            let command = format!("am force-stop {}", pkg);
            tv.sh_write(&command, &command, QUICK);
            ok(&format!("restarted (force-stopped) {}", label));
        }
        if let Some(wanted) = &opts.launch {
            if label.to_lowercase().contains(&wanted.to_lowercase()) {
                let command = format!(
                    "monkey -p {} -c android.intent.category.LAUNCHER 1",
                    pkg
                );
                tv.sh_write(&command, &format!("launch {}", label), QUICK);
                ok(&format!("launched {} on the TV", label));
            }
        }
    }
    println!();
}

pub fn full(tv: &Tv, opts: &Opts) {
    hdr("Full service");
    info("report -> clean -> optimise -> app check");
    report(tv);
    clean(tv, opts);
    optimize(tv, opts);
    apps(tv, opts);
    hdr("Finished");
    ok("restart the TV to finish:  tvdoctor reboot --yes");
    println!();
}

// ---------------------------------------------------------------------------
// Commands that do not need a connected TV
// ---------------------------------------------------------------------------
pub fn discover(opts: &Opts) {
    info("scanning the network for Android TV devices");
    let found = net::discover(opts.subnet.as_deref());
    hdr("Devices found");
    if found.is_empty() {
        warn("nothing that looks like an Android TV answered.");
        info("Is the TV switched on? A TV in standby does not answer.");
        if std::env::var("TVDOCTOR_LAUNCHED_FROM").as_deref() == Ok("bundle") {
            println!();
            warn("Launched from Finder, so the more likely cause is macOS:");
            info("System Settings -> Privacy & Security -> Local Network, and");
            info("switch on TV Doctor. Finder apps need that to scan the network.");
        }
        return;
    }
    for f in &found {
        println!();
        kv("IP", &f.ip);
        kv("Name", if f.friendly_name.is_empty() { "?" } else { &f.friendly_name });
        kv("Make / model", &f.describe());
        kv("Cast port 8008", if f.cast { "open" } else { "closed" });
        kv("Remote port 6466", if f.remote { "open" } else { "closed" });
        kv(
            "ADB port 5555",
            &if f.adb {
                ui::c("32", "open")
            } else {
                ui::c("33", "closed")
            },
        );
        if !f.adb {
            warn("ADB is off on this device, so tvdoctor cannot work on it yet");
        }
    }
    config::remember_ip(&found[0].ip);
    println!();
    ok(&format!(
        "remembered {} as your TV ({})",
        found[0].ip,
        config::config_path().display()
    ));
    if !found[0].adb {
        println!();
        println!("{}", ENABLE_ADB_HELP);
    }
    println!();
}

pub fn connect(opts: &Opts) -> Result<(), String> {
    let ip = match opts.ip.clone().or_else(|| config::load().ip) {
        Some(ip) if !ip.is_empty() => ip,
        _ => {
            let found = net::discover(None);
            if found.is_empty() {
                return Err(
                    "no TV found. Run tvdoctor discover with the TV switched on."
                        .to_string(),
                );
            }
            found[0].ip.clone()
        }
    };

    hdr(&format!("Connecting to {}", ip));
    if !net::port_open(&ip, ADB_PORT, 1500) {
        if let Some((name, maker, model)) = net::dial_describe(&ip) {
            kv("Device", &format!("{} {} ({})", maker, model, name));
        }
        bad("port 5555 is closed");
        println!();
        println!("{}", ENABLE_ADB_HELP);
        return Err("ADB is not reachable yet".to_string());
    }

    let serial = adb::connect(&ip)?;
    config::remember_ip(&ip);
    let tv = Tv::new(serial.clone(), false)?;
    ok(&format!(
        "connected to {} {} ({})",
        tv.getprop("ro.product.manufacturer"),
        tv.getprop("ro.product.model"),
        serial
    ));
    info("run:  tvdoctor report");
    println!();
    Ok(())
}

pub fn pair(opts: &Opts) -> Result<(), String> {
    let host = opts
        .host
        .clone()
        .ok_or("pair needs --host <ip>:<pairing-port>")?;
    let code = opts.code.clone().ok_or("pair needs --code <6 digits>")?;
    hdr("Pairing");
    let adb_bin = adb::adb_path()?;
    let (_, out, err) = adb::run(
        &adb_bin,
        &["pair".to_string(), host.clone(), code],
        Some(Duration::from_secs(60)),
    );
    let text = format!("{}{}", out, err);
    for line in text.lines() {
        println!("  {}", line);
    }
    if text.contains("Successfully paired") {
        let ip = host.split(':').next().unwrap_or("").to_string();
        config::remember_ip(&ip);
        ok(&format!("paired. Now run:  tvdoctor connect --ip {}", ip));
        Ok(())
    } else {
        Err("pairing failed".to_string())
    }
}

pub fn reboot(tv: &Tv, opts: &Opts) -> Result<(), String> {
    hdr("Restarting the TV");
    if !opts.yes {
        warn("this will restart the TV now. Re-run with --yes");
        return Ok(());
    }
    tv.reboot()?;
    ok("reboot sent. The TV comes back in a minute or two.");
    info("ADB reconnects on its own:  tvdoctor connect");
    println!();
    Ok(())
}

pub fn shell(tv: &Tv, opts: &Opts) {
    let command = opts.rest.join(" ");
    if command.trim().is_empty() {
        warn("nothing to run. Try:  tvdoctor shell 'df /data'");
        return;
    }
    println!("{}", tv.sh_timeout(&command, VERY_SLOW));
}

// ---------------------------------------------------------------------------
// Things that work without ADB
// ---------------------------------------------------------------------------

/// What the TV will tell us over the Cast and DIAL endpoints when ADB is not
/// available. Much less than `report`, but it needs nothing switched on.
pub fn remote_check(opts: &Opts) -> Result<(), String> {
    let ip = match opts.ip.clone().or_else(|| config::load().ip) {
        Some(ip) if !ip.is_empty() => ip,
        _ => {
            let found = net::discover(None);
            if found.is_empty() {
                return Err("no TV found. Is it switched on?".to_string());
            }
            found[0].ip.clone()
        }
    };

    hdr("Remote check");
    info("read over the Cast endpoint, so this works without ADB");
    kv("IP", &ip);

    if !net::port_open(&ip, net::CAST_PORT, 2000) {
        bad("the TV is not answering. A TV in standby does not answer.");
        return Ok(());
    }

    if let Some((name, maker, model)) = net::dial_describe(&ip) {
        kv("Name", &name);
        kv("Make / model", &format!("{} {}", maker, model).trim().to_string());
    }

    if let Some(body) = net::eureka(&ip) {
        if let Some(build) = net::json_field(&body, "cast_build_revision") {
            kv("Cast build", &build);
        }
        if let Some(update) = net::json_field(&body, "has_update") {
            kv(
                "System update waiting",
                if update == "true" { "yes" } else { "no" },
            );
            if update == "true" {
                warn("the TV is holding an update. Settings -> About -> System update");
            }
        }
        if let Some(eth) = net::json_field(&body, "ethernet_connected") {
            kv("Network", if eth == "true" { "wired ethernet" } else { "wi-fi" });
        }
        if let Some(uptime) = net::json_field(&body, "uptime").and_then(|v| v.parse::<f64>().ok())
        {
            let days = uptime / 86_400.0;
            kv("Uptime", &format!("{:.1} days", days));
            if days > 3.0 {
                warn("the TV has not been restarted in days. Restart it first; it \
                      costs nothing and often fixes slowness on its own.");
            }
        }
    }

    hdr("Apps the TV admits to over DIAL");
    info("DIAL only knows the apps that registered with it, so a blank is not proof");
    for app in ["Netflix", "YouTube", "Spotify", "DStv", "Prime Video"] {
        match net::dial_app(&ip, app) {
            Some((name, state)) if !state.is_empty() => {
                ok(&format!("{:<12} installed, currently {}", name, state))
            }
            Some((name, _)) => ok(&format!("{:<12} installed", name)),
            None => info(&format!("{:<12} no answer", app)),
        }
    }

    hdr("What this cannot tell you");
    println!("  Free storage, free RAM, app caches, app versions and which apps run");
    println!("  in the background all need ADB. Without it, run:  tvdoctor guide");
    println!();
    Ok(())
}

/// The same cleanup tvdoctor performs, written out as remote button presses,
/// for a TV that will not expose ADB on the network.
pub fn guide() {
    hdr("Cleaning this TV from the remote");
    println!("  Your set only offers USB debugging, so tvdoctor cannot drive it over");
    println!("  the network. Everything below is the same work, done by hand. Order");
    println!("  matters: the first two steps are where the speed comes from.");

    hdr("1. Clear the caches  (10 minutes, biggest win)");
    println!("  Settings -> Apps -> See all apps -> pick an app -> Clear cache");
    println!();
    println!("  Do it for these, in this order:");
    println!("    Google Play Store      usually the worst offender");
    println!("    YouTube");
    println!("    Netflix");
    println!("    DStv");
    println!("    Spotify");
    println!("    then anything else with a large size shown");
    println!();
    warn("Clear cache, not Clear data. Clear data signs you out of the app.");
    println!();
    println!("  Then check what you got back:");
    println!("    Settings -> Device Preferences -> Storage");
    println!("  Aim for at least 1.5 GB free. Below that the TV will crawl no matter");
    println!("  what else you do, and app updates fail silently.");

    hdr("2. Remove what you never use  (frees RAM, not just space)");
    println!("  Settings -> Apps -> See all apps -> pick an app ->");
    println!("    Uninstall   for apps you installed");
    println!("    Disable     for preinstalled ones that have no Uninstall");
    println!();
    println!("  Safe to disable on an Android TV:");
    for (pkg, desc) in KNOWN_BLOAT.iter().take(8) {
        println!("    {:<38} {}", desc, pkg);
    }
    println!();
    println!("  Leave alone: anything named Hisense, MediaTek, Google Play services,");
    println!("  Android System WebView, the launcher, the keyboard and TV inputs.");
    println!();
    info("Every app removed is one less thing waking up in the background. This is");
    info("what actually fixes 'everything feels like a cold start'.");

    hdr("3. Switch off the animations  (you have Developer options)");
    println!("  Settings -> Device Preferences -> Developer options, then set:");
    println!("    Window animation scale      -> Animation off");
    println!("    Transition animation scale  -> Animation off");
    println!("    Animator duration scale     -> Animation off");
    println!("    Background process limit    -> At most 4 processes");
    println!();
    info("These four are exactly what 'tvdoctor optimize' would have set. The menus");
    info("will feel instant afterwards, because nothing is waiting on a fade.");

    hdr("4. Get the four apps current");
    println!("  Play Store -> Settings -> Auto-update apps -> Over any network");
    println!("  Then Play Store -> My apps, and update DStv, YouTube, Netflix,");
    println!("  Spotify by hand this once.");
    println!();
    info("An old app version on a slow TV is often the whole problem, especially");
    info("with DStv, which has grown heavier every year.");

    hdr("5. Restart properly");
    println!("  Settings -> Device Preferences -> About -> Restart");
    println!();
    warn("Use Restart, not the power button. The power button is standby: the TV");
    warn("keeps running and nothing is released.");

    hdr("What you still cannot do by hand");
    println!("  Recompiling every installed app ('tvdoctor optimize') is the one");
    println!("  step with no menu equivalent, and on a TV this age it is worth real");
    println!("  speed. It needs ADB over the network.");
    println!();
    println!("  If you ever get ADB listening, the whole job becomes:");
    println!("    tvdoctor full && tvdoctor reboot --yes");

    hdr("Do not factory reset");
    println!("  It fixes the storage and the compilation, costs you every login and");
    println!("  every setting, and within a few months you are back here. Steps 1 to");
    println!("  3 get you most of the same result and cost you nothing.");
    println!();
}

/// Which TV to aim a no-ADB command at.
pub fn remembered_ip(opts: &Opts) -> Result<String, String> {
    match opts.ip.clone().or_else(|| config::load().ip) {
        Some(ip) if !ip.is_empty() => Ok(ip),
        _ => {
            let found = net::discover(None);
            if found.is_empty() {
                return Err("no TV found. Is it switched on?".to_string());
            }
            Ok(found[0].ip.clone())
        }
    }
}

/// DIAL app names are capitalised exactly, so accept friendlier spellings.
fn dial_name_of(input: &str) -> String {
    match input.to_lowercase().as_str() {
        "netflix" => "Netflix".to_string(),
        "youtube" => "YouTube".to_string(),
        "spotify" => "Spotify".to_string(),
        "dstv" => "DStv".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// How an app can be reached from the laptop.
pub enum Route {
    /// DIAL on port 8008, by registered app name.
    Dial(String),
    /// Cast v2 on port 8009, by Cast application id.
    Cast(String),
    /// Neither: the app ships no receiver of either kind.
    Unreachable,
    /// The TV did not answer, so nothing is known either way.
    Unknown,
}

/// Work out the best route to an app.
///
/// DIAL is tried first because an app that registers with DIAL answers it
/// reliably, and Netflix on this TV reports APP_UNAVAILABLE over Cast while
/// launching perfectly over DIAL. Cast is the fallback, and it is what carries
/// YouTube and Spotify, neither of which registers with DIAL here.
pub fn route_for(ip: &str, label: &str) -> Route {
    let entry = cast::app_entry(label);
    let (cast_id, dial_name) = match entry {
        Some((_, cast_id, dial_name)) => (cast_id, dial_name),
        // An app we do not know: still worth asking DIAL by name.
        None => ("", ""),
    };

    let dial_candidate = if dial_name.is_empty() {
        dial_name_of(label)
    } else {
        dial_name.to_string()
    };

    let mut asked_everything = true;
    match net::dial_app_state(ip, &dial_candidate) {
        net::DialApp::Registered { name, .. } => return Route::Dial(name),
        net::DialApp::NoAnswer => asked_everything = false,
        net::DialApp::NotRegistered => {}
    }

    if !cast_id.is_empty() {
        match cast::availability(ip, &[cast_id]) {
            Ok(list) => {
                if list.iter().any(|(_, available)| *available) {
                    return Route::Cast(cast_id.to_string());
                }
            }
            Err(_) => asked_everything = false,
        }
    }

    // Only call it unreachable when both questions actually got answered.
    if asked_everything {
        Route::Unreachable
    } else {
        Route::Unknown
    }
}

/// The label as the catalogue spells it, so "netflix" prints as "Netflix".
fn canonical(label: &str) -> String {
    cast::app_entry(label)
        .map(|(name, _, _)| name.to_string())
        .unwrap_or_else(|| dial_name_of(label))
}

fn why_unknown(label: &str) {
    warn(&format!("could not tell how to reach {}", label));
    info("The TV did not answer. It may be busy, or it may have just been asked");
    info("to start or stop something. Wait a moment and try again.");
}

fn why_unreachable(label: &str) {
    bad(&format!("{} cannot be started from here", label));
    info("It registers with neither DIAL (port 8008) nor Cast (port 8009), so");
    info("nothing on the network can launch it. The only remaining route is the");
    info("Android TV remote protocol on port 6466, which needs a pairing code");
    info("from the TV screen and a much larger implementation.");
    info("Start it with the remote instead.");
}

/// Start an app on the TV. Uses DIAL or Cast, whichever the app answers.
pub fn launch_app(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    let Some(wanted) = opts.rest.first().cloned().or_else(|| opts.launch.clone()) else {
        return Err("name an app, e.g.  tvdoctor launch youtube".to_string());
    };
    let label = canonical(&wanted);

    hdr(&format!("Launching {}", label));
    match route_for(&ip, &label) {
        Route::Dial(name) => {
            kv("Route", "DIAL, port 8008");
            match net::dial_launch(&ip, &name) {
                Some(status) => {
                    kv("TV replied", &status);
                    if status.contains("201") || status.contains("200") {
                        ok(&format!("{} is starting on the TV", label));
                    } else {
                        warn("unexpected reply, check the TV screen");
                    }
                }
                None => bad("no reply from the TV"),
            }
        }
        Route::Cast(app_id) => {
            kv("Route", "Cast v2, port 8009");
            kv("Cast app id", &app_id);
            match cast::launch(&ip, &app_id) {
                Ok(name) => {
                    let shown = if name.is_empty() { label.clone() } else { name };
                    ok(&format!("{} is starting on the TV", shown));
                }
                Err(message) => bad(&message),
            }
        }
        Route::Unreachable => why_unreachable(&label),
        Route::Unknown => why_unknown(&label),
    }
    println!();
    Ok(())
}

/// Stop an app on the TV, handing back the memory it was holding.
pub fn stop_app(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    let Some(wanted) = opts.rest.first().cloned().or_else(|| opts.launch.clone()) else {
        return Err("name an app, e.g.  tvdoctor stop youtube".to_string());
    };
    let label = canonical(&wanted);

    hdr(&format!("Stopping {}", label));
    match route_for(&ip, &label) {
        Route::Dial(name) => {
            kv("Route", "DIAL, port 8008");
            match net::dial_app_state(&ip, &name) {
                net::DialApp::Registered { state, .. } if state == "stopped" => {
                    info("it is not running, nothing to stop");
                }
                net::DialApp::Registered { state, .. } => {
                    kv("Current state", &state);
                    match net::dial_stop(&ip, &name) {
                        Some(status) => {
                            kv("TV replied", &status);
                            if status.contains("200") || status.contains("204") {
                                ok(&format!("{} stopped, and its memory is back", label));
                            } else {
                                warn("the TV did not confirm the stop");
                            }
                        }
                        None => bad("no reply from the TV"),
                    }
                }
                net::DialApp::NotRegistered => bad("the TV stopped reporting this app"),
                net::DialApp::NoAnswer => why_unknown(&label),
            }
        }
        Route::Cast(app_id) => {
            kv("Route", "Cast v2, port 8009");
            match cast::status(&ip)? {
                None => info("nothing is running to stop"),
                Some((name, running_id, session, idle)) => {
                    kv("On screen now", &name);
                    if idle {
                        info("the TV is on its idle screen, nothing to stop");
                    } else if running_id != app_id {
                        warn(&format!(
                            "{} is running, not {}. Stop that instead if you meant to.",
                            name, label
                        ));
                    } else if session.is_empty() {
                        warn("the TV gave no session id, so it cannot be stopped");
                    } else {
                        cast::stop(&ip, &session)?;
                        ok(&format!("{} stopped, and its memory is back", label));
                    }
                }
            }
        }
        Route::Unreachable => why_unreachable(&label),
        Route::Unknown => why_unknown(&label),
    }
    println!();
    Ok(())
}

/// Which of the four apps can be driven from the laptop, and by which route.
pub fn routes(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    hdr("How each app can be reached");
    info("checked live against the TV, no ADB involved");
    for (label, _, _) in cast::APPS {
        match route_for(&ip, label) {
            Route::Dial(name) => ok(&format!(
                "{:<8} DIAL on port 8008 (registered as \"{}\")",
                label, name
            )),
            Route::Cast(app_id) => {
                ok(&format!("{:<8} Cast v2 on port 8009 (app id {})", label, app_id))
            }
            Route::Unreachable => warn(&format!(
                "{:<8} no receiver of either kind, use the remote",
                label
            )),
            Route::Unknown => warn(&format!(
                "{:<8} the TV did not answer, so this is not known",
                label
            )),
        }
    }
    println!();
    match cast::status(&ip)? {
        Some((name, _, _, idle)) if !idle => ok(&format!("on screen now: {}", name)),
        Some((name, _, _, _)) => info(&format!("on screen now: {} (idle screen)", name)),
        None => info("the TV reports nothing running"),
    }
    println!();
    Ok(())
}

/// Send the television back to its home screen, stopping what is on it.
///
/// For an app with a receiver this is a real stop. For one without, like DStv,
/// there is no stop to ask for, so the screen is taken by an app that does have
/// a receiver and that session is then ended, which drops the original app out
/// of the foreground. Android reclaims the memory of a backgrounded app under
/// pressure, which is the point, but this is not the same as `am force-stop`
/// over ADB and it is not claimed to be.
pub fn force_stop(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    let named = opts.rest.first().cloned().or_else(|| opts.launch.clone());

    hdr("Force stop");

    // An app with a receiver can simply be told to stop.
    if let Some(wanted) = &named {
        let label = canonical(wanted);
        match route_for(&ip, &label) {
            Route::Dial(_) | Route::Cast(_) => {
                info(&format!("{} has a receiver, so it can be stopped properly", label));
                println!();
                return stop_app(opts);
            }
            Route::Unreachable => {
                kv("App", &label);
                warn(&format!(
                    "{} has no receiver, so there is nothing to send a stop to",
                    label
                ));
                return displace(&ip, Some(&label));
            }
            Route::Unknown => {
                kv("App", &label);
                why_unknown(&label);
                println!();
                return Ok(());
            }
        }
    }

    displace(&ip, None)
}

/// Take the screen from whatever holds it, then hand the TV back to its home
/// screen.
fn displace(ip: &str, blamed: Option<&str>) -> Result<(), String> {
    let current = cast::status(ip)?;

    // Something running under Cast can be ended directly.
    if let Some((name, _, session, idle)) = &current {
        if !idle && !session.is_empty() {
            kv("On screen now", name);
            if let Some(label) = blamed {
                if !name.eq_ignore_ascii_case(label) {
                    info(&format!(
                        "{} holds the screen, not {}. Ending it returns the TV",
                        name, label
                    ));
                    info(&format!(
                        "to its home screen, which drops {} out of the foreground too.",
                        label
                    ));
                }
            }
            cast::stop(ip, session)?;
            ok(&format!("{} stopped, and the TV is back on its home screen", name));
            println!();
            return Ok(());
        }
    }

    // Nothing is running under Cast. With no app named there is nothing to do,
    // and taking the screen anyway would be noise on a TV that is already home.
    // Note that a TV sitting on its home screen reports no applications at all,
    // not an idle one, so None has to be handled here too.
    if blamed.is_none() {
        match &current {
            Some((name, _, _, _)) => {
                kv("On screen now", name);
                info("already the idle screen, so there is nothing to stop");
            }
            None => info("nothing is running, the TV is already on its home screen"),
        }
        println!();
        return Ok(());
    }

    // Nothing to stop through Cast. Take the screen with an app that does
    // answer, then end that, which lands the TV on its home screen.
    let helper = ["YouTube", "Spotify", "Netflix"]
        .iter()
        .find_map(|label| match route_for(ip, label) {
            Route::Cast(app_id) => Some((label.to_string(), app_id)),
            _ => None,
        });

    let Some((helper_label, app_id)) = helper else {
        bad("no app on this TV can be launched, so the screen cannot be taken");
        info("Press Home on the remote instead.");
        println!();
        return Ok(());
    };

    info(&format!(
        "taking the screen with {} and then ending it, which drops the",
        helper_label
    ));
    info("app in front out of the foreground");

    cast::launch(ip, &app_id)?;
    std::thread::sleep(Duration::from_secs(5));

    match cast::status(ip)? {
        Some((_, _, session, idle)) if !idle && !session.is_empty() => {
            cast::stop(ip, &session)?;
            ok("the TV is back on its home screen");
        }
        _ => warn("could not confirm the handover, check the TV screen"),
    }

    if let Some(label) = blamed {
        println!();
        ok(&format!("{} no longer has the screen", label));
        warn("Android decides when to end a backgrounded app, so this frees its");
        warn("memory under pressure rather than killing it outright. A true force");
        warn("stop needs ADB, which this TV does not expose.");
    }
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// remote: this Mac as the TV's remote control, no ADB needed
// ---------------------------------------------------------------------------
pub fn remote(opts: &Opts) -> Result<(), String> {
    let sub = opts.rest.first().map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "keys" => {
            hdr("Remote keys");
            let names: Vec<&str> = remote::KEYS.iter().map(|(n, _)| *n).collect();
            println!("  {}", names.join(" "));
            println!();
            println!("  Digits can run together (1969). hold:NAME holds a key for 2s,");
            println!("  hold:NAME:SECONDS for longer, wait:SECONDS pauses.");
            println!();
            Ok(())
        }
        "pair" => remote_pair(opts),
        "forget" => {
            let ip = remembered_ip(opts)?;
            remote::forget_paired(&ip);
            ok(&format!("forgot the pairing with {}", ip));
            Ok(())
        }
        "status" => remote_status(opts),
        "press" => remote_press(opts),
        "link" => {
            let ip = remembered_ip(opts)?;
            let link = opts.rest.get(1).ok_or("give a link, e.g. tvdoctor remote link https://www.youtube.com")?;
            let session = remote::Remote::connect(&ip)?;
            session.open_link(link)?;
            std::thread::sleep(Duration::from_millis(300));
            ok(&format!("asked the TV to open {}", link));
            Ok(())
        }
        other => Err(format!(
            "unknown remote command '{}'. Try: pair, status, press, keys, link, forget",
            other
        )),
    }
}

fn remote_pair(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    hdr("Pair this Mac as a remote");
    kv("TV", &ip);
    info("asking the TV to pair...");
    let pairing = remote::start_pairing(&ip)?;
    ok("the TV is showing a code");
    for _ in 0..3 {
        let code = ui::prompt("  Type the code from the TV screen: ");
        if code == "q" || code.is_empty() {
            pairing.cancel();
            warn("pairing cancelled");
            return Ok(());
        }
        match pairing.finish(&code) {
            Ok(()) => {
                ok("paired. This Mac is now a remote for the TV");
                println!();
                println!("  Try:  tvdoctor remote press home");
                println!();
                return Ok(());
            }
            Err(message) if message.contains("does not match") || message.contains("six characters") => {
                warn(&message)
            }
            Err(message) => return Err(message),
        }
    }
    pairing.cancel();
    Err("three wrong codes. Run tvdoctor remote pair to try again".to_string())
}

fn remote_status(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    hdr("Remote");
    kv("TV", &ip);
    if !remote::is_paired(&ip) {
        warn("not paired yet. Run: tvdoctor remote pair");
        println!();
        return Ok(());
    }
    let session = remote::Remote::connect(&ip)?;
    // The TV reports power and volume just after the handshake.
    std::thread::sleep(Duration::from_millis(1200));
    let state = session.state();
    ok("connected as a remote");
    if !state.vendor.is_empty() || !state.model.is_empty() {
        kv("Device", format!("{} {}", state.vendor, state.model).trim());
    }
    if let Some(on) = state.on {
        kv("Screen", if on { "on" } else { "off" });
    }
    if let Some(volume) = state.volume {
        let max = state.volume_max.map(|m| format!(" of {}", m)).unwrap_or_default();
        let muted = if state.muted == Some(true) { ", muted" } else { "" };
        kv("Volume", &format!("{}{}{}", volume, max, muted));
    }
    if !state.app.is_empty() {
        kv("App in front", &state.app);
    }
    println!();
    Ok(())
}

fn remote_press(opts: &Opts) -> Result<(), String> {
    let ip = remembered_ip(opts)?;
    let mut steps = Vec::new();
    for token in opts.rest.iter().skip(1) {
        steps.extend(remote::parse_step(token)?);
    }
    if steps.is_empty() {
        return Err("name keys to press, e.g. tvdoctor remote press home down ok".to_string());
    }
    if config::dry_run() {
        ui::dry(&format!("would press {}", opts.rest[1..].join(" ")));
        return Ok(());
    }
    let session = remote::Remote::connect(&ip)?;
    session.run(&steps)?;
    // Let the last press reach the TV before the connection closes.
    std::thread::sleep(Duration::from_millis(300));
    ok(&format!("pressed {}", opts.rest[1..].join(" ")));
    Ok(())
}
