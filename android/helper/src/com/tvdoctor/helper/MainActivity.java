package com.tvdoctor.helper;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.NetworkInterface;
import java.util.Collections;

/**
 * The one screen: the code for TV Doctor, where to find the nurse, and the two
 * permissions worth granting, each with a button to its Settings page.
 */
public class MainActivity extends Activity {
    private static final String BG = "#081215";
    private static final String TEXT = "#DFEFF0";
    private static final String DIM = "#87A2A9";
    private static final String ACCENT = "#4FD1C5";
    private static final String OK = "#45C884";
    private static final String WARN = "#DFB155";

    private TextView code;
    private TextView expiry;
    private TextView address;
    private final Handler ticker = new Handler(Looper.getMainLooper());
    /** Keeps the code and its countdown current while the screen is open. */
    private final Runnable tick = new Runnable() {
        @Override public void run() {
            refresh();
            ticker.postDelayed(this, 15000);
        }
    };
    private TextView usageLine;
    private TextView energyLine;
    private TextView note;

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        NurseService.start(this);
        Server.ensureStarted(this);

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.HORIZONTAL);
        root.setGravity(Gravity.CENTER);
        root.setPadding(dp(64), dp(40), dp(64), dp(40));
        root.setBackgroundColor(Color.parseColor(BG));

        // Left: the mark and the name.
        LinearLayout left = new LinearLayout(this);
        left.setOrientation(LinearLayout.VERTICAL);
        left.setGravity(Gravity.CENTER);
        ImageView mark = new ImageView(this);
        mark.setImageResource(R.drawable.mark);
        left.addView(mark, new LinearLayout.LayoutParams(dp(150), dp(150)));
        TextView name = text("TV NURSE", 34, ACCENT);
        name.setLetterSpacing(0.12f);
        name.getPaint().setFakeBoldText(true);
        name.setPadding(0, dp(10), 0, 0);
        left.addView(name);
        left.addView(text("on call for TV Doctor · " + Info.VERSION, 13, DIM));
        root.addView(left, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 2));

        // Right: the code, the address and the permissions.
        LinearLayout right = new LinearLayout(this);
        right.setOrientation(LinearLayout.VERTICAL);
        right.setPadding(dp(48), 0, 0, 0);

        right.addView(label("Code for TV Doctor"));
        code = text("", 46, TEXT);
        code.setGravity(Gravity.START);
        code.setLetterSpacing(0.08f);
        code.getPaint().setFakeBoldText(true);
        right.addView(code);
        expiry = text("", 14, WARN);
        expiry.setGravity(Gravity.START);
        right.addView(expiry);
        address = text("", 15, DIM);
        address.setGravity(Gravity.START);
        right.addView(address);

        TextView perms = label("Permissions");
        perms.setPadding(0, dp(26), 0, dp(4));
        right.addView(perms);
        usageLine = line();
        right.addView(usageLine);
        energyLine = line();
        right.addView(energyLine);

        LinearLayout buttons = new LinearLayout(this);
        buttons.setOrientation(LinearLayout.HORIZONTAL);
        buttons.setPadding(0, dp(16), 0, 0);
        Button usage = button("Usage access", "usage");
        Button energy = button("Energy optimisation", "energy");
        buttons.addView(usage);
        LinearLayout.LayoutParams gap = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        gap.leftMargin = dp(14);
        buttons.addView(energy, gap);
        right.addView(buttons);

        note = text("", 13, DIM);
        note.setGravity(Gravity.START);
        note.setPadding(0, dp(14), 0, 0);
        right.addView(note);

        root.addView(right, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 3));
        setContentView(root);
        usage.requestFocus();
    }

    @Override
    protected void onResume() {
        super.onResume();
        ticker.removeCallbacks(tick);
        ticker.post(tick);
    }

    @Override
    protected void onPause() {
        ticker.removeCallbacks(tick);
        super.onPause();
    }

    private void refresh() {
        code.setText(Pairing.spaced(this));
        long minutes = (Pairing.remainingMs(this) + 59999) / 60000;
        expiry.setText(Pairing.lockedOut()
                ? "Too many wrong codes. Locked for a minute"
                : "Changes in " + minutes + (minutes == 1 ? " minute" : " minutes")
                        + ". TV Doctor will ask for the new one");
        address.setText("TV Doctor finds me at " + ipAddress() + ":" + Server.PORT);
        boolean usage = Info.usageAccess(this);
        boolean energy = Info.energyExempt(this);
        mark(usageLine, usage, "Usage access",
                "app sizes, caches and when each app was last used");
        mark(energyLine, energy, "Energy optimisation off",
                "so Android does not stop me in the background");
        if (usage && energy) {
            note.setText("All set. You can close this screen; I keep running in the background.");
        } else {
            note.setText("Find TV Nurse on each page and allow it. Both are also under "
                    + "Settings > Apps > Special app access.");
        }
    }

    private void mark(TextView view, boolean on, String title, String why) {
        view.setText((on ? "✓  " : "✗  ") + title + "  ·  " + why);
        view.setTextColor(Color.parseColor(on ? OK : WARN));
    }

    private void open(String which) {
        Intent intent = Actions.settingsFor(which);
        if (intent.resolveActivity(getPackageManager()) == null) {
            note.setText("This TV has no shortcut for that page. Go to Settings > Apps > "
                    + "Special app access and find TV Nurse there.");
            return;
        }
        try {
            startActivity(intent);
        } catch (Exception e) {
            note.setText("This TV would not open that page. Go to Settings > Apps > "
                    + "Special app access and find TV Nurse there.");
        }
    }

    private Button button(String title, final String which) {
        Button b = new Button(this);
        b.setText(title);
        b.setAllCaps(false);
        b.setTextSize(16);
        b.setTextColor(Color.parseColor(BG));
        final GradientDrawable face = new GradientDrawable();
        face.setColor(Color.parseColor(ACCENT));
        face.setCornerRadius(dp(10));
        b.setBackground(face);
        b.setPadding(dp(22), dp(10), dp(22), dp(10));
        b.setOnFocusChangeListener(new View.OnFocusChangeListener() {
            @Override public void onFocusChange(View v, boolean focused) {
                face.setStroke(focused ? dp(3) : 0, Color.WHITE);
            }
        });
        b.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { open(which); }
        });
        return b;
    }

    private TextView label(String value) {
        TextView view = text(value.toUpperCase(), 12, ACCENT);
        view.setGravity(Gravity.START);
        view.setLetterSpacing(0.14f);
        return view;
    }

    private TextView line() {
        TextView view = text("", 15, TEXT);
        view.setGravity(Gravity.START);
        view.setPadding(0, dp(3), 0, dp(3));
        return view;
    }

    private String ipAddress() {
        try {
            for (NetworkInterface nif : Collections.list(NetworkInterface.getNetworkInterfaces())) {
                if (!nif.isUp() || nif.isLoopback()) continue;
                for (InetAddress a : Collections.list(nif.getInetAddresses())) {
                    if (a instanceof Inet4Address) return a.getHostAddress();
                }
            }
        } catch (Exception ignored) {}
        return "this TV's address";
    }

    private TextView text(String value, int sp, String color) {
        TextView view = new TextView(this);
        view.setText(value);
        view.setTextSize(sp);
        view.setTextColor(Color.parseColor(color));
        view.setGravity(Gravity.CENTER);
        return view;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
