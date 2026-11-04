//! What matters on this TV, what must never be touched, and what is safe to
//! switch off.

/// The apps the TV is actually used for: label, name fragments to match against
/// installed packages, then the known package ids in order of preference.
pub const KEY_APPS: &[(&str, &[&str], &[&str])] = &[
    (
        "DStv",
        &["dstv", "multichoice"],
        &[
            "com.dstv.now.android.views.leanback",
            "com.dstvdm.dstvnow",
            "com.multichoice.dstv",
        ],
    ),
    (
        "YouTube",
        &["youtube"],
        &[
            "com.google.android.youtube.tv",
            "com.google.android.youtube.tvunplugged",
            "com.google.android.youtube",
        ],
    ),
    (
        "Netflix",
        &["netflix"],
        &["com.netflix.ninja", "com.netflix.mediaclient"],
    ),
    (
        "Spotify",
        &["spotify"],
        &["com.spotify.tv.android", "com.spotify.music"],
    ),
];

/// Never disable, uninstall or wipe anything matching these. Matched as
/// substrings, so the net is deliberately wide: breaking one of these can cost
/// you the picture, the sound, the remote or the network.
pub const PROTECTED_FRAGMENTS: &[&str] = &[
    "com.android.",
    "com.google.android.gms",
    "com.google.android.gsf",
    "com.google.android.tvlauncher",
    "com.google.android.leanbacklauncher",
    "com.google.android.tungsten",
    "com.google.android.katniss",
    "com.google.android.webview",
    "com.google.android.tv.remote",
    "com.google.android.apps.mediashell",
    "com.google.android.tv.provision",
    "com.google.android.tv.frameworkpackagestubs",
    "systemui",
    "inputmethod",
    "keyboard",
    "provider",
    "settings",
    "packageinstaller",
    "permission",
    "framework",
    "overlay",
    "bluetooth",
    "wifi",
    "nfc",
    "telephony",
    "drm",
    "widevine",
    "hdmi",
    "tvinput",
    "tv.input",
    "launcher",
    "setupwizard",
    "com.hisense.",
    "com.mediatek.",
    "com.realtek.",
    "com.amlogic.",
    "com.mstar.",
    "com.vewd.",
    "com.opera.",
    "com.droidlogic",
    "com.tcl.",
];

/// Preinstalled extras that are safe to switch off on an Android TV. Hand
/// checked, and every one is reversible with `tvdoctor restore`.
pub const KNOWN_BLOAT: &[(&str, &str)] = &[
    ("com.google.android.play.games", "Google Play Games"),
    ("com.google.android.videos", "Google Play Movies / Google TV store"),
    ("com.google.android.music", "Google Play Music (dead service)"),
    ("com.google.android.feedback", "Google feedback reporter"),
    (
        "com.google.android.printservice.recommendation",
        "Print service suggestions",
    ),
    ("com.android.bips", "Built-in print spooler"),
    ("com.android.wallpaperbackup", "Wallpaper backup"),
    ("com.google.android.apps.photos", "Google Photos"),
    ("com.google.android.apps.docs", "Google Drive"),
    ("com.google.android.keep", "Google Keep"),
    ("com.google.android.calendar", "Google Calendar"),
    ("com.google.android.gm", "Gmail"),
    ("com.google.android.apps.magazines", "Google News"),
    ("com.google.android.talk", "Hangouts"),
    ("com.facebook.katana", "Facebook"),
    ("com.facebook.system", "Facebook installer service"),
    ("com.facebook.appmanager", "Facebook app manager"),
    ("com.facebook.services", "Facebook services"),
];

pub const ANIMATION_KEYS: &[&str] = &[
    "window_animation_scale",
    "transition_animation_scale",
    "animator_duration_scale",
];

pub fn is_known_bloat(pkg: &str) -> bool {
    KNOWN_BLOAT.iter().any(|(p, _)| *p == pkg)
}

/// True when the package must be left alone.
pub fn is_protected(pkg: &str, extra_keep: &[String]) -> bool {
    let low = pkg.to_lowercase();
    if extra_keep.iter().any(|k| k.to_lowercase() == low) {
        return true;
    }
    // KNOWN_BLOAT is a vetted allow list, so it wins over the broad fragment
    // guard below. Without this, com.android.bips and com.android.wallpaperbackup
    // would be listed as removable and then refused.
    if is_known_bloat(pkg) {
        return false;
    }
    PROTECTED_FRAGMENTS.iter().any(|frag| low.contains(frag))
}

pub const ENABLE_ADB_HELP: &str = "\
Network ADB debugging is not switched on yet. On the TV, with the remote:

  1. Settings  ->  Device Preferences  ->  About
  2. Scroll to \"Build\" and press OK on it 7 times.
     You will see \"You are now a developer\".
  3. Go back one screen  ->  Developer options
  4. Turn ON  \"ADB debugging\"   (some sets call it \"USB debugging\")
  5. Turn ON  \"Network debugging\" if your set lists it separately.
     On older Hisense sets there is only \"ADB debugging\" and that is enough.

Then run:  tvdoctor connect

The first connection makes the TV show \"Allow debugging from this computer?\".
Pick \"Always allow from this computer\" and press OK.

On Android 11 and newer TVs, Developer options shows \"Wireless debugging\" with
a pairing code instead. In that case use:
  tvdoctor pair --host <ip>:<pair-port> --code <6-digit-code>

If Developer options lists only \"USB debugging\", with nothing about network or
wireless debugging, then this build does not expose ADB over the network and no
toggle will change that. A TV's USB ports are host ports, so a cable to a
computer does not work either. Run:

  tvdoctor guide

which walks you through the same cleanup using the remote.";
