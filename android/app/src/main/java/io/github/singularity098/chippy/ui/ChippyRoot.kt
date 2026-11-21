package io.github.singularity098.chippy.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.LyricsInfo
import io.github.singularity098.chippy.core.TrackInfo
import io.github.singularity098.chippy.core.UiEvent
import kotlinx.coroutines.launch

enum class Page { Player, Browser, Playlist, Settings }

/** What only the activity can do. */
interface Host {
    /** The system's folder picker, for "music", "soundfont" or "export". */
    fun pickFolder(which: String)

    /** Back with nothing open: to the background, still playing. */
    fun moveToBack()
}

@Composable
fun ChippyRoot(store: Store, host: Host) {
    val settings by store.settings.collectAsStateWithLifecycle()
    val look = lookNamed(settings?.theme)
    ChippyTheme(look) {
        val now by store.now.collectAsStateWithLifecycle()
        val browser by store.browser.collectAsStateWithLifecycle()
        val playlist by store.playlist.collectAsStateWithLifecycle()
        val export by store.export.collectAsStateWithLifecycle()
        val tabs by store.tabs.collectAsStateWithLifecycle()
        var lyrics by remember { mutableStateOf<LyricsInfo?>(null) }
        var page by rememberSaveable { mutableStateOf(Page.Player) }
        var showExport by rememberSaveable { mutableStateOf(false) }
        var info by remember { mutableStateOf<TrackInfo?>(null) }
        val snackbar = remember { SnackbarHostState() }
        val scope = rememberCoroutineScope()

        LaunchedEffect(Unit) {
            store.events.collect { e ->
                when (e) {
                    is UiEvent.Toast -> launch {
                        snackbar.currentSnackbarData?.dismiss()
                        snackbar.showSnackbar(
                            e.text,
                            withDismissAction = e.error,
                            duration = if (e.error) SnackbarDuration.Long else SnackbarDuration.Short,
                        )
                    }
                    is UiEvent.OpenBrowser -> page = Page.Browser
                }
            }
        }

        BackHandler(enabled = page == Page.Player) { host.moveToBack() }

        Scaffold(
            snackbarHost = { SnackbarHost(snackbar) },
            containerColor = ChippyBackground,
        ) { _ ->
            Box(
                Modifier
                    .fillMaxSize()
                    .background(ChippyBackground)
                    .background(
                        // A glow of the theme behind the player.
                        Brush.radialGradient(
                            listOf(look.paintC.copy(alpha = 0.10f), Color.Transparent),
                            radius = 1400f,
                        )
                    )
                    .swipeBetweenPages(page) { page = it },
            ) {
                AnimatedContent(
                    targetState = page,
                    transitionSpec = { transition(initialState, targetState) },
                    label = "page",
                ) { p ->
                    when (p) {
                        Page.Player -> PlayerScreen(
                            store, now,
                            onOpenBrowser = { page = Page.Browser },
                            onOpenPlaylist = { page = Page.Playlist },
                            onOpenSettings = { page = Page.Settings },
                            onLyrics = { scope.launch { lyrics = store.ask { lyrics() } } },
                            onTabs = { store.send { openTabs() } },
                        )
                        Page.Browser -> BrowserScreen(store, browser, onClose = { page = Page.Player })
                        Page.Playlist -> PlaylistScreen(
                            store, playlist,
                            onClose = { page = Page.Player },
                            onOpenBrowser = { page = Page.Browser },
                            onInfo = { i ->
                                scope.launch { info = store.ask { trackInfo(i.toUInt()) } }
                            },
                            onExport = { row ->
                                scope.launch {
                                    if (store.ask { prepareExport(row?.toUInt()) } == true) showExport = true
                                }
                            },
                        )
                        Page.Settings -> SettingsScreen(
                            store, settings,
                            onClose = { page = Page.Player },
                            onPickFolder = host::pickFolder,
                        )
                    }
                }
            }
        }

        if (showExport) {
            ExportDialog(store, export, onPickFolder = { host.pickFolder("export") }, onClose = { showExport = false })
        }
        info?.let { InfoSheet(it, onClose = { info = null }) }
        // The lyrics follow the playlist to the next track.
        LaunchedEffect(now?.title, now?.location) {
            if (lyrics != null) lyrics = store.ask { lyrics() }
        }
        lyrics?.let { LyricsSheet(store, it, onClose = { lyrics = null }) }
        tabs?.takeIf { it.shown }?.let { TabsSheet(store, it, onClose = { store.send { closeTabs() } }) }
    }
}

/**
 * Swiping sideways moves between the pages the way they slide in: on the
 * player, a swipe to the right pulls the browser in from the left and one
 * to the left pulls the settings in from the right; the opposite swipe
 * puts either away. Drags a slider or a list takes for itself (the seek
 * and volume bars) are left to it.
 */
private fun Modifier.swipeBetweenPages(page: Page, go: (Page) -> Unit): Modifier =
    pointerInput(page) {
        val needed = 72.dp.toPx()
        var dx = 0f
        detectHorizontalDragGestures(
            onDragStart = { dx = 0f },
            onDragCancel = { dx = 0f },
            onDragEnd = {
                val to = when {
                    dx > needed -> when (page) {
                        Page.Player -> Page.Browser
                        Page.Settings -> Page.Player
                        else -> null
                    }
                    dx < -needed -> when (page) {
                        Page.Player -> Page.Settings
                        Page.Browser -> Page.Player
                        else -> null
                    }
                    else -> null
                }
                to?.let(go)
            },
            onHorizontalDrag = { _, amount -> dx += amount },
        )
    }

/** The browser comes from the left, the settings from the right, the
 * playlist from below, as their buttons sit on the player. */
private fun transition(from: Page, to: Page): ContentTransform {
    val ms = 280
    fun enter(p: Page) = when (p) {
        Page.Browser -> slideInHorizontally(tween(ms)) { -it }
        Page.Settings -> slideInHorizontally(tween(ms)) { it }
        Page.Playlist -> slideInVertically(tween(ms)) { it }
        Page.Player -> fadeIn(tween(ms))
    }
    fun exit(p: Page) = when (p) {
        Page.Browser -> slideOutHorizontally(tween(ms)) { -it }
        Page.Settings -> slideOutHorizontally(tween(ms)) { it }
        Page.Playlist -> slideOutVertically(tween(ms)) { it }
        Page.Player -> fadeOut(tween(ms))
    }
    return if (to == Page.Player) {
        fadeIn(tween(ms)) togetherWith exit(from)
    } else {
        enter(to) togetherWith fadeOut(tween(ms))
    }.apply { targetContentZIndex = if (to == Page.Player) -1f else 1f }
}
