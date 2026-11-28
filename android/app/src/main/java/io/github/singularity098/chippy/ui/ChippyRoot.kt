package io.github.singularity098.chippy.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.LyricsInfo
import io.github.singularity098.chippy.core.TabsState
import io.github.singularity098.chippy.core.TrackInfo
import io.github.singularity098.chippy.core.UiEvent
import kotlin.math.abs
import kotlin.math.roundToInt
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
        // The page underneath (the player or the playlist), and the panel
        // pulled out over it (the browser or the settings), if any.
        var base by rememberSaveable { mutableStateOf(Page.Player) }
        var panel by rememberSaveable { mutableStateOf<Page?>(null) }
        var showExport by rememberSaveable { mutableStateOf(false) }
        var info by remember { mutableStateOf<TrackInfo?>(null) }
        val snackbar = remember { SnackbarHostState() }
        val scope = rememberCoroutineScope()
        val pull = remember { PanelPull(panel.pullTarget(), scope) }
        // Opened or closed by a button, Back or the engine: slid there.
        LaunchedEffect(panel) {
            val to = panel.pullTarget()
            if (pull.value != to) pull.settle(to)
        }

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
                    is UiEvent.OpenBrowser -> panel = Page.Browser
                }
            }
        }

        BackHandler(enabled = base == Page.Player && panel == null) { host.moveToBack() }

        // The lyrics or the guitar tab, slid up over the player (one at a
        // time); back, the arrow or a drag down puts it away.
        val tabsShown = tabs?.shown == true
        val sheetOpen = tabsShown || lyrics != null
        fun closeSheet() {
            if (tabsShown) store.send { closeTabs() }
            lyrics = null
        }
        BackHandler(enabled = sheetOpen) { closeSheet() }
        // What the panel showed, kept while it slides away.
        var lastLyrics by remember { mutableStateOf<LyricsInfo?>(null) }
        var lastTabs by remember { mutableStateOf<TabsState?>(null) }
        if (tabsShown) {
            lastTabs = tabs
            lastLyrics = null
        } else if (lyrics != null) {
            lastLyrics = lyrics
            lastTabs = null
        }

        Scaffold(
            snackbarHost = { SnackbarHost(snackbar) },
            containerColor = ChippyBackground,
        ) { _ ->
            BoxWithConstraints(
                Modifier
                    .fillMaxSize()
                    .background(ChippyBackground)
                    .background(
                        // A glow of the theme behind the player.
                        Brush.radialGradient(
                            listOf(look.paintC.copy(alpha = 0.10f), Color.Transparent),
                            radius = 1400f,
                        )
                    ),
            ) {
                // The panels come nearly across the screen, a strip of the
                // player left beside them to tap or drag them away by.
                val panelWidth = maxOf(maxWidth - 36.dp, minOf(maxWidth, 280.dp))
                val widthPx = with(LocalDensity.current) { panelWidth.toPx() }
                fun letGo(velocity: Float) {
                    val to = pull.target(velocity)
                    val next = when {
                        to > 0f -> Page.Browser
                        to < 0f -> Page.Settings
                        else -> null
                    }
                    if (next != panel) panel = next
                    pull.settle(to, velocity, widthPx)
                }
                val dragOpen = { pull.value.let { if (it > 0.5f) 1f else if (it < -0.5f) -1f else 0f } }
                Box(
                    Modifier
                        .fillMaxSize()
                        // On the player, a swipe right pulls the browser out
                        // and one left the settings, following the finger.
                        .then(
                            if (base == Page.Player) {
                                Modifier.pullPanels(pull, base to widthPx, { 0f }, { widthPx }, ::letGo)
                            } else {
                                Modifier
                            }
                        ),
                ) {
                    AnimatedContent(
                        targetState = base,
                        transitionSpec = { transition(initialState, targetState) },
                        label = "page",
                    ) { p ->
                        when (p) {
                            Page.Playlist -> PlaylistScreen(
                                store, playlist,
                                onClose = { base = Page.Player },
                                onOpenBrowser = { base = Page.Player; panel = Page.Browser },
                                onInfo = { i ->
                                    scope.launch { info = store.ask { trackInfo(i.toUInt()) } }
                                },
                                onExport = { row ->
                                    scope.launch {
                                        if (store.ask { prepareExport(row?.toUInt()) } == true) showExport = true
                                    }
                                },
                            )
                            else -> PlayerScreen(
                                store, now,
                                onOpenBrowser = { panel = Page.Browser },
                                onOpenPlaylist = { base = Page.Playlist },
                                onOpenSettings = { panel = Page.Settings },
                                onLyrics = {
                                    if (tabsShown) store.send { closeTabs() }
                                    scope.launch { lyrics = store.ask { lyrics() } }
                                },
                                onTabs = {
                                    lyrics = null
                                    store.send { openTabs() }
                                },
                            )
                        }
                    }
                }
                val shown = pull.value
                if (abs(shown) > 0.001f) {
                    // The player darkens as a panel comes over it; a tap or
                    // a drag on what's left of it puts the panel away.
                    Box(
                        Modifier
                            .fillMaxSize()
                            .background(Color.Black.copy(alpha = 0.5f * abs(shown)))
                            .pullPanels(pull, panel to widthPx, dragOpen, { widthPx }, ::letGo)
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                            ) { panel = null },
                    )
                }
                if (shown > 0.001f || panel == Page.Browser) {
                    SidePanel(
                        left = true, width = panelWidth,
                        offsetPx = -(1f - shown.coerceAtLeast(0f)) * widthPx,
                        drag = Modifier.pullPanels(pull, panel to widthPx, dragOpen, { widthPx }, ::letGo),
                    ) {
                        BrowserScreen(store, browser, onClose = { panel = null })
                    }
                }
                if (shown < -0.001f || panel == Page.Settings) {
                    SidePanel(
                        left = false, width = panelWidth,
                        offsetPx = (1f + shown.coerceAtMost(0f)) * widthPx,
                        drag = Modifier.pullPanels(pull, panel to widthPx, dragOpen, { widthPx }, ::letGo),
                    ) {
                        SettingsScreen(
                            store, settings,
                            onClose = { panel = null },
                            onPickFolder = host::pickFolder,
                        )
                    }
                }
            }
            // Over the pages, but out of their sideways swipes.
            AnimatedVisibility(
                visible = sheetOpen,
                enter = slideInVertically(tween(280)) { it },
                exit = slideOutVertically(tween(280)) { it },
            ) {
                lastTabs?.let { TabsSheet(store, it, onClose = ::closeSheet) }
                    ?: lastLyrics?.let { LyricsSheet(store, it, onClose = ::closeSheet) }
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
    }
}

/** Where the panel pull rests with this panel out. */
private fun Page?.pullTarget(): Float = when (this) {
    Page.Browser -> 1f
    Page.Settings -> -1f
    else -> 0f
}

/** A panel at the left or right edge, `offsetPx` slid out of view (0
 * fully out), its inner corners rounded. */
@Composable
private fun BoxScope.SidePanel(
    left: Boolean,
    width: Dp,
    offsetPx: Float,
    drag: Modifier,
    content: @Composable () -> Unit,
) {
    val shape = if (left) {
        RoundedCornerShape(topEnd = 16.dp, bottomEnd = 16.dp)
    } else {
        RoundedCornerShape(topStart = 16.dp, bottomStart = 16.dp)
    }
    Box(
        Modifier
            .align(if (left) Alignment.CenterStart else Alignment.CenterEnd)
            .offset { IntOffset(offsetPx.roundToInt(), 0) }
            .width(width)
            .fillMaxHeight()
            .shadow(12.dp, shape)
            .clip(shape)
            .then(drag),
    ) { content() }
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
