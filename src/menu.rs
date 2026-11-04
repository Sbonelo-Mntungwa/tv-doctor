//! The interactive menu you get when tvdoctor is run with no command.

use crate::adb::{self, Tv};
use crate::cmds::{self, Opts};
use crate::config;
use crate::net::{self, ADB_PORT};
use crate::ui::{self, bad, confirm, hdr, info, kv, ok, prompt, warn};

fn banner(tv: &Option<Tv>) {
    println!();
    println!("{}", ui::c("1;36", "  tvdoctor"));
    match tv {
        Some(tv) => {
            println!(
                "  {} {} at {}",
                tv.getprop("ro.product.manufacturer"),
                tv.getprop("ro.product.model"),
                tv.serial
            );
        }
        None => {
            let ip = config::load().ip.unwrap_or_default();
            if ip.is_empty() {
                println!("  {}", ui::c("33", "no TV found yet"));
            } else {
                println!("  {} at {}", ui::c("33", "ADB not available"), ip);
                println!("  the options below work without it");
            }
        }
    }
}

fn menu_lines(connected: bool) {
    println!();
    if connected {
        println!("   1  Health report       what is slowing the TV down right now");
        println!("   2  Clean               free space, drop caches, stop background apps");
        println!("   3  Optimise            recompile apps, switch off animations (slow)");
        println!("   4  Installed apps      see what is installed, switch off what you never use");
        println!("   5  Your four apps      DStv, YouTube, Netflix, Spotify");
        println!("   6  Updates             app and system update status");
        println!("   7  Full service        report, clean, optimise, app check");
        println!("   8  Restart the TV");
        println!("   9  Undo everything tvdoctor changed");
        println!("   0  Find the TV again / connection help");
    } else {
        println!("   g  Cleanup guide        clean the TV from the remote, no ADB needed");
        println!("   r  Remote check         what the TV reports over the network");
        println!("   l  Launch an app        start it on the TV from here");
        println!("   s  Stop an app          stop it, freeing the memory it held");
        println!("   0  Find the TV and show how to switch ADB on");
        println!("   c  Try to connect again");
    }
    if connected {
        println!("   g  Cleanup guide        the same job done from the remote");
    }
    println!("   q  Quit");
    println!();
}

/// Reconnect so each action runs against a live device.
fn reconnect(opts: &Opts) -> Option<Tv> {
    match adb::get_tv(opts.ip.as_deref(), opts.dry_run, false) {
        Ok(tv) => tv,
        Err(message) => {
            bad(&message);
            None
        }
    }
}

pub fn run(opts: &Opts) {
    let mut tv = reconnect(opts);

    loop {
        banner(&tv);
        menu_lines(tv.is_some());
        let choice = prompt("  choose: ").to_lowercase();

        match choice.as_str() {
            "q" | "quit" | "exit" | "" => {
                println!();
                return;
            }
            "0" => {
                cmds::discover(opts);
                tv = reconnect(opts);
            }
            "c" => {
                if let Err(message) = cmds::connect(opts) {
                    bad(&message);
                }
                tv = reconnect(opts);
            }
            "g" => cmds::guide(),
            "l" | "s" => {
                let wanted = prompt("  which app? [netflix] ");
                let mut opts = opts.clone();
                opts.rest = vec![if wanted.is_empty() || wanted == "q" {
                    "netflix".to_string()
                } else {
                    wanted
                }];
                let outcome = if choice == "l" {
                    cmds::launch_app(&opts)
                } else {
                    cmds::stop_app(&opts)
                };
                if let Err(message) = outcome {
                    bad(&message);
                }
            }
            "r" => {
                if let Err(message) = cmds::remote_check(opts) {
                    bad(&message);
                }
            }
            _ => {
                let Some(ref device) = tv else {
                    warn("not connected to a TV yet. Choose 0 for help.");
                    continue;
                };
                action(device, opts, &choice);
            }
        }
    }
}

fn action(tv: &Tv, opts: &Opts, choice: &str) {
    match choice {
        "1" => cmds::report(tv),
        "2" => {
            let mut opts = opts.clone();
            info("This clears caches only. Your logins are not touched.");
            if confirm("clean the TV now?") {
                opts.deep = None;
                cmds::clean(tv, &opts);
            }
        }
        "3" => {
            warn("Recompiling every app can take 10 to 30 minutes on this TV.");
            info("Leave the TV switched on while it runs.");
            if confirm("start optimising?") {
                cmds::optimize(tv, opts);
            }
        }
        "4" => {
            cmds::bloat(tv, opts);
            if confirm("switch off the preinstalled extras listed above?") {
                let mut opts = opts.clone();
                opts.auto = true;
                opts.yes = true;
                cmds::bloat(tv, &opts);
            } else {
                let answer = prompt(
                    "  packages to switch off, comma separated (blank to skip): ",
                );
                if !answer.is_empty() && answer != "q" {
                    let mut opts = opts.clone();
                    opts.disable = Some(answer);
                    opts.yes = true;
                    cmds::bloat(tv, &opts);
                }
            }
        }
        "5" => cmds::apps(tv, opts),
        "6" => {
            cmds::update(tv, opts);
            if confirm("put the Play Store pages on the TV screen?") {
                let mut opts = opts.clone();
                opts.open = true;
                cmds::update(tv, &opts);
            }
        }
        "7" => {
            warn("Full service includes the recompile, so allow up to half an hour.");
            if confirm("run the full service?") {
                cmds::full(tv, opts);
            }
        }
        "8" => {
            if confirm("restart the TV now?") {
                let mut opts = opts.clone();
                opts.yes = true;
                if let Err(message) = cmds::reboot(tv, &opts) {
                    bad(&message);
                }
            }
        }
        "9" => {
            cmds::restore(tv, opts);
            if confirm("undo the changes listed above?") {
                let mut opts = opts.clone();
                opts.yes = true;
                cmds::restore(tv, &opts);
            }
        }
        other => warn(&format!("'{}' is not one of the options", other)),
    }
}

/// Status line used by `tvdoctor status`, and worth seeing before anything else.
pub fn status(opts: &Opts) {
    hdr("Status");
    let cfg = config::load();
    match cfg.ip {
        Some(ip) if !ip.is_empty() => {
            kv("Remembered TV", &ip);
            let awake = net::port_open(&ip, net::CAST_PORT, 2500);
            kv("TV awake", if awake { "yes" } else { "no, or asleep" });
            let adb_open = net::port_open(&ip, ADB_PORT, 2000);
            kv("ADB reachable", if adb_open { "yes" } else { "no" });
            if adb_open {
                match adb::get_tv(Some(&ip), opts.dry_run, false) {
                    Ok(Some(tv)) => ok(&format!(
                        "connected to {} {}",
                        tv.getprop("ro.product.manufacturer"),
                        tv.getprop("ro.product.model")
                    )),
                    _ => warn("ADB port is open but the connection was refused"),
                }
            }
        }
        _ => info("no TV remembered yet. Run:  tvdoctor discover"),
    }
    println!();
}
