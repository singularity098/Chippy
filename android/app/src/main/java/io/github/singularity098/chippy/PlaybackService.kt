package io.github.singularity098.chippy

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.IBinder
import io.github.singularity098.chippy.core.PlayState

/**
 * Keeps Chippy playing with the screen off or another app in front: a
 * foreground service while music plays, showing the media notification
 * (title, artist, previous / play-pause / next). Paused, the notification
 * stays but the service leaves the foreground; stopped, both go.
 */
class PlaybackService : Service() {
    companion object {
        private const val CHANNEL = "playback"
        private const val ID = 1
        private const val ACTION = "io.github.singularity098.chippy.ACTION"

        private var running: PlaybackService? = null

        fun start(ctx: Context) {
            running?.let {
                it.show()
                return
            }
            try {
                ctx.startForegroundService(Intent(ctx, PlaybackService::class.java))
            } catch (e: RuntimeException) {
                // Not allowed from the background on newer Android: the
                // music still plays while the app is in front.
                android.util.Log.w("chippy", "cannot start the playback service", e)
            }
        }

        fun refresh() {
            running?.show()
        }

        fun stop(ctx: Context) {
            running?.let {
                it.stopForeground(STOP_FOREGROUND_REMOVE)
                it.stopSelf()
            }
            running = null
            ctx.getSystemService(NotificationManager::class.java).cancel(ID)
        }
    }

    private val store get() = (application as ChippyApplication).store

    override fun onCreate() {
        super.onCreate()
        val ch = NotificationChannel(
            CHANNEL, getString(R.string.channel_playback), NotificationManager.IMPORTANCE_LOW,
        )
        ch.setShowBadge(false)
        getSystemService(NotificationManager::class.java).createNotificationChannel(ch)
        running = this
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.getStringExtra(ACTION)) {
            "previous" -> store.send { previous() }
            "play" -> store.send { play() }
            "pause" -> store.send { pause() }
            "next" -> store.send { next() }
        }
        running = this
        // Started with startForegroundService: Android insists on
        // startForeground, even if the music paused in the meantime.
        try {
            foreground(build())
        } catch (e: RuntimeException) {
            android.util.Log.w("chippy", "cannot go to the foreground", e)
        }
        show()
        return START_NOT_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiped away from the recent apps: the music stops with the app.
        store.shutdown()
        stop(this)
        MediaBridge.release(applicationContext)
        android.os.Process.killProcess(android.os.Process.myPid())
    }

    override fun onDestroy() {
        if (running === this) running = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun foreground(n: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        } else {
            startForeground(ID, n)
        }
    }

    private fun show() {
        val n = build()
        val nm = getSystemService(NotificationManager::class.java)
        if (MediaBridge.state == PlayState.PLAYING) {
            try {
                foreground(n)
            } catch (e: RuntimeException) {
                nm.notify(ID, n)
            }
        } else {
            // Paused: may be swiped away, and Android may stop the service.
            stopForeground(STOP_FOREGROUND_DETACH)
            nm.notify(ID, n)
        }
    }

    private fun build(): Notification {
        val playing = MediaBridge.state == PlayState.PLAYING
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val b = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_chippy)
            .setContentTitle(MediaBridge.title.ifEmpty { getString(R.string.app_name) })
            .setContentText(MediaBridge.artist)
            .setSubText(MediaBridge.album)
            .setContentIntent(open)
            .setOngoing(playing)
            .setShowWhen(false)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setCategory(Notification.CATEGORY_TRANSPORT)
            .addAction(action(android.R.drawable.ic_media_previous, R.string.previous, "previous", 1))
            .addAction(
                if (playing) action(android.R.drawable.ic_media_pause, R.string.pause, "pause", 2)
                else action(android.R.drawable.ic_media_play, R.string.play, "play", 2)
            )
            .addAction(action(android.R.drawable.ic_media_next, R.string.next, "next", 3))
        MediaBridge.art?.let { b.setLargeIcon(it) }
        val style = Notification.MediaStyle().setShowActionsInCompactView(0, 1, 2)
        MediaBridge.session?.let { style.setMediaSession(it.sessionToken) }
        b.style = style
        return b.build()
    }

    private fun action(icon: Int, label: Int, command: String, request: Int): Notification.Action {
        val i = Intent(this, PlaybackService::class.java).putExtra(ACTION, command)
        val pi = PendingIntent.getService(
            this, request, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Action.Builder(Icon.createWithResource(this, icon), getString(label), pi).build()
    }
}
