package io.github.singularity098.chippy;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.graphics.drawable.Icon;
import android.os.Build;
import android.os.IBinder;

/**
 * Keeps Chippy playing with the screen off or another app in front: a
 * foreground service while music plays, showing the media notification
 * (title, artist, previous / play-pause / next). Paused, the notification
 * stays but the service leaves the foreground; stopped, both go.
 */
public class PlaybackService extends Service {
    private static final String CHANNEL = "playback";
    private static final int ID = 1;
    private static final String ACTION = "io.github.singularity098.chippy.ACTION";

    private static PlaybackService running;

    static void start(Context ctx) {
        if (running != null) {
            running.show();
            return;
        }
        try {
            ctx.startForegroundService(new Intent(ctx, PlaybackService.class));
        } catch (RuntimeException e) {
            // Not allowed from the background on newer Android: the music
            // still plays while the app is in front.
            android.util.Log.w("chippy", "cannot start the playback service", e);
        }
    }

    static void refresh(Context ctx) {
        if (running != null) {
            running.show();
        }
    }

    static void stop(Context ctx) {
        if (running != null) {
            running.stopForeground(STOP_FOREGROUND_REMOVE);
            running.stopSelf();
            running = null;
        }
        ctx.getSystemService(NotificationManager.class).cancel(ID);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        NotificationManager nm = getSystemService(NotificationManager.class);
        NotificationChannel ch = new NotificationChannel(CHANNEL,
                getString(R.string.channel_playback), NotificationManager.IMPORTANCE_LOW);
        ch.setShowBadge(false);
        nm.createNotificationChannel(ch);
        running = this;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && intent.hasExtra(ACTION)) {
            MainActivity.nativeCommand(intent.getStringExtra(ACTION));
        }
        running = this;
        // Started with startForegroundService: Android insists on
        // startForeground, even if the music paused in the meantime.
        try {
            foreground(build());
        } catch (RuntimeException e) {
            android.util.Log.w("chippy", "cannot go to the foreground", e);
        }
        show();
        return START_NOT_STICKY;
    }

    @Override
    public void onTaskRemoved(Intent rootIntent) {
        // Swiped away from the recent apps: the music stops with the app.
        stop(this);
        MediaBridge.release(getApplicationContext());
        android.os.Process.killProcess(android.os.Process.myPid());
    }

    @Override
    public void onDestroy() {
        if (running == this) {
            running = null;
        }
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private void foreground(Notification n) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(ID, n);
        }
    }

    private void show() {
        Notification n = build();
        if (MediaBridge.state == 1) {
            try {
                foreground(n);
            } catch (RuntimeException e) {
                getSystemService(NotificationManager.class).notify(ID, n);
            }
        } else {
            // Paused: may be swiped away, and Android may stop the service.
            stopForeground(STOP_FOREGROUND_DETACH);
            getSystemService(NotificationManager.class).notify(ID, n);
        }
    }

    private Notification build() {
        boolean playing = MediaBridge.state == 1;
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification.Builder b = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_chippy)
                .setContentTitle(MediaBridge.title.isEmpty() ? getString(R.string.app_name)
                        : MediaBridge.title)
                .setContentText(MediaBridge.artist)
                .setSubText(MediaBridge.album)
                .setContentIntent(open)
                .setOngoing(playing)
                .setShowWhen(false)
                .setVisibility(Notification.VISIBILITY_PUBLIC)
                .setCategory(Notification.CATEGORY_TRANSPORT)
                .addAction(action(android.R.drawable.ic_media_previous, R.string.previous,
                        "previous", 1))
                .addAction(playing
                        ? action(android.R.drawable.ic_media_pause, R.string.pause, "pause", 2)
                        : action(android.R.drawable.ic_media_play, R.string.play, "play", 2))
                .addAction(action(android.R.drawable.ic_media_next, R.string.next, "next", 3));
        Notification.MediaStyle style = new Notification.MediaStyle()
                .setShowActionsInCompactView(0, 1, 2);
        if (MediaBridge.session != null) {
            style.setMediaSession(MediaBridge.session.getSessionToken());
        }
        b.setStyle(style);
        return b.build();
    }

    private Notification.Action action(int icon, int label, String command, int request) {
        Intent i = new Intent(this, PlaybackService.class).putExtra(ACTION, command);
        PendingIntent pi = PendingIntent.getService(this, request, i,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Action.Builder(Icon.createWithResource(this, icon),
                getString(label), pi).build();
    }
}
