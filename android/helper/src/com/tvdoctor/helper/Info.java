package com.tvdoctor.helper;

import android.app.ActivityManager;
import android.app.AppOpsManager;
import android.app.usage.StorageStats;
import android.app.usage.StorageStatsManager;
import android.app.usage.UsageStats;
import android.app.usage.UsageStatsManager;
import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Environment;
import android.os.PowerManager;
import android.os.Process;
import android.os.StatFs;
import android.os.SystemClock;
import android.os.storage.StorageManager;
import android.provider.Settings;

import java.util.List;
import java.util.Map;

/** What TV Nurse can tell TV Doctor about the TV, read without ADB. */
final class Info {
    static final String VERSION = "0.5.0";
    static final int VERSION_CODE = 7;

    /** How far back "last used" looks. */
    private static final long USAGE_WINDOW_MS = 180L * 24 * 60 * 60 * 1000;

    private Info() {}

    /** Usage access, granted under Settings > Apps > Special app access. */
    static boolean usageAccess(Context context) {
        AppOpsManager ops = (AppOpsManager) context.getSystemService(Context.APP_OPS_SERVICE);
        int mode = ops.checkOpNoThrow(
                AppOpsManager.OPSTR_GET_USAGE_STATS, Process.myUid(), context.getPackageName());
        return mode == AppOpsManager.MODE_ALLOWED;
    }

    /** Left alone by energy optimisation, so Android does not stop it. */
    static boolean energyExempt(Context context) {
        PowerManager pm = (PowerManager) context.getSystemService(Context.POWER_SERVICE);
        return pm != null && pm.isIgnoringBatteryOptimizations(context.getPackageName());
    }

    static ActivityManager.MemoryInfo memory(Context context) {
        ActivityManager am = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        ActivityManager.MemoryInfo memory = new ActivityManager.MemoryInfo();
        am.getMemoryInfo(memory);
        return memory;
    }

    static String status(Context context, boolean paired) {
        StatFs data = new StatFs(Environment.getDataDirectory().getPath());
        ActivityManager.MemoryInfo memory = memory(context);

        Json json = new Json().begin()
                .field("app", "TV Nurse")
                .field("version", VERSION)
                .field("versionCode", VERSION_CODE)
                .field("paired", paired)
                .field("codeLifetimeS", Pairing.LIFETIME_MS / 1000)
                .field("lockedOut", Pairing.lockedOut())
                .field("android", Build.VERSION.RELEASE)
                .field("sdk", Build.VERSION.SDK_INT)
                .field("maker", Build.MANUFACTURER)
                .field("model", Build.MODEL)
                .field("device", Build.DEVICE)
                .field("running", NurseService.running())
                .field("usageAccess", usageAccess(context))
                .field("energyExempt", energyExempt(context))
                .field("uptimeMs", SystemClock.elapsedRealtime());
        // Only the paired side learns when its code runs out.
        if (paired) json.field("codeExpiresInS", Pairing.remainingMs(context) / 1000);
        json.key("storage").begin()
                .field("total", data.getTotalBytes())
                .field("free", data.getAvailableBytes())
                .end();
        json.key("memory").begin()
                .field("total", memory.totalMem)
                .field("available", memory.availMem)
                .field("threshold", memory.threshold)
                .field("low", memory.lowMemory)
                .end();
        // Menu animation speeds, which any app may read. 1 is normal, 0 is off.
        json.key("animations").begin()
                .field("window", scale(context, Settings.Global.WINDOW_ANIMATION_SCALE))
                .field("transition", scale(context, Settings.Global.TRANSITION_ANIMATION_SCALE))
                .field("animator", scale(context, Settings.Global.ANIMATOR_DURATION_SCALE))
                .end();
        return json.end().toString();
    }

    /** An animation scale as text, so 0.5 survives the trip. */
    private static String scale(Context context, String key) {
        return String.valueOf(Settings.Global.getFloat(context.getContentResolver(), key, 1.0f));
    }

