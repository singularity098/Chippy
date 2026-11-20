package io.github.singularity098.chippy

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import io.github.singularity098.chippy.core.NowState
import io.github.singularity098.chippy.core.PlayState

/**
 * What Android knows of the music: a media session (lock screen, the
 * notification's controls, headset and Bluetooth buttons, Android Auto...),
 * audio focus (a call or another player pauses Chippy), and headphones
 * being unplugged (pauses too). The foreground [PlaybackService] runs
 * while something plays, so Android lets it go on in the background.
 *
 * Everything here runs on the main thread; the engine's updates are
 * posted to it.
 */
object MediaBridge {
    private val main = Handler(Looper.getMainLooper())

    var session: MediaSession? = null
        private set
    var title = ""
        private set
    var artist = ""
        private set
    var album = ""
        private set
    private var duration = -1L
    /** The cover, for the notification and the lock screen. */
    var art: Bitmap? = null
        private set
    var state = PlayState.STOPPED
        private set
    private var seekSerial = ULong.MAX_VALUE

    private lateinit var audio: AudioManager
    private lateinit var focusRequest: AudioFocusRequest
    private var pausedForFocus = false
    private var noisyRegistered = false
    private lateinit var store: Store

    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                store.send { pause() }
            }
        }
    }

    fun init(ctx: Context, store: Store) {
        if (session != null) return
        this.store = store
        audio = ctx.getSystemService(AudioManager::class.java)
        val s = MediaSession(ctx, "Chippy")
        s.setCallback(object : MediaSession.Callback() {
            override fun onPlay() = store.send { play() }
            override fun onPause() = store.send { pause() }
            override fun onStop() = store.send { stop() }
            override fun onSkipToNext() = store.send { next() }
            override fun onSkipToPrevious() = store.send { previous() }
            override fun onSeekTo(pos: Long) = store.send { seekTo(pos.coerceAtLeast(0).toULong()) }
        })
        s.setSessionActivity(
            PendingIntent.getActivity(
                ctx, 0, Intent(ctx, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        )
        session = s
        setState(PlaybackState.STATE_STOPPED, 0)
        val attrs = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build()
        focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attrs)
            .setOnAudioFocusChangeListener(::focusChanged, main)
            .build()
    }

    /** What plays changed (called from the engine's thread). */
    fun update(ctx: Context, now: NowState) {
        main.post {
            if (session == null) return@post
            val length = now.durationMs?.toLong() ?: -1L
            if (now.title != title || now.artist != artist || now.album != album || length != duration) {
                title = now.title
                artist = now.artist
                album = now.album
                duration = length
                publishMetadata()
                PlaybackService.refresh()
            }
            if (now.state != state || now.seekSerial != seekSerial) {
                seekSerial = now.seekSerial
                setPlayback(ctx, now.state, now.positionMs.toLong())
            }
        }
    }

    /** The cover for the notification and the lock screen; null for none. */
    fun updateArt(bitmap: Bitmap?) {
        main.post {
            if (session == null) return@post
            art = bitmap
            publishMetadata()
            PlaybackService.refresh()
        }
    }

    private fun publishMetadata() {
        val b = MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadata.METADATA_KEY_ALBUM, album)
        if (duration >= 0) b.putLong(MediaMetadata.METADATA_KEY_DURATION, duration)
        art?.let { b.putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, it) }
        session?.setMetadata(b.build())
    }

    private fun setPlayback(ctx: Context, newState: PlayState, positionMs: Long) {
        val s = session ?: return
        val was = state
        state = newState
        when (newState) {
            PlayState.PLAYING -> {
                setState(PlaybackState.STATE_PLAYING, positionMs)
                if (!s.isActive) s.isActive = true
                if (was != PlayState.PLAYING) {
                    pausedForFocus = false
                    audio.requestAudioFocus(focusRequest)
                    if (!noisyRegistered) {
                        ctx.registerReceiver(noisy, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY))
                        noisyRegistered = true
                    }
                }
                PlaybackService.start(ctx)
            }
            PlayState.PAUSED -> {
                setState(PlaybackState.STATE_PAUSED, positionMs)
                unregisterNoisy(ctx)
                PlaybackService.refresh()
            }
            PlayState.STOPPED -> {
                setState(PlaybackState.STATE_STOPPED, 0)
                unregisterNoisy(ctx)
                if (!pausedForFocus) audio.abandonAudioFocusRequest(focusRequest)
                PlaybackService.stop(ctx)
            }
        }
    }

    fun release(ctx: Context) {
        main.post {
            PlaybackService.stop(ctx)
            unregisterNoisy(ctx)
            if (::audio.isInitialized) audio.abandonAudioFocusRequest(focusRequest)
            session?.let {
                it.isActive = false
                it.release()
            }
            session = null
        }
    }

    private fun setState(s: Int, positionMs: Long) {
        val actions = PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or
            PlaybackState.ACTION_PLAY_PAUSE or PlaybackState.ACTION_STOP or
            PlaybackState.ACTION_SKIP_TO_NEXT or PlaybackState.ACTION_SKIP_TO_PREVIOUS or
            PlaybackState.ACTION_SEEK_TO
        session?.setPlaybackState(
            PlaybackState.Builder()
                .setActions(actions)
                .setState(
                    s, positionMs,
                    if (s == PlaybackState.STATE_PLAYING) 1f else 0f,
                    SystemClock.elapsedRealtime(),
                )
                .build()
        )
    }

    private fun unregisterNoisy(ctx: Context) {
        if (noisyRegistered) {
            try {
                ctx.unregisterReceiver(noisy)
            } catch (ignored: IllegalArgumentException) {
                // Not registered after all.
            }
            noisyRegistered = false
        }
    }

    private fun focusChanged(change: Int) {
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                pausedForFocus = false
                store.send { pause() }
            }
            // A call, a navigation prompt: pause, and carry on after.
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> if (state == PlayState.PLAYING) {
                pausedForFocus = true
                store.send { pause() }
            }
            AudioManager.AUDIOFOCUS_GAIN -> if (pausedForFocus) {
                pausedForFocus = false
                store.send { play() }
            }
        }
    }
}
