package io.github.singularity098.chippy.ui

import androidx.compose.animation.Crossfade
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.LyricsInfo
import io.github.singularity098.chippy.core.NowState
import io.github.singularity098.chippy.core.PlayState
import io.github.singularity098.chippy.core.TabLine
import io.github.singularity098.chippy.core.TabsState
import kotlin.math.floor

/** Where playback is, in ms, followed every display frame. */
@Composable
private fun rememberPosition(store: Store): Long {
    val now by store.now.collectAsStateWithLifecycle()
    val frame = rememberFrame(store, now?.state == PlayState.PLAYING)
    return frame?.positionMs?.toLong() ?: 0L
}

/**
 * A panel over the player: a grip and heading on top (a tap on the arrow,
 * or dragging the top down, puts it away), what it shows, and a small
 * player below so the track can still be paused, sought and skipped.
 */
@Composable
private fun SheetPanel(
    store: Store,
    title: String,
    onClose: () -> Unit,
    actions: @Composable RowScope.() -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val now by store.now.collectAsStateWithLifecycle()
    Column(
        Modifier
            .fillMaxSize()
            .statusBarsPadding()
            .padding(top = 8.dp)
            .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            // Taps on the panel stay on it.
            .clickable(remember { MutableInteractionSource() }, indication = null) {},
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .pointerInput(Unit) {
                    val needed = 64.dp.toPx()
                    var dy = 0f
                    detectVerticalDragGestures(
                        onDragStart = { dy = 0f },
                        onDragEnd = { if (dy > needed) onClose() },
                        onDragCancel = { dy = 0f },
                    ) { _, amount -> dy += amount }
                },
        ) {
            Box(
                Modifier
                    .padding(top = 8.dp)
                    .size(40.dp, 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(MaterialTheme.colorScheme.outlineVariant)
                    .align(Alignment.CenterHorizontally),
            )
            Row(
                Modifier.padding(start = 24.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    title,
                    style = MaterialTheme.typography.titleLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                actions()
                IconButton(onClick = onClose) {
                    Icon(ChippyIcons.ChevronDown, stringResource(R.string.put_away))
                }
            }
        }
        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth(),
            content = content,
        )
        MiniPlayer(store, now)
    }
}

/** The transport under a panel: the timeline, the track, and play/pause. */
@Composable
private fun MiniPlayer(store: Store, now: NowState?) {
    val look = LocalLook.current
    val playing = now?.state == PlayState.PLAYING
    val frame = rememberFrame(store, playing)
    var dragging by remember { mutableStateOf<Float?>(null) }
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(
            Modifier
                .navigationBarsPadding()
                .padding(horizontal = 16.dp)
                .padding(bottom = 8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    dragging?.let { f -> now?.durationMs?.let { formatMs((it.toDouble() * f).toLong()) } }
                        ?: frame?.elapsed ?: "0:00",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.width(40.dp),
                )
                Slider(
                    value = dragging ?: frame?.fraction ?: 0f,
                    onValueChange = { dragging = it },
                    onValueChangeFinished = {
                        dragging?.let { f -> store.send { seekFraction(f) } }
                        dragging = null
                    },
                    enabled = now?.seekable == true,
                    colors = SliderDefaults.colors(
                        thumbColor = look.accent,
                        activeTrackColor = look.paintB,
                        inactiveTrackColor = Color.White.copy(alpha = 0.10f),
                        disabledInactiveTrackColor = Color.White.copy(alpha = 0.06f),
                        disabledActiveTrackColor = Color.White.copy(alpha = 0.2f),
                        disabledThumbColor = Color.White.copy(alpha = 0.25f),
                    ),
                    modifier = Modifier.weight(1f),
                )
                Text(
                    now?.total ?: "–:––",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier
                        .width(40.dp)
                        .padding(start = 6.dp),
                )
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        if (now?.hasTrack == true) now.title else stringResource(R.string.nothing_playing),
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (!now?.artist.isNullOrEmpty()) {
                        Text(
                            now!!.artist,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                IconButton(onClick = { store.send { previous() } }) {
                    Icon(ChippyIcons.Previous, stringResource(R.string.previous))
                }
                PaintedCircle(
                    48.dp,
                    onClick = { store.send { playPause() } },
                    onClickLabel = stringResource(if (playing) R.string.pause else R.string.play),
                    modifier = Modifier.padding(horizontal = 4.dp),
                ) {
                    Crossfade(playing, label = "play") { isPlaying ->
                        Icon(
                            if (isPlaying) ChippyIcons.Pause else ChippyIcons.Play,
                            null,
                            Modifier.size(22.dp),
                            tint = Color.Black,
                        )
                    }
                }
                IconButton(onClick = { store.send { next() } }) {
                    Icon(ChippyIcons.Next, stringResource(R.string.next))
                }
            }
        }
    }
}