    /**
     * Every installed app. With usage access, also its size, data, cache and
     * when it was last on screen.
     */
    static String report(Context context) {
        PackageManager pm = context.getPackageManager();
        boolean usage = usageAccess(context);
        Map<String, UsageStats> used = null;
        StorageStatsManager storage = null;
        if (usage) {
            long now = System.currentTimeMillis();
            UsageStatsManager usm =
                    (UsageStatsManager) context.getSystemService(Context.USAGE_STATS_SERVICE);
            used = usm.queryAndAggregateUsageStats(now - USAGE_WINDOW_MS, now);
            storage = (StorageStatsManager) context.getSystemService(Context.STORAGE_STATS_SERVICE);
        }

        List<PackageInfo> packages = pm.getInstalledPackages(0);
        Json json = new Json().begin().field("usageAccess", usage);
        json.key("apps").beginArray();
        for (PackageInfo p : packages) {
            ApplicationInfo ai = p.applicationInfo;
            if (ai == null) continue;
            boolean launchable = pm.getLeanbackLaunchIntentForPackage(p.packageName) != null
                    || pm.getLaunchIntentForPackage(p.packageName) != null;
            json.begin()
                    .field("package", p.packageName)
                    .field("label", String.valueOf(ai.loadLabel(pm)))
                    .field("version", p.versionName == null ? "" : p.versionName)
                    .field("system", (ai.flags & ApplicationInfo.FLAG_SYSTEM) != 0)
                    .field("updatedSystem", (ai.flags & ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0)
                    .field("enabled", ai.enabled)
                    .field("launchable", launchable)
                    .field("installed", p.firstInstallTime)
                    .field("updated", p.lastUpdateTime);
            if (storage != null) {
                try {
                    StorageStats s = storage.queryStatsForPackage(
                            StorageManager.UUID_DEFAULT, p.packageName, Process.myUserHandle());
                    json.field("appBytes", s.getAppBytes())
                            .field("dataBytes", s.getDataBytes())
                            .field("cacheBytes", s.getCacheBytes());
                } catch (Exception ignored) {
                    // Some system packages refuse; leave the sizes out.
                }
            }
            if (used != null) {
                UsageStats u = used.get(p.packageName);
                json.field("lastUsed", u == null ? 0 : u.getLastTimeUsed())
                        .field("foregroundMs", u == null ? 0 : u.getTotalTimeInForeground());
            }
            json.end();
        }
        json.endArray();
        return json.end().toString();
    }

    /** A tiny JSON writer, enough for objects and arrays of them. */
    static final class Json {
        private final StringBuilder out = new StringBuilder();
        private boolean needComma = false;

        private void comma() {
            if (needComma) out.append(',');
            needComma = false;
        }

        Json begin() { comma(); out.append('{'); return this; }
        Json end() { out.append('}'); needComma = true; return this; }
        Json beginArray() { comma(); out.append('['); return this; }
        Json endArray() { out.append(']'); needComma = true; return this; }

        Json key(String name) {
            comma();
            quote(name);
            out.append(':');
            return this;
        }

        Json field(String name, String text) { key(name); quote(text); needComma = true; return this; }
        Json field(String name, long number) { key(name); out.append(number); needComma = true; return this; }
        Json field(String name, boolean flag) { key(name); out.append(flag); needComma = true; return this; }

        private void quote(String text) {
            out.append('"');
            for (int i = 0; i < text.length(); i++) {
                char c = text.charAt(i);
                switch (c) {
                    case '"': out.append("\\\""); break;
                    case '\\': out.append("\\\\"); break;
                    case '\n': out.append("\\n"); break;
                    case '\r': out.append("\\r"); break;
                    case '\t': out.append("\\t"); break;
                    default:
                        if (c < 0x20) out.append(String.format("\\u%04x", (int) c));
                        else out.append(c);
                }
            }
            out.append('"');
        }

        @Override public String toString() { return out.toString(); }
    }
}
