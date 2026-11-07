package com.tvdoctor.helper;

import android.content.Context;
import android.content.SharedPreferences;

import java.security.MessageDigest;
import java.security.SecureRandom;

/**
 * The code that lets TV Doctor give orders.
 *
 * It is shown on the TV Nurse screen and typed into TV Doctor, and it lasts
 * one hour. After that a new code is made and the old one stops working, so a
 * code that leaks is only good for the rest of its hour.
 *
 * Six digits is a million codes, which a machine on the network could try in
 * well under an hour. So after five wrong codes in a row, every code, even the
 * right one, is refused for a minute. That caps guessing at a few hundred tries
 * an hour.
 */
final class Pairing {
    static final long LIFETIME_MS = 60L * 60 * 1000;
    private static final int MAX_FAILURES = 5;
    private static final long LOCKOUT_MS = 60L * 1000;

    private static final String PREFS = "nurse";
    private static final String KEY_CODE = "code";
    private static final String KEY_ISSUED = "issued";

    private static int failures = 0;
    private static long lockedUntil = 0;

    private Pairing() {}

    /** The current code, making a new one when the old one has run out. */
    static synchronized String code(Context context) {
        SharedPreferences prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        String code = prefs.getString(KEY_CODE, null);
        long issued = prefs.getLong(KEY_ISSUED, 0);
        long now = System.currentTimeMillis();
        // A clock that went backwards also counts as expired.
        boolean expired = issued <= 0 || issued > now || now - issued >= LIFETIME_MS;
        if (code == null || code.length() != 6 || expired) {
            code = String.format("%06d", new SecureRandom().nextInt(1000000));
            prefs.edit().putString(KEY_CODE, code).putLong(KEY_ISSUED, now).apply();
            failures = 0;
        }
        return code;
    }

    /** Milliseconds until the current code is replaced. */
    static synchronized long remainingMs(Context context) {
        code(context);
        long issued = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getLong(KEY_ISSUED, 0);
        return Math.max(0, issued + LIFETIME_MS - System.currentTimeMillis());
    }

    /**
     * Whether `offered` is the current code. No code at all is not a guess and
     * is not counted. Compared in constant time.
     */
    static synchronized boolean matches(Context context, String offered) {
        if (offered == null || offered.trim().isEmpty()) return false;
        long now = System.currentTimeMillis();
        if (now < lockedUntil) return false;
        boolean right = MessageDigest.isEqual(code(context).getBytes(), offered.trim().getBytes());
        if (right) {
            failures = 0;
        } else if (++failures >= MAX_FAILURES) {
            failures = 0;
            lockedUntil = now + LOCKOUT_MS;
        }
        return right;
    }

    static synchronized boolean lockedOut() {
        return System.currentTimeMillis() < lockedUntil;
    }

    static String spaced(Context context) {
        String code = code(context);
        return code.substring(0, 3) + " " + code.substring(3);
    }
}