/** The playing track's lyrics; timed ones follow the song. */
@Composable
fun LyricsSheet(store: Store, info: LyricsInfo, onClose: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    val look = LocalLook.current
    val pos = rememberPosition(store)
    val current = if (info.synced) info.lines.indexOfLast { it.ms in 0..pos } else -1
    val list = rememberLazyListState()
    LaunchedEffect(current) {
        if (current >= 0) list.animateScrollToItem((current - 2).coerceAtLeast(0))
    }
    SheetPanel(
        store,
        stringResource(R.string.lyrics_title, info.title),
        onClose,
        actions = {
            if (info.lines.isNotEmpty()) {
                TextButton(onClick = {
                    clipboard.setText(AnnotatedString(info.lines.joinToString("\n") { it.text }))
                }) { Text(stringResource(R.string.copy)) }
            }
        },
    ) {
        if (info.source.isNotEmpty()) {
            Text(
                info.source,
                Modifier.padding(horizontal = 24.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (info.lines.isEmpty()) {
            Text(
                info.message,
                Modifier.padding(24.dp),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            return@SheetPanel
        }
        LazyColumn(Modifier.fillMaxSize(), state = list) {
            itemsIndexed(info.lines) { i, line ->
                val now = i == current
                Text(
                    line.text.ifEmpty { " " },
                    Modifier
                        .fillMaxWidth()
                        .clickable(enabled = line.ms >= 0) { store.send { seekTo(line.ms.toULong()) } }
                        .padding(horizontal = 24.dp, vertical = 8.dp),
                    style = if (now) MaterialTheme.typography.headlineSmall else MaterialTheme.typography.titleMedium,
                    fontWeight = if (now) FontWeight.Medium else FontWeight.Light,
                    color = when {
                        now -> look.accent
                        info.synced && i < current -> MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        else -> MaterialTheme.colorScheme.onSurface
                    },
                )
            }
        }
    }
}

/**
 * A choice from a list: the one chosen, and the list below it on a tap.
 * Its width comes from where it is, so what is beside it stays put.
 */
@Composable
private fun Dropdown(
    items: List<String>,
    selected: Int,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    onPick: (Int) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val usable = enabled && items.isNotEmpty()
    Box(modifier) {
        Surface(
            onClick = { open = true },
            enabled = usable,
            shape = RoundedCornerShape(10.dp),
            color = MaterialTheme.colorScheme.surfaceContainerHigh,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier
                .fillMaxWidth()
                .height(40.dp),
        ) {
            Row(
                Modifier.padding(start = 12.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    items.getOrNull(selected) ?: "–",
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = if (usable) LocalLook.current.accent else MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Icon(
                    ChippyIcons.ChevronDown,
                    null,
                    Modifier.size(18.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            items.forEachIndexed { i, name ->
                DropdownMenuItem(
                    text = { Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    onClick = {
                        open = false
                        onPick(i)
                    },
                    leadingIcon = {
                        if (i == selected) {
                            Icon(ChippyIcons.Check, null, tint = LocalLook.current.accent)
                        } else {
                            Spacer(Modifier.size(24.dp))
                        }
                    },
                )
            }
        }
    }
}

/** Where the playhead is across `line` at `pos` ms, 0..1. */
private fun playheadAt(line: TabLine, pos: Long): Float {
    val m = line.marks
    if (m.isEmpty()) {
        val span = (line.endMs.toLong() - line.startMs.toLong()).coerceAtLeast(1L)
        return ((pos - line.startMs.toLong()).toFloat() / span).coerceIn(0f, 1f)
    }
    val i = m.indexOfFirst { it.ms.toLong() > pos }
    if (i == -1) return m.last().x
    if (i == 0) return m[0].x
    val a = m[i - 1]
    val b = m[i]
    val f = ((pos - a.ms.toLong()).toFloat() / (b.ms.toLong() - a.ms.toLong()).coerceAtLeast(1L))
        .coerceIn(0f, 1f)
    return a.x + (b.x - a.x) * f
}

/** The width a digit of tab is given (a fret number and its padding). */
private val Digit = 10.dp
/** Left of the notes: the strings' names and the bar. */
private val TabLeft = 30.dp

/** One line of tab: the strings, the frets, and the playhead if playing. */
@Composable
private fun TabLineView(line: TabLine, strings: List<String>, digits: Int, playhead: Float?, onClick: () -> Unit) {
    val measurer = rememberTextMeasurer()
    val look = LocalLook.current
    val lineColor = MaterialTheme.colorScheme.outlineVariant
    val text = MaterialTheme.colorScheme.onSurface
    val faint = MaterialTheme.colorScheme.onSurfaceVariant
    val back = if (playhead != null) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.surfaceContainerLow
    val gap = 14.dp
    val n = strings.size
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 2.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(back)
            .clickable(onClick = onClick)
            .padding(vertical = 6.dp),
    ) {
        Text(
            line.label,
            Modifier.padding(start = TabLeft),
            style = MaterialTheme.typography.labelSmall,
            color = if (playhead != null) look.accent else faint,
        )
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(gap * (n - 1).coerceAtLeast(1) + 16.dp),
        ) {
            val g = gap.toPx()
            val left = TabLeft.toPx()
            val span = digits * Digit.toPx()
            val top = 8.dp.toPx()
            val pad = 3.dp.toPx()
            val small = TextStyle(fontSize = 11.sp, color = faint)
            val fret = TextStyle(fontSize = 13.sp, color = text, fontWeight = FontWeight.Medium)
            strings.forEachIndexed { i, name ->
                val y = top + (n - 1 - i) * g
                drawLine(lineColor, Offset(left, y), Offset(size.width - 6.dp.toPx(), y), 1f)
                val m = measurer.measure(name, small)
                drawText(m, topLeft = Offset((left - m.size.width) / 2f, y - m.size.height / 2f))
            }
            drawLine(lineColor, Offset(left, top), Offset(left, top + (n - 1) * g), 2f)
            for (note in line.notes) {
                val m = measurer.measure(note.fret.toString(), fret)
                val x = left + note.x * span
                val y = top + (n - 1 - note.string.toInt()) * g
                drawRect(
                    back,
                    topLeft = Offset(x - m.size.width / 2f - pad, y - m.size.height / 2f),
                    size = Size(m.size.width + 2 * pad, m.size.height.toFloat()),
                )
                drawText(m, topLeft = Offset(x - m.size.width / 2f, y - m.size.height / 2f))
            }
            if (playhead != null) {
                val x = left + playhead * span
                drawLine(look.accent, Offset(x, top - 6.dp.toPx()), Offset(x, top + (n - 1) * g + 6.dp.toPx()), 2.dp.toPx())
            }
        }
    }
}

/** A guitar (or bass) tab of the playing track, following it. */
@Composable
fun TabsSheet(store: Store, state: TabsState, onClose: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    val pos = rememberPosition(store)
    val current = state.lines.indexOfFirst { pos < it.endMs.toLong() }
    val playhead = state.lines.getOrNull(current)?.let { playheadAt(it, pos) }
    val list = rememberLazyListState()
    LaunchedEffect(current, state.lines.size) {
        if (current >= 0) list.animateScrollToItem((current - 1).coerceAtLeast(0))
    }
    SheetPanel(
        store,
        stringResource(R.string.tab_title, state.title),
        onClose,
        actions = {
            if (state.lines.isNotEmpty()) {
                TextButton(onClick = { clipboard.setText(AnnotatedString(state.text)) }) {
                    Text(stringResource(R.string.copy))
                }
            }
        },
    ) {
        val parts = state.parts.size
        val label = Modifier.width(40.dp)
        val labelStyle = MaterialTheme.typography.labelMedium
        val labelColor = MaterialTheme.colorScheme.onSurfaceVariant
        Column(Modifier.padding(start = 20.dp, end = 8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.tab_part), label, style = labelStyle, color = labelColor)
                Dropdown(
                    state.parts,
                    state.part.toInt(),
                    enabled = parts > 1 && !state.busy,
                    modifier = Modifier.weight(1f),
                ) { i -> store.send { setTabPart(i.toUInt()) } }
                IconButton(
                    onClick = {
                        val i = Math.floorMod(state.part.toInt() - 1, parts.coerceAtLeast(1))
                        store.send { setTabPart(i.toUInt()) }
                    },
                    enabled = parts > 1 && !state.busy,
                ) { Icon(ChippyIcons.ChevronLeft, stringResource(R.string.tab_previous_part)) }
                IconButton(
                    onClick = {
                        val i = Math.floorMod(state.part.toInt() + 1, parts.coerceAtLeast(1))
                        store.send { setTabPart(i.toUInt()) }
                    },
                    enabled = parts > 1 && !state.busy,
                ) { Icon(ChippyIcons.ChevronRight, stringResource(R.string.tab_next_part)) }
            }
            Row(
                Modifier.padding(top = 6.dp, bottom = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(stringResource(R.string.tab_instrument), label, style = labelStyle, color = labelColor)
                Dropdown(
                    state.tunings,
                    state.tuning.toInt(),
                    enabled = parts > 0 && !state.busy,
                    modifier = Modifier.weight(1f),
                ) { i -> store.send { setTabTuning(i.toUInt()) } }
                // Lined up with the part's dropdown above.
                Spacer(Modifier.width(96.dp))
            }
        }
        if (state.busy || state.lines.isEmpty()) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(24.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    if (state.busy) stringResource(R.string.tab_busy) else state.message,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            return@SheetPanel
        }
        BoxWithConstraints(Modifier.fillMaxSize()) {
            // As many digits as fit beside the strings' names: the engine
            // lays the lines out so no two fret numbers touch.
            val digits = floor(((maxWidth - 24.dp - TabLeft - 12.dp) / Digit).toDouble()).toInt().coerceAtLeast(12)
            LaunchedEffect(digits) { store.send { setTabWidth(digits.toFloat()) } }
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                itemsIndexed(state.lines) { i, line ->
                    TabLineView(line, state.strings, digits, if (i == current) playhead else null) {
                        store.send { seekTo(line.startMs) }
                    }
                }
                if (state.detail.isNotEmpty()) {
                    item {
                        Text(
                            state.detail,
                            Modifier.padding(horizontal = 24.dp, vertical = 12.dp),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}
