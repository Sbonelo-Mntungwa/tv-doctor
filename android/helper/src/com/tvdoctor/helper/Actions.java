package com.tvdoctor.helper;

import android.app.ActivityManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.net.Uri;
import android.provider.Settings;

import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** The things TV Nurse does when TV Doctor asks. */
final class Actions {
    private Actions() {}

    /** Packages whose background processes are left alone. */
    private static Set<String> spared(Context context) {
        Set<String> keep = new HashSet<>();
        keep.add(context.getPackageName());
        keep.add("android");
        keep.add("com.android.systemui");
        keep.add("com.android.phone");
        keep.add("com.google.android.gms");
        keep.add("com.google.android.gsf");
        keep.add("com.google.android.tvlauncher");
        keep.add("com.google.android.leanbacklauncher");
        // The launcher in use, whatever it is.
        ResolveInfo home = context.getPackageManager().resolveActivity(
                new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME), 0);
        if (home != null && home.activityInfo != null) keep.add(home.activityInfo.packageName);
        // The keyboard in use.
        String ime = Settings.Secure.getString(
                context.getContentResolver(), Settings.Secure.DEFAULT_INPUT_METHOD);
        if (ime != null && ime.contains("/")) keep.add(ime.substring(0, ime.indexOf('/')));
        return keep;
    }

    /**
     * Ask Android to end the background processes of every other app. Apps on
     * screen are not touched, and anything ended starts again when opened.
     */
    static String freeMemory(Context context) {
        ActivityManager am = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        long before = Info.memory(context).availMem;
        Set<String> keep = spared(context);
        int asked = 0;
        List<ApplicationInfo> apps = context.getPackageManager().getInstalledApplications(0);
        for (ApplicationInfo ai : apps) {
            if (keep.contains(ai.packageName) || ai.packageName.startsWith("com.android.providers.")) {
                continue;
            }
            am.killBackgroundProcesses(ai.packageName);
            asked++;
        }
        try {
            Thread.sleep(2500);
        } catch (InterruptedException ignored) {}
        long after = Info.memory(context).availMem;
        return new Info.Json().begin()
                .field("ok", true)
                .field("before", before)
                .field("after", after)
                .field("freed", Math.max(0, after - before))
                .field("asked", asked)
                .end().toString();
    }

    private static boolean installed(Context context, String pkg) {
        try {
            context.getPackageManager().getPackageInfo(pkg, 0);
            return true;
        } catch (PackageManager.NameNotFoundException e) {
            return false;
        }
    }

    private static String start(Context context, Intent intent, String what) {
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (intent.resolveActivity(context.getPackageManager()) == null) {
            return error("this TV has no screen for " + what);
        }
        try {
            context.startActivity(intent);
            return "{\"ok\":true}";
        } catch (Exception e) {
            return error("the TV would not open " + what + ": " + e.getMessage());
        }
    }

    /** The app's own page in Settings: Force stop, Clear cache, Disable. */
    static String openAppPage(Context context, String pkg) {
        if (!installed(context, pkg)) return error("no app called " + pkg);
        Intent intent = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.fromParts("package", pkg, null));
        return start(context, intent, "that app's settings");
    }

    static String launch(Context context, String pkg) {
        if (!installed(context, pkg)) return error("no app called " + pkg);
        PackageManager pm = context.getPackageManager();
        Intent intent = pm.getLeanbackLaunchIntentForPackage(pkg);
        if (intent == null) intent = pm.getLaunchIntentForPackage(pkg);
        if (intent == null) return error(pkg + " has nothing to open");
        return start(context, intent, pkg);
    }

    /** A Settings screen TV Doctor wants on the TV, from a short fixed list. */
    static String openSettings(Context context, String page) {
        Intent intent;
        switch (page) {
            case "developer":
                intent = new Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS);
                break;
            case "storage":
                intent = new Intent(Settings.ACTION_INTERNAL_STORAGE_SETTINGS);
                break;
            case "apps":
                intent = new Intent(Settings.ACTION_APPLICATION_SETTINGS);
                break;
            case "update":
                intent = new Intent("android.settings.SYSTEM_UPDATE_SETTINGS");
                break;
            case "about":
                // Android TV keeps Restart on this page.
                intent = new Intent(Settings.ACTION_DEVICE_INFO_SETTINGS);
                break;
            case "security":
                intent = new Intent(Settings.ACTION_SECURITY_SETTINGS);
                break;
            case "main":
                intent = new Intent(Settings.ACTION_SETTINGS);
                break;
            default:
                return error("no settings page called " + page);
        }
        return start(context, intent, "the " + page + " settings");
    }

    /** An app's page in the Play Store, where Update lives. */
    static String openStore(Context context, String pkg) {
        Intent intent = new Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=" + pkg));
        return start(context, intent, "the Play Store");
    }

    /** Settings screens TV Nurse itself needs the person to visit. */
    static Intent settingsFor(String which) {
        switch (which) {
            case "usage":
                return new Intent(Settings.ACTION_USAGE_ACCESS_SETTINGS);
            case "energy":
                return new Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS);
            default:
                return new Intent(Settings.ACTION_SETTINGS);
        }
    }

    static String error(String message) {
        return new Info.Json().begin().field("ok", false).field("error", message).end().toString();
    }
}
