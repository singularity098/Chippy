package io.github.singularity098.chippy;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.graphics.Bitmap;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

/**
 * What Android knows of the music: a media session (lock screen, the
 * notification's controls, headset and Bluetooth buttons, Android Auto...),
 * audio focus (a call or another player pauses Chippy), and headphones
 * being unplugged (pauses too). The foreground {@link PlaybackService}
 * runs while something plays, so Android lets it go on in the background.
 *
 * Everything here runs on the main thread; the Rust side's calls are
 * posted to it.
 */
final class MediaBridge {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    static MediaSession session;
    static String title = "";
    static String artist = "";
    static String album = "";
    static long duration = -1;
    /** The cover, for the notification and the lock screen; may be null. */
    static Bitmap art;
    static int state; // 0 stopped, 1 playing, 2 paused

    private static AudioManager audio;
    private static AudioFocusRequest focusRequest;
    private static boolean pausedForFocus;
    private static boolean noisyRegistered;

    private static final BroadcastReceiver NOISY = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (AudioManager.ACTION_AUDIO_BECOMING_NOISY.equals(intent.getAction())) {
                MainActivity.nativeCommand("pause");
            }
        }
    };

    private MediaBridge() {}

    static void init(Context ctx) {
        if (session != null) {
            return;
        }
        audio = ctx.getSystemService(AudioManager.class);
        session = new MediaSession(ctx, "Chippy");
        session.setCallback(new MediaSession.Callback() {
            @Override public void onPlay() { MainActivity.nativeCommand("play"); }
            @Override public void onPause() { MainActivity.nativeCommand("pause"); }
            @Override public void onStop() { MainActivity.nativeCommand("stop"); }
            @Override public void onSkipToNext() { MainActivity.nativeCommand("next"); }
            @Override public void onSkipToPrevious() { MainActivity.nativeCommand("previous"); }
            @Override public void onSeekTo(long pos) { MainActivity.nativeCommand("seek:" + pos); }
        });
        Intent open = new Intent(ctx, MainActivity.class);
        session.setSessionActivity(android.app.PendingIntent.getActivity(ctx, 0, open,
                android.app.PendingIntent.FLAG_IMMUTABLE
                        | android.app.PendingIntent.FLAG_UPDATE_CURRENT));
        setState(PlaybackState.STATE_STOPPED, 0);
        AudioAttributes attrs = new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build();
        focusRequest = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(attrs)
                .setOnAudioFocusChangeListener(MediaBridge::focusChanged, MAIN)
                .build();
    }

    static void updateMetadata(Context ctx, String t, String ar, String al, long durationMs) {
        MAIN.post(() -> {
            if (session == null) {
                return;
            }
            title = t;
            artist = ar;
            album = al;
            duration = durationMs;
            publishMetadata();
            PlaybackService.refresh(ctx);
        });
    }

    /** rgba: premultiplied pixels, width * height * 4 bytes; null for none. */
    static void updateArt(Context ctx, byte[] rgba, int width, int height) {
        Bitmap bitmap = null;
        if (rgba != null && width > 0 && height > 0 && rgba.length >= width * height * 4) {
            bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
            bitmap.copyPixelsFromBuffer(java.nio.ByteBuffer.wrap(rgba, 0, width * height * 4));
        }
        Bitmap b = bitmap;
        MAIN.post(() -> {
            if (session == null) {
                return;
            }
            art = b;
            publishMetadata();
            PlaybackService.refresh(ctx);
        });
    }

    private static void publishMetadata() {
        MediaMetadata.Builder b = new MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
                .putString(MediaMetadata.METADATA_KEY_ALBUM, album);
        if (duration >= 0) {
            b.putLong(MediaMetadata.METADATA_KEY_DURATION, duration);
        }
        if (art != null) {
            b.putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, art);
        }
        session.setMetadata(b.build());
    }

    static void updatePlayback(Context ctx, int newState, long positionMs) {
        MAIN.post(() -> {
            if (session == null) {
                return;
            }
            int was = state;
            state = newState;
            switch (newState) {
                case 1:
                    setState(PlaybackState.STATE_PLAYING, positionMs);
                    if (!session.isActive()) {
                        session.setActive(true);
                    }
                    if (was != 1) {
                        pausedForFocus = false;
                        audio.requestAudioFocus(focusRequest);
                        if (!noisyRegistered) {
                            ctx.registerReceiver(NOISY,
                                    new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY));
                            noisyRegistered = true;
                        }
                    }
                    PlaybackService.start(ctx);
                    break;
                case 2:
                    setState(PlaybackState.STATE_PAUSED, positionMs);
                    unregisterNoisy(ctx);
                    PlaybackService.refresh(ctx);
                    break;
                default:
                    setState(PlaybackState.STATE_STOPPED, 0);
                    unregisterNoisy(ctx);
                    if (!pausedForFocus) {
                        audio.abandonAudioFocusRequest(focusRequest);
                    }
                    PlaybackService.stop(ctx);
                    break;
            }
        });
    }

    static void release(Context ctx) {
        MAIN.post(() -> {
            PlaybackService.stop(ctx);
            unregisterNoisy(ctx);
            if (audio != null && focusRequest != null) {
                audio.abandonAudioFocusRequest(focusRequest);
            }
            if (session != null) {
                session.setActive(false);
                session.release();
                session = null;
            }
        });
    }

    private static void setState(int s, long positionMs) {
        long actions = PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_STOP
                | PlaybackState.ACTION_SKIP_TO_NEXT | PlaybackState.ACTION_SKIP_TO_PREVIOUS
                | PlaybackState.ACTION_SEEK_TO;
        session.setPlaybackState(new PlaybackState.Builder()
                .setActions(actions)
                .setState(s, positionMs, s == PlaybackState.STATE_PLAYING ? 1f : 0f,
                        SystemClock.elapsedRealtime())
                .build());
    }

    private static void unregisterNoisy(Context ctx) {
        if (noisyRegistered) {
            try {
                ctx.unregisterReceiver(NOISY);
            } catch (IllegalArgumentException ignored) {
                // Not registered after all.
            }
            noisyRegistered = false;
        }
    }

    private static void focusChanged(int change) {
        switch (change) {
            case AudioManager.AUDIOFOCUS_LOSS:
                pausedForFocus = false;
                MainActivity.nativeCommand("pause");
                break;
            case AudioManager.AUDIOFOCUS_LOSS_TRANSIENT:
            case AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK:
                // A call, a navigation prompt: pause, and carry on after.
                if (state == 1) {
                    pausedForFocus = true;
                    MainActivity.nativeCommand("pause");
                }
                break;
            case AudioManager.AUDIOFOCUS_GAIN:
                if (pausedForFocus) {
                    pausedForFocus = false;
                    MainActivity.nativeCommand("play");
                }
                break;
            default:
                break;
        }
    }
}
