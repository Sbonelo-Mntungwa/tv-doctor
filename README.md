# TV Doctor

A Rust command line tool and macOS app that reaches an Android TV over the home
network and does the things that actually make an old Android TV fast again:
frees storage, drops every app cache, stops background apps, recompiles the
installed apps, switches off the menu animations, and switches off preinstalled
apps you never use.

DStv, YouTube, Netflix and Spotify are treated as protected. The tool refuses to
disable, uninstall or wipe them, and reports on them separately.

No external crates, so it builds offline in a few seconds.

## Run it

```sh
git clone https://github.com/Sbonelo-Mntungwa/tv-doctor.git
cd tv-doctor
cp .env.example .env        # optional: put your TV's address in it
./scripts/build.sh
open "TV Doctor.app"
```

`TV Doctor.app` is a normal macOS app. Double click it in Finder, or drag it to
your Applications folder or your Dock first. It opens the TV Doctor window in
your browser, served by the binary on `127.0.0.1:8787`. Nothing leaves the
laptop. Close it with the **Quit** button in the window.

Every button in the window runs the same `tvdoctor` command you could type
yourself and shows exactly what it printed, so the window and the command line
can never disagree.

While something is running, every button is disabled except **Stop** in the
monitor bar, which ends the running command and its children. Use it to get out
of a long job such as Optimise. Cancelling a force stop part way through can
leave the helper app on screen, so press **Back to the home screen** afterwards.

The command line is still there if you prefer it:

```sh
./target/release/tvdoctor report      # or any command below
./target/release/tvdoctor menu        # the terminal menu
./target/release/tvdoctor app 9000    # the window on another port
```

There are no external crates, so it builds offline in a few seconds. Cast needs
TLS, which goes through the `openssl` already on the machine rather than pulling
in a dependency.

### Always build with ./scripts/build.sh

`cargo build --release` alone is not enough for the app bundle. The bundle runs
its own copy of the binary from inside `TV Doctor.app/Contents/MacOS/`, and
replacing that copy invalidates the bundle's ad-hoc code signature, after which
**Finder refuses to launch the app and shows no error at all**. `scripts/build.sh`
builds, copies and re-signs in one go. The launcher re-signs too, so a stale
bundle repairs itself on the next launch.

The binary has to live inside the bundle for a second reason: macOS grants
Local Network access per app, and a binary executed from outside the bundle does
not inherit the bundle's identity, so it gets blocked silently. That is what
makes the TV look switched off when it is not.

### If the app cannot see the TV

Open **System Settings &rarr; Privacy &amp; Security &rarr; Local Network** and
switch on **TV Doctor**. The window shows a banner when it detects this. From a
terminal the same binary uses Terminal's permission instead, which is why the
command line can work while the app does not.

## Your TV and network

Nothing about a particular TV or network is in the source. Your own settings go
in a `.env` file, which is git-ignored:

```sh
cp .env.example .env
```

| Variable | What it is |
|---|---|
| `TVDOCTOR_IP` | The TV's address, e.g. `192.168.1.20`. Find it with `tvdoctor discover` |
| `TVDOCTOR_SUBNET` | The network `discover` sweeps, e.g. `192.168.1`. Only needed if it guesses wrong, say on a VPN |
| `TVDOCTOR_KEEP` | Extra apps to protect, comma separated |
| `TVDOCTOR_PORT` | Port for the app window (default `8787`) |

All of them are optional. With none set, `discover` finds the TV and the address
is remembered in `~/.config/tvdoctor/config.json`. `TVDOCTOR_IP` takes the
place of that remembered address, and `--ip` on the command line beats both.

`.env` is read from the current directory, then the project folder (which is
how `TV Doctor.app` finds it when launched from Finder), then
`~/.config/tvdoctor/.env`. Variables already set in the environment win.

No Wi-Fi details are needed. The laptop only has to be on the same network as
the TV; TV Doctor never reads, stores or sends a Wi-Fi name or password.

## ADB on this TV

Written from testing on a Hisense HAT4KDTV; other sets may differ.

This set's Developer options has a **Debugging** section containing only
**USB debugging**, **Select mock location app** and **Select debug app**. There
is no "Network debugging" or "Wireless debugging" entry, and port 5555 refuses
connections. That means `adbd` on this build is bound to USB only, and no
toggle in the menus changes it. A TV's USB ports are host ports, so a cable to
the laptop is not a way round it either.

So on this TV the ADB commands cannot run. What does work, all tested against
the real set:

```sh
tvdoctor status         # is the TV awake, is ADB reachable
tvdoctor discover       # find it, identify it, check its ports
tvdoctor remote-check   # model, build, uptime, pending update
tvdoctor routes         # which apps can be launched, and how
tvdoctor launch youtube # start YouTube, Netflix or Spotify on the TV
tvdoctor stop youtube   # stop it again, freeing the memory it held
tvdoctor guide          # the whole cleanup, step by step on the remote
```

