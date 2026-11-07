//! tvdoctor - find, inspect, clean and speed up an Android TV over the network.
//!
//! Works with any Android TV on the local network. The TV is found with
//! `discover` and remembered in ~/.config/tvdoctor/config.json.

mod adb;
mod cast;
mod catalog;
mod cmds;
mod config;
mod menu;
mod net;
mod nurse;
mod proto;
mod remote;
mod server;
mod ui;

use cmds::Opts;
use ui::bad;

const USAGE: &str = "\
tvdoctor - find, inspect, clean and speed up an Android TV over the network

usage: tvdoctor [--ip ADDRESS] [-n|--dry-run] <command> [options]
       tvdoctor                      with no command, opens the app window

commands:
  app                  open the TV Doctor window on this laptop (recommended)
  menu                 the interactive menu in this terminal
  status               is the TV awake, is ADB reachable
  guide                clean the TV from the remote, when ADB is not available
  remote-check         what the TV reports over Cast, without ADB
  routes               which apps can be launched from here, and how
  launch <app>         start an app on the TV (DIAL or Cast), without ADB
  stop <app>           stop an app on the TV over DIAL, freeing its memory
  force-stop [app]     take the screen back and return the TV to its home
                       screen, for apps with no receiver of their own
  discover             scan the network for Android TVs
  connect              connect adb to the TV
  pair                 Android 11+ pairing: --host IP:PORT --code NNNNNN
  report               health report: storage, RAM, apps, settings
  clean                free space and drop caches (safe)
                         --deep PKGS --yes   also wipe those apps' data
  optimize             recompile apps, switch off animations
                         --aggressive        compile mode 'speed'
                         --keep-animations   leave the animations alone
  tune                 read or set performance settings
                         --animations on|off
                         --background-limit N
  bloat                list installed apps, switch off unused ones
                         --auto --yes        switch off known extras
                         --disable a,b --yes switch off specific ones
  restore --yes        undo everything tvdoctor changed
  update               app and system update status
                         --open              put update screens on the TV
  apps                 status of DStv, YouTube, Netflix, Spotify
                         --restart           force-stop them so they start clean
                         --launch NAME       open one on the TV
  full                 report, clean, optimise, app check
  reboot --yes         restart the TV
  shell '<cmd>'        run a raw adb shell command on the TV
  enable-adb           print the steps to switch ADB on
  remote pair          pair this Mac as the TV's remote (no ADB needed)
  remote status        power, volume and the app in front
  remote press KEYS    press keys: home back up down left right ok
                       settings volup voldown mute power, digits (1969),
                       hold:back:15, wait:2. remote keys lists them all
  remote link URL      open a link on the TV
  (on a TV without ADB, report, clean, optimize, tune, bloat, restore,
   update, apps, full and reboot work through TV Nurse instead)
  nurse status         TV Nurse, the companion app on the TV: storage, memory
  nurse pair CODE      pair with TV Nurse using the code on its screen
  nurse report         per-app sizes, caches, and apps nobody uses
  nurse free           close background apps to free memory
  nurse clean [PKGS]   clear the biggest caches, with you at the remote
  nurse open PKG       show an app's settings page on the TV
  nurse launch PKG     open an app on the TV
  nurse store PKG      show an app's Play Store page on the TV

in the app window, type any of these into the Monitor without 'tvdoctor',
for example:  nurse free   or   remote press home   (shell is terminal only)

typical first run:
  tvdoctor discover
  tvdoctor connect
  tvdoctor report
  tvdoctor clean
  tvdoctor optimize
  tvdoctor reboot --yes";

/// Flags that take a value.
const VALUE_FLAGS: &[&str] = &[
    "--ip",
    "--deep",
    "--animations",
    "--background-limit",
    "--disable",
    "--launch",
    "--subnet",
    "--host",
    "--code",
];

