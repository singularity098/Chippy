package io.github.singularity098.chippy;

import android.Manifest;
import android.app.AlertDialog;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Rect;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.view.View;

import java.util.ArrayList;
import java.util.List;

/**
 * Chippy's one activity. The app itself is the Rust library this
 * NativeActivity loads (crates/chippy-android); this class adds what needs
 * Java: storage and notification permissions, the back button, keeping the
 * screen edges for the sidebars, and the media session ({@link MediaBridge}).
 *
 * The Rust side calls the public methods below through JNI (from its own
 * thread), and sends nothing else; Java talks to Rust through
 * {@link #nativeCommand}.
 */
public class MainActivity extends NativeActivity {
    static {
        // NativeActivity loads the library itself, but not through the
        // class loader: loading it here too lets nativeCommand be found.
        System.loadLibrary("chippy_android");
    }

    /** "play", "pause", "next", "seek:12345", "back", "refresh"... */
    static native void nativeCommand(String command);

    private static final int REQUEST_STORAGE = 1;
    private static final int REQUEST_NOTIFICATIONS = 2;

    private boolean hadStorage;
    private boolean askedForStorage;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        MediaBridge.init(getApplicationContext());
        hadStorage = hasStorage();
        if (!hadStorage) {
            askForStorage();
        } else {
            askForNotifications();
        }
        keepEdgesForSidebars();
    }

    @Override
    protected void onResume() {
        super.onResume();
        // Back from the "All files access" settings page.
        boolean has = hasStorage();
        if (has && !hadStorage) {
            nativeCommand("refresh");
            askForNotifications();
        }
        hadStorage = has;
    }

    @Override
    protected void onDestroy() {
        // Returns once the Rust side has shut down (settings saved).
        super.onDestroy();
        MediaBridge.release(getApplicationContext());
        // The Rust app lives once per process: a new activity in this one
        // could not start it again, so the process goes with the activity.
        android.os.Process.killProcess(android.os.Process.myPid());
    }

    /** The back button or gesture: Chippy decides (close a panel...). */
    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        nativeCommand("back");
    }

    // ------------------------------------------------------- called by Rust

    /** Back with nothing left to close: to the background, still playing. */
    public void moveToBack() {
        runOnUiThread(() -> moveTaskToBack(true));
    }

    public void updateMetadata(String title, String artist, String album, long durationMs) {
        MediaBridge.updateMetadata(getApplicationContext(), title, artist, album, durationMs);
    }

    /** state: 0 stopped, 1 playing, 2 paused. */
    public void updatePlayback(int state, long positionMs) {
        MediaBridge.updatePlayback(getApplicationContext(), state, positionMs);
    }

    /** The Rust app has ended (its window closed for good). */
    public void onNativeExit() {
        MediaBridge.release(getApplicationContext());
        runOnUiThread(this::finish);
    }

    // ---------------------------------------------------------- permissions

    private boolean hasStorage() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            return Environment.isExternalStorageManager();
        }
        return checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE)
                == PackageManager.PERMISSION_GRANTED;
    }

    private void askForStorage() {
        if (askedForStorage) {
            return;
        }
        askedForStorage = true;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            requestPermissions(new String[] {
                Manifest.permission.READ_EXTERNAL_STORAGE,
                Manifest.permission.WRITE_EXTERNAL_STORAGE,
            }, REQUEST_STORAGE);
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle(R.string.storage_title)
                .setMessage(R.string.storage_message)
                .setPositiveButton(R.string.storage_allow, (d, w) -> {
                    Intent i = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                            Uri.parse("package:" + getPackageName()));
                    try {
                        startActivity(i);
                    } catch (Exception e) {
                        startActivity(new Intent(
                                Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                    }
                })
                .setNegativeButton(R.string.storage_later, (d, w) -> askForNotifications())
                .show();
    }

    private void askForNotifications() {
        if (Build.VERSION.SDK_INT >= 33
                && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS},
                    REQUEST_NOTIFICATIONS);
        }
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(code, permissions, results);
        if (code == REQUEST_STORAGE) {
            if (hasStorage()) {
                hadStorage = true;
                nativeCommand("refresh");
            }
            askForNotifications();
        }
    }

    // ---------------------------------------------------------------- edges

    /**
     * With gesture navigation a swipe in from the side means "back"; the
     * middle of each edge is kept for Chippy, where a swipe brings in the
     * file browser (left) or the settings (right). Android grants up to
     * 200 dp of each edge.
     */
    private void keepEdgesForSidebars() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return;
        }
        View decor = getWindow().getDecorView();
        decor.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            float dp = getResources().getDisplayMetrics().density;
            int w = r - l;
            int h = b - t;
            int edge = (int) (24 * dp);
            int band = (int) (200 * dp);
            int top = Math.max(0, (h - band) / 2);
            List<Rect> rects = new ArrayList<>();
            rects.add(new Rect(0, top, edge, top + band));
            rects.add(new Rect(w - edge, top, w, top + band));
            v.setSystemGestureExclusionRects(rects);
        });
    }
}
