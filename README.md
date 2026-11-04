# TV Doctor

A Rust tool and macOS app that speeds up an old Android TV over your home network.

## What is TV Doctor?

TV Doctor finds your Android TV on the network and does the things that make it
fast again, from a window on your Mac or from the terminal:

- **Health report** - storage, free memory, running apps, installed apps
- **Clean** - drops app caches and stops background apps, without touching logins
- **Optimise** - recompiles installed apps and switches off menu animations
- **Bloat** - switches off preinstalled apps you never use, with full undo
- **Launch and stop apps** - YouTube, Netflix and Spotify, with no ADB needed

```
┌─────────────────────────────────────────────┐
│                 TV DOCTOR                   │
├─────────────────────────────────────────────┤
│  ✅ Finds the TV on your network            │
│  ✅ Window on your Mac, or the command line │
│  ✅ DStv, YouTube, Netflix, Spotify safe    │
│  ✅ Every change journaled and undoable     │
│  ✅ No external crates, builds offline      │
└─────────────────────────────────────────────┘
```

## Requirements

### Hardware
- An Android TV
- A Mac on the same network as the TV

### Software (on your Mac)
- [Rust](https://rustup.rs/)
- [Android platform tools](https://developer.android.com/tools/releases/platform-tools) (`adb`)

```bash
# Install on macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install --cask android-platform-tools
```

## Quick Start

```bash
# 1. Clone this repo
git clone https://github.com/Sbonelo-Mntungwa/tv-doctor.git
cd tv-doctor

# 2. Build the tool and the app
./scripts/build.sh

# 3. Open it
open "dist/TV Doctor.app"
```

The window opens in your browser at `http://127.0.0.1:8787`. Nothing leaves your
Mac. To keep the app, drag `dist/TV Doctor.app` to your Applications folder.

The first time, macOS asks whether TV Doctor may find devices on your local
network. Choose **Allow**.

## Your TV

TV Doctor searches your network for the TV and remembers its address in
`~/.config/tvdoctor/config.json`, outside the project, so there is nothing to
set up. No Wi-Fi details are needed: your Mac only has to be on the same
network as the TV.

If it picks the wrong TV or searches the wrong network, tell it directly:

```bash
./target/release/tvdoctor --ip 192.168.1.20 connect   # a specific TV, remembered from then on
./target/release/tvdoctor discover --subnet 192.168.1 # search a specific network
```

To protect extra apps from `bloat`, add them to the config file:

```json
{ "ip": "192.168.1.20", "keep": ["com.showmax.app"] }
```

### Find Your TV's IP

```bash
# Option 1: Let TV Doctor search the network
./target/release/tvdoctor discover

# Option 2: On the TV
# Settings > Network & Internet > your network > IP address

# Option 3: Check your router's list of connected devices
```

## Switching On ADB (for clean, optimise and bloat)

Launching apps and the report over Cast work straight away. The cleanup
commands need ADB over the network:

1. On the TV, open **Settings > Device Preferences > About**
2. Select **Build** seven times until it says you are a developer
3. Go back to **Developer options** and switch on **Network debugging** (or **ADB debugging**)
4. Run `./target/release/tvdoctor connect`
5. On the TV, choose **Always allow from this computer**

Some TVs only allow USB debugging. If yours does, run
`./target/release/tvdoctor guide` for the same cleanup done step by step with
the remote.

## Using It

```bash
tvdoctor report           # what is wrong
tvdoctor clean            # free space, drop caches, stop background apps
tvdoctor optimize         # recompile apps, switch off animations (10-30 min)
tvdoctor bloat            # list unused preinstalled apps
tvdoctor reboot --yes     # restart so it all takes effect

tvdoctor launch youtube   # start an app, no ADB needed
tvdoctor restore --yes    # undo everything TV Doctor changed
tvdoctor --help           # every command
```

Run these as `./target/release/tvdoctor` from the project folder. Add
`--dry-run` to any command to see what it would do without changing anything.

## Project Structure

```
tv-doctor/
├── README.md               # This file
├── Cargo.toml
├── scripts/
│   └── build.sh            # Build, then assemble dist/TV Doctor.app
├── macos/
│   ├── Info.plist          # The app's details for macOS
│   └── launch              # What runs when you open the app
└── src/
    ├── main.rs             # Commands and arguments
    ├── server.rs           # The app window (local web server)
    ├── app.html            # The window's page
    ├── cmds.rs             # What each command does
    ├── adb.rs              # Talking to the TV over ADB
    ├── cast.rs             # Launching apps over Cast
    ├── net.rs              # Finding the TV on the network
    ├── catalog.rs          # Protected apps and known bloat
    ├── config.rs           # Remembered TV and undo journal
    ├── menu.rs             # The terminal menu
    └── ui.rs               # Terminal output
```

## Troubleshooting

| Problem | Cause | Solution |
|---------|-------|----------|
| App will not open from Finder | Signature out of date | Run `./scripts/build.sh` again |
| App says the TV is off, terminal works | Local Network permission off | **System Settings > Privacy & Security > Local Network**, switch on TV Doctor |
| `no Android TV found` | TV in standby, or wrong network | Wake the TV, check both are on the same network, or use `discover --subnet` |
| `cannot reach ADB` | Network debugging off | See [Switching On ADB](#switching-on-adb-for-clean-optimise-and-bloat) |
| Wrong TV picked | More than one TV found | Run `tvdoctor --ip ADDRESS connect` |
| `could not listen on 127.0.0.1:8787` | Port in use | Run `tvdoctor app 9000` for another port |

## License

MIT License - See LICENSE file