A restart was tried: the TV came back with an uptime of zero and port 5555 still
refused, so rebooting does not bind it either.

See [Starting apps from the laptop](#starting-apps-from-the-laptop) for how each
app is reached.

The Cast reboot endpoint (`POST /setup/reboot`) returns 403 on current firmware,
so tvdoctor cannot restart the TV, and this set has no Restart menu item either.
Unplug it at the wall for a minute instead.

`guide` is the one to use. It covers cache clearing, removing unused apps, the
animation scales and the background process limit (both of which you *do* have
in Developer options), app updates and a proper restart, in the order that
matters.

It is still worth trying once more after a full restart of the TV, since a few
Hisense builds only bind the network port at boot:

```sh
tvdoctor status         # says whether ADB has become reachable
```

If you ever do get ADB listening, the TV will show
**"Allow debugging from this computer?"** on the first connection. Choose
**Always allow from this computer**, and everything below becomes available.

## Starting apps from the laptop

Three of the four apps can be launched over the network with no ADB at all.
Which route each app takes was worked out by asking the TV, not by guessing:

| App | Route | Why |
|---|---|---|
| **YouTube** | Cast v2, port 8009, app id `233637DE` | Reports `APP_AVAILABLE` over Cast. Not registered with DIAL on this TV |
| **Netflix** | DIAL, port 8008 | Registered with DIAL and launches reliably. Reports `APP_UNAVAILABLE` over Cast, so DIAL is tried first |
| **Spotify** | Cast v2, port 8009, app id `CC32E753` | Reports `APP_AVAILABLE` over Cast |
| **DStv** | none | Ships no Cast receiver and no DIAL registration, so nothing on the network can start it. Use the remote |

`tvdoctor routes` checks all four live and tells you what it found.

Cast v2 is TLS carrying length-prefixed protobuf frames with JSON payloads, and
needs no pairing, which is why it is the route taken. The Android TV remote
protocol on port 6466 could launch DStv too, but it needs a pairing code read
off the TV screen and a far larger implementation, so it is not built.

`stop` is the one with an effect on speed: it ends the app's session and hands
back the memory it held, which is the same thing `clean` does over ADB, one app
at a time.

```sh
tvdoctor routes
tvdoctor launch youtube
tvdoctor stop youtube
tvdoctor force-stop           # end whatever is on screen, back to home
tvdoctor force-stop dstv      # for an app with no receiver of its own
```

### Force stopping an app with no receiver

DStv answers neither protocol, so there is no stop to send it. `force-stop dstv`
does the only thing available over the network: it takes the screen with an app
that does have a receiver, then ends that session, which lands the TV on its
home screen and drops DStv out of the foreground. Android then reclaims a
backgrounded app's memory under pressure.

That is not the same as `am force-stop` over ADB and the tool does not claim it
is. It frees the foreground and most of the working memory; it does not
guarantee the process is gone.

Launching Backdrop, the idle screen app, was tried first and does not displace a
running app, which is why the route is launch-then-stop.

## The short version

Once ADB is reachable:

```sh
tvdoctor report          # what is actually wrong
tvdoctor clean           # free space, drop caches, stop background apps
tvdoctor optimize        # recompile every app, switch off animations
tvdoctor bloat           # see what is installed, disable what you never use
tvdoctor reboot --yes    # restart so it all takes effect
```

Or let it do the lot:

```sh
tvdoctor full
tvdoctor reboot --yes
```

Expect `optimize` to take 10 to 30 minutes on a TV this age. That is the
recompile working, and it is the single biggest speed win. Leave the TV on
while it runs.

## Commands

| Command | What it does |
|---|---|
| *(none)* | Opens the interactive menu |
| `status` | Is the TV awake, is ADB reachable, what is it |
| `guide` | The whole cleanup written out as remote button presses. Needs no ADB |
| `remote-check` | Model, name, uptime, pending system update, wired or wi-fi, and which apps answer over DIAL. Needs no ADB |
| `app` | Opens the TV Doctor window. This is what `TV Doctor.app` runs, and what you get with no command at all |
| `routes` | Checks live which apps can be launched from here, and by which route |
| `launch <app>` | Starts an app on the TV, over DIAL or Cast as appropriate. Needs no ADB |
| `stop <app>` | Stops a running app, freeing the memory it held. Needs no ADB |
| `force-stop [app]` | Takes the screen back and returns the TV to its home screen. With no app named it ends whatever is running. Named with an app that has no receiver, it displaces it. Needs no ADB |
| `discover` | Scans the network for Android TVs, names them, says whether ADB is reachable |
| `connect` | Connects ADB to the TV and remembers its address |
| `pair --host IP:PORT --code NNNNNN` | For sets that use wireless debugging pairing codes |
| `report` | Storage, free RAM, process count, installed and disabled app counts, your four apps, animation settings. Flags anything out of range |
| `clean` | Clears log buffers, trims every app cache, stops background apps, clears the Play Store cache, runs a filesystem trim. Reports space recovered |
| `clean --deep PKGS --yes` | Also wipes app data for the packages you name. You will have to sign in again |
| `optimize` | Switches animations off and recompiles all apps (`speed-profile`, or `speed` with `--aggressive`) |
| `tune` | Read or set the animation scales and the background process limit |
| `bloat` | Lists preinstalled extras and your own installed apps. `--auto --yes` switches off the known extras; `--disable a,b --yes` switches off specific ones |
| `restore --yes` | Undoes everything tvdoctor disabled or changed |
| `update` | Shows the Android build, security patch and your four apps' versions. `--open` puts each app's Play Store page and the system update screen on the TV so you can press Update with the remote |
| `apps` | Detail on DStv, YouTube, Netflix and Spotify. `--restart` force-stops them so they start clean, `--launch dstv` opens one |
| `full` | report, then clean, then optimize, then the app check |
| `reboot --yes` | Restarts the TV |
| `shell '<cmd>'` | Runs a raw adb shell command, for anything the tool does not cover |
| `enable-adb` | Prints the steps above |

Global flags: `--ip ADDRESS` to override the remembered TV, and `-n` /
`--dry-run` to print every change without making any.

## Safety

- **Your four apps are protected.** DStv, YouTube, Netflix and Spotify are
  refused by name, as are system packages (anything matching the launcher, HDMI
  and TV input, keyboard, Play services, WebView, Hisense and chipset packages).
  Asking to disable one of these prints a refusal instead of doing it.
- **Everything reversible is journaled.** Each disable and setting change is
  appended to `~/.config/tvdoctor/journal.jsonl`, and `restore --yes` puts it
  all back.
- **`clean` never touches your logins.** It only clears caches, which Android
  regenerates. The one destructive option, `clean --deep`, needs both explicit
  package names and `--yes`.
- **`--dry-run` shows you everything first** and writes nothing to the journal.
- To protect extra apps, list them in `TVDOCTOR_KEEP` in `.env`, or in
  `~/.config/tvdoctor/config.json`:

  ```json
  { "ip": "192.168.1.20", "keep": ["com.showmax.app", "com.plexapp.android"] }
  ```

## Why the TV got slow, and what helps most

On a six year old Android TV with a small eMMC disk and 1 to 2 GB of RAM, slow
is almost always one of four things. `report` tells you which:

1. **Storage nearly full.** Under about 1.5 GB free, Android TV crawls and app
   updates silently fail. `clean` is the fix.
2. **No free RAM.** Apps get killed and relaunched constantly, so everything
   feels like a cold start. `bloat` is the fix: every disabled app is one less
   thing waking up in the background.
3. **Apps running interpreted rather than compiled.** Years of Play Store
   updates leave code poorly compiled. `optimize` recompiles the lot.
4. **Animations.** They were tuned for a faster chip than yours. `optimize`
   switches them off, which makes the menus feel instant.

The thing to avoid is a factory reset. It fixes 1 and 3, costs you every login,
and within a few months you are back where you started.

## Layout

| File | What is in it |
|---|---|
| `src/main.rs` | Argument parsing and command dispatch |
| `src/server.rs` | The app window: local HTTP server and the action whitelist |
| `src/app.html` | The window itself, embedded into the binary at build time |
| `src/cast.rs` | Cast v2: protobuf by hand, TLS through openssl |
| `src/menu.rs` | The terminal menu and `status` |
| `src/cmds.rs` | What each command does |
| `src/adb.rs` | Running adb, the `Tv` type, package queries |
| `src/net.rs` | Port probes, HTTP/1.1 client, Cast/DIAL queries, subnet sweep |
| `src/catalog.rs` | The four key apps, the protected list, the known extras |
| `src/config.rs` | Remembered address, keep list, undo journal |
| `src/env.rs` | Reads `.env` for your TV and network settings |
| `src/ui.rs` | Terminal output |
| `scripts/build.sh` | Build, copy into the app bundle, re-sign |
| `TV Doctor.app/` | The macOS app bundle: `Info.plist` and the Finder launcher |
| `.env.example` | Template for your own `.env` |

## Requirements

- `adb` (`brew install --cask android-platform-tools`)
- Rust, for building
- macOS, for the app bundle. The command line builds anywhere Rust does
- The laptop and the TV on the same network, and the TV switched on and awake.
  A TV in standby does not answer.

## License

MIT. See [LICENSE](LICENSE).
