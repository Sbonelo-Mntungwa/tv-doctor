package com.tvdoctor.helper;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.IBinder;

/**
 * Keeps TV Nurse running in the background so TV Doctor can always reach it.
 * A foreground service is the one way an ordinary app is allowed to stay up
 * on Android 8 and later; it carries a quiet notification, which Android TV
 * does not put on screen.
 */
public class NurseService extends Service {
    private static final String CHANNEL = "nurse";
    private static volatile boolean running = false;

    static boolean running() {
        return running;
    }

    static void start(Context context) {
        Intent intent = new Intent(context, NurseService.class);
        try {
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent);
            else context.startService(intent);
        } catch (Exception ignored) {
            // Started from somewhere Android does not allow; the screen or the
            // next boot will start it instead.
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        Notification.Builder builder;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationManager nm = (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
            nm.createNotificationChannel(new NotificationChannel(
                    CHANNEL, "TV Nurse", NotificationManager.IMPORTANCE_MIN));
            builder = new Notification.Builder(this, CHANNEL);
        } else {
            builder = new Notification.Builder(this);
        }
        Notification note = builder
                .setSmallIcon(R.mipmap.ic_launcher)
                .setContentTitle("TV Nurse")
                .setContentText("On call for TV Doctor")
                .setOngoing(true)
                .build();
        startForeground(1, note);
        running = true;
        Server.ensureStarted(this);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Server.ensureStarted(this);
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        running = false;
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
