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
import android.provider.DocumentsContract;
import android.provider.Settings;
import android.view.Display;
import android.view.View;
import android.view.WindowManager;

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
    private static final int REQUEST_FOLDER = 3;
    private static final String STORAGE_DOCUMENTS = "com.android.externalstorage.documents";

    /** The setting the folder picker showing is for ("music"...). */
    private String folderFor;

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
        preferFastestRefreshRate();
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

    /** The cover for the media notification; null when there is none. */
    public void updateArt(byte[] rgba, int width, int height) {
        MediaBridge.updateArt(getApplicationContext(), rgba, width, height);
    }

    /**
     * The system's folder picker, for a setting ("music", "soundfont",
     * "export"), opening at {@code start} where it can. The folder comes
     * back as "folder:which:/its/path" ("folder:which:" if none).
     */
    public void pickFolder(String which, String start) {
        runOnUiThread(() -> {
            if (folderFor != null) {
                nativeCommand("folder:" + folderFor + ":");
            }
            folderFor = which;
            Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            Uri initial = documentUriFor(start);
            if (initial != null) {
                i.putExtra(DocumentsContract.EXTRA_INITIAL_URI, initial);
            }
            try {
                startActivityForResult(i, REQUEST_FOLDER);
            } catch (Exception e) {
                android.util.Log.w("chippy", "no folder picker", e);
                folderFor = null;
                nativeCommand("folder:" + which + ":");
            }
        });
    }

    /** The Rust app has ended (its window closed for good). */
    public void onNativeExit() {
        MediaBridge.release(getApplicationContext());
        runOnUiThread(this::finish);
    }

    @Override
    protected void onActivityResult(int code, int result, Intent data) {
        super.onActivityResult(code, result, data);
        if (code != REQUEST_FOLDER || folderFor == null) {
            return;
        }
        String which = folderFor;
        folderFor = null;
        String path = "";
        if (result == RESULT_OK && data != null && data.getData() != null) {
            path = pathFor(data.getData());
            if (path == null) {
                path = "";
                android.widget.Toast.makeText(this, R.string.folder_not_local,
                        android.widget.Toast.LENGTH_LONG).show();
            }
        }
        nativeCommand("folder:" + which + ":" + path);
    }

    /**
     * The file system path of a folder the picker returned. The app reads
     * folders by path (it has "All files access"), so only folders on the
     * device's own storage or an SD card will do; null for others (cloud
     * drives...).
     */
    private static String pathFor(Uri tree) {
        String id;
        try {
            id = DocumentsContract.getTreeDocumentId(tree);
        } catch (IllegalArgumentException e) {
            return null;
        }
        String authority = tree.getAuthority();
        if ("com.android.providers.downloads.documents".equals(authority)) {
            if (id.startsWith("raw:")) {
                return id.substring(4);
            }
            if (id.equals("downloads")) {
                return Environment.getExternalStoragePublicDirectory(
                        Environment.DIRECTORY_DOWNLOADS).getAbsolutePath();
            }
            return null;
        }
        if (!STORAGE_DOCUMENTS.equals(authority)) {
            return null;
        }
        int colon = id.indexOf(':');
        String volume = colon < 0 ? id : id.substring(0, colon);
        String rest = colon < 0 ? "" : id.substring(colon + 1);
        String root;
        if (volume.equals("primary")) {
            root = Environment.getExternalStorageDirectory().getAbsolutePath();
        } else if (volume.equals("home")) {
            root = Environment.getExternalStoragePublicDirectory(
                    Environment.DIRECTORY_DOCUMENTS).getAbsolutePath();
        } else {
            root = "/storage/" + volume;
        }
        return rest.isEmpty() ? root : root + "/" + rest;
    }

    /** The picker's document for a path on the device's storage, if any. */
    private static Uri documentUriFor(String path) {
        if (path == null || path.isEmpty()) {
            return null;
        }
        String primary = Environment.getExternalStorageDirectory().getAbsolutePath();
        String id;
        if (path.equals(primary)) {
            id = "primary:";
        } else if (path.startsWith(primary + "/")) {
            id = "primary:" + path.substring(primary.length() + 1);
        } else if (path.startsWith("/storage/")) {
            String rest = path.substring("/storage/".length());
            int slash = rest.indexOf('/');
            id = slash < 0 ? rest + ":" : rest.substring(0, slash) + ":" + rest.substring(slash + 1);
        } else {
            return null;
        }
        return DocumentsContract.buildDocumentUri(STORAGE_DOCUMENTS, id);
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

    // ---------------------------------------------------------- frame rate

    /**
     * Phones with 90 or 120 Hz screens often give apps 60 Hz unless they
     * ask: ask for the fastest mode the display has at its current size.
     */
    private void preferFastestRefreshRate() {
        Display display = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R
                ? getDisplay() : getWindowManager().getDefaultDisplay();
        if (display == null) {
            return;
        }
        Display.Mode current = display.getMode();
        Display.Mode best = current;
        for (Display.Mode m : display.getSupportedModes()) {
            if (m.getPhysicalWidth() == current.getPhysicalWidth()
                    && m.getPhysicalHeight() == current.getPhysicalHeight()
                    && m.getRefreshRate() > best.getRefreshRate()) {
                best = m;
            }
        }
        WindowManager.LayoutParams lp = getWindow().getAttributes();
        lp.preferredDisplayModeId = best.getModeId();
        lp.preferredRefreshRate = best.getRefreshRate();
        getWindow().setAttributes(lp);
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
