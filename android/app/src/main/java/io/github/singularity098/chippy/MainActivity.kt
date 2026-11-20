package io.github.singularity098.chippy

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.Settings
import android.view.Display
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import io.github.singularity098.chippy.ui.ChippyRoot
import io.github.singularity098.chippy.ui.ChippyTheme
import io.github.singularity098.chippy.ui.lookNamed
import io.github.singularity098.chippy.ui.Host
import io.github.singularity098.chippy.ui.ChippyIcons
import io.github.singularity098.chippy.ui.PaintedIcon

/**
 * Chippy's one activity: the Compose UI over the engine (which lives in
 * [ChippyApplication], so it outlasts the activity), plus what needs an
 * activity: storage and notification permissions, and the folder picker.
 */
class MainActivity : ComponentActivity(), Host {
    private val store get() = (application as ChippyApplication).store

    /** The setting the folder picker showing is for ("music"...). */
    private var folderFor: String? = null
    private var hadStorage = false
    private var askStorage by mutableStateOf(false)

    private val pickFolder = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        val which = folderFor ?: return@registerForActivityResult
        folderFor = null
        if (uri == null) return@registerForActivityResult
        val path = Folders.pathFor(uri)
        if (path == null) {
            Toast.makeText(this, R.string.folder_not_local, Toast.LENGTH_LONG).show()
            return@registerForActivityResult
        }
        store.send {
            when (which) {
                "music" -> addMusicDir(path)
                "soundfont" -> setSoundfontDir(path)
                "export" -> setExportDir(path)
            }
        }
    }

    private val legacyStorage = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        if (hasStorage()) {
            hadStorage = true
            store.send { refresh() }
        }
        askForNotifications()
    }

    private val notifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        hadStorage = hasStorage()
        if (!hadStorage) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                askStorage = savedInstanceState == null
            } else {
                legacyStorage.launch(
                    arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE)
                )
            }
        } else {
            askForNotifications()
        }
        preferFastestRefreshRate()
        setContent {
            ChippyRoot(store, this)
            if (askStorage) ChippyTheme(lookNamed(store.settings.value?.theme)) {
                AlertDialog(
                    onDismissRequest = {},
                    icon = { PaintedIcon(ChippyIcons.Folder) },
                    title = { Text(stringResource(R.string.storage_title)) },
                    text = { Text(stringResource(R.string.storage_message)) },
                    confirmButton = {
                        TextButton(onClick = {
                            askStorage = false
                            openAllFilesSettings()
                        }) { Text(stringResource(R.string.storage_allow)) }
                    },
                    dismissButton = {
                        TextButton(onClick = {
                            askStorage = false
                            askForNotifications()
                        }) { Text(stringResource(R.string.storage_later)) }
                    },
                )
            }
        }
    }

    override fun onResume() {
        super.onResume()
        // Back from the "All files access" settings page.
        val has = hasStorage()
        if (has && !hadStorage) {
            store.send { refresh() }
            askForNotifications()
        }
        hadStorage = has
    }

    override fun onStop() {
        super.onStop()
        // Android may end the app from here on without warning.
        store.send { save() }
    }

    // ------------------------------------------------------------ Host

    override fun moveToBack() {
        moveTaskToBack(true)
    }

    override fun pickFolder(which: String) {
        folderFor = which
        lifecycleScope.launch {
            val start = store.ask { pickerStart(which) }
            try {
                pickFolder.launch(Folders.documentUriFor(start))
            } catch (e: Exception) {
                folderFor = null
                Toast.makeText(this@MainActivity, R.string.no_folder_picker, Toast.LENGTH_LONG).show()
            }
        }
    }

    // ----------------------------------------------------- permissions

    private fun hasStorage(): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

    private fun openAllFilesSettings() {
        val i = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:$packageName"))
        try {
            startActivity(i)
        } catch (e: Exception) {
            startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
        }
    }

    private fun askForNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    /**
     * Phones with 90 or 120 Hz screens often give apps 60 Hz unless they
     * ask: ask for the fastest mode the display has at its current size.
     */
    @Suppress("DEPRECATION")
    private fun preferFastestRefreshRate() {
        val display: Display = (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) display else windowManager.defaultDisplay)
            ?: return
        val current = display.mode
        val best = display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxByOrNull { it.refreshRate } ?: return
        val lp = window.attributes
        lp.preferredDisplayModeId = best.modeId
        window.attributes = lp
    }
}
