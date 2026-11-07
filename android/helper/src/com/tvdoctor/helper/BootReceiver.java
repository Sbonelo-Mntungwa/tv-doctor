package com.tvdoctor.helper;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Starts TV Nurse when the TV boots, and again after it is updated. */
public class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        NurseService.start(context);
    }
}