fn parse_args(argv: &[String]) -> Result<(String, Opts), String> {
    let mut opts = Opts::default();
    let mut command: Option<String> = None;
    let mut index = 0;

    while index < argv.len() {
        let arg = argv[index].clone();
        index += 1;

        if arg == "-h" || arg == "--help" {
            println!("{}", USAGE);
            std::process::exit(0);
        }
        if arg == "-n" || arg == "--dry-run" {
            opts.dry_run = true;
            continue;
        }

        if VALUE_FLAGS.contains(&arg.as_str()) {
            let value = argv
                .get(index)
                .cloned()
                .ok_or(format!("{} needs a value", arg))?;
            index += 1;
            match arg.as_str() {
                "--ip" => opts.ip = Some(value),
                "--deep" => opts.deep = Some(value),
                "--animations" => {
                    if value != "on" && value != "off" {
                        return Err("--animations takes 'on' or 'off'".to_string());
                    }
                    opts.animations = Some(value);
                }
                "--background-limit" => {
                    opts.background_limit = Some(
                        value
                            .parse::<i64>()
                            .map_err(|_| "--background-limit takes a number")?,
                    )
                }
                "--disable" => opts.disable = Some(value),
                "--launch" => opts.launch = Some(value),
                "--subnet" => opts.subnet = Some(value),
                "--host" => opts.host = Some(value),
                "--code" => opts.code = Some(value),
                _ => unreachable!(),
            }
            continue;
        }

        match arg.as_str() {
            "--yes" | "-y" => opts.yes = true,
            "--aggressive" => opts.aggressive = true,
            "--keep-animations" => opts.keep_animations = true,
            "--auto" => opts.auto = true,
            "--open" => opts.open = true,
            "--restart" => opts.restart = true,
            "--no-open" => opts.no_open = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {}", other))
            }
            other => {
                if command.is_none() {
                    command = Some(other.to_string());
                } else {
                    opts.rest.push(other.to_string());
                }
            }
        }
    }

    Ok((command.unwrap_or_else(|| "app".to_string()), opts))
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (command, opts) = parse_args(&argv)?;
    config::set_dry_run(opts.dry_run);

    // Commands that do not need a connected TV.
    match command.as_str() {
        "app" | "gui" => {
            let port = opts
                .rest
                .first()
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(8787);
            return server::serve(&opts, port, !opts.no_open);
        }
        "menu" => {
            menu::run(&opts);
            return Ok(());
        }
        "status" => {
            menu::status(&opts);
            return Ok(());
        }
        "guide" => {
            cmds::guide();
            return Ok(());
        }
        "remote-check" | "check" => return cmds::remote_check(&opts),
        "routes" => return cmds::routes(&opts),
        "launch" => return cmds::launch_app(&opts),
        "stop" => return cmds::stop_app(&opts),
        "force-stop" | "home" => return cmds::force_stop(&opts),
        "discover" => {
            cmds::discover(&opts);
            return Ok(());
        }
        "connect" => return cmds::connect(&opts),
        "pair" => return cmds::pair(&opts),
        "remote" => return cmds::remote(&opts),
        "nurse" | "helper" => return nurse::command(&opts),
        "enable-adb" => {
            println!();
            println!("{}", catalog::ENABLE_ADB_HELP);
            println!();
            return Ok(());
        }
        _ => {}
    }

    const TREATMENTS: &[&str] = &[
        "report", "clean", "optimize", "optimise", "tune", "bloat", "restore", "update", "apps",
        "full", "reboot", "shell",
    ];
    if !TREATMENTS.contains(&command.as_str()) {
        return Err(format!("unknown command '{}'. Run tvdoctor --help", command));
    }

    // ADB when the TV allows it, TV Nurse when it does not.
    let tv = match adb::get_tv(opts.ip.as_deref(), opts.dry_run, false)? {
        Some(tv) => tv,
        None => {
            let ip = cmds::remembered_ip(&opts)?;
            if let Some(nurse) = nurse::Nurse::reach(&ip, 5000) {
                return nurse::run(&command, &nurse, &opts);
            }
            // TV Nurse is there but wants its new code: say that, not "enable ADB".
            if let Err(message) = nurse::need(&ip) {
                if !message.contains("not answering") {
                    return Err(message);
                }
            }
            println!();
            println!("{}", catalog::ENABLE_ADB_HELP);
            println!();
            println!("  Or, on a TV that never allows ADB, install TV Nurse (see the README)");
            println!("  and pair it:  tvdoctor nurse pair CODE");
            println!();
            return Err("this needs ADB or TV Nurse, and neither is answering".to_string());
        }
    };

    match command.as_str() {
        "report" => cmds::report(&tv),
        "clean" => cmds::clean(&tv, &opts),
        "optimize" | "optimise" => cmds::optimize(&tv, &opts),
        "tune" => cmds::tune(&tv, &opts),
        "bloat" => cmds::bloat(&tv, &opts),
        "restore" => cmds::restore(&tv, &opts),
        "update" => cmds::update(&tv, &opts),
        "apps" => cmds::apps(&tv, &opts),
        "full" => cmds::full(&tv, &opts),
        "reboot" => cmds::reboot(&tv, &opts)?,
        "shell" => cmds::shell(&tv, &opts),
        other => {
            return Err(format!(
                "unknown command '{}'. Run tvdoctor --help",
                other
            ))
        }
    }
    Ok(())
}

fn main() {
    if let Err(message) = run() {
        bad(&message);
        eprintln!("{} {}", ui::c("31", "error:"), message);
        std::process::exit(1);
    }
}
