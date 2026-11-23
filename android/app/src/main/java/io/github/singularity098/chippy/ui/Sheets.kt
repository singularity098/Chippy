package io.github.singularity098.chippy.ui

import androidx.compose.animation.Crossfade
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
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
import androidx.compose.runtime.saveable.rememberSaveable
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
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
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
import io.github.singularity098.chippy.core.Fretboard
import io.github.singularity098.chippy.core.TabBar
import io.github.singularity098.chippy.core.TabLine
import io.github.singularity098.chippy.core.TabPart
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
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable(enabled = line.ms >= 0) { store.send { seekTo(line.ms.toULong()) } }
                        .padding(horizontal = 24.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    // When it is sung, faintly beside it: "[1:23]".
                    if (line.ms >= 0) {
                        val s = line.ms / 1000
                        Text(
                            "[%d:%02d]".format(s / 60, s % 60),
                            Modifier.width(52.dp),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.35f),
                        )
                    }
                    Text(
                        line.text.ifEmpty { " " },
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

/** Mute (M) or solo (S) on a row of the list of parts. */
@Composable
private fun MixToggle(letter: String, on: Boolean, lit: Color, description: String, onClick: () -> Unit) {
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(6.dp),
        color = if (on) lit else Color.Transparent,
        border = BorderStroke(1.dp, if (on) lit else MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier
            .padding(start = 6.dp)
            .size(width = 34.dp, height = 30.dp)
            .semantics { contentDescription = description },
    ) {
        Box(contentAlignment = Alignment.Center) {
            Text(
                letter,
                style = MaterialTheme.typography.labelLarge,
                color = if (on) Color.Black else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/**
 * The part shown, and below it on a tap every part: each with an icon for
 * what it plays, and buttons to mute it or solo it. The drums are listed
 * to mute and solo, but can't be shown as tab.
 */
@Composable
private fun PartDropdown(
    state: TabsState,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    store: Store,
    onPick: (Int) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val usable = enabled && state.mix.isNotEmpty()
    val shown = state.mix.getOrNull(state.part.toInt())?.takeIf { it.pick >= 0 }
    val look = LocalLook.current
    val faint = MaterialTheme.colorScheme.onSurfaceVariant
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
                Modifier.padding(start = 10.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (shown != null) {
                    Icon(
                        pathIcon(shown.iconLine, shown.iconSolid),
                        shown.kind,
                        Modifier
                            .padding(end = 8.dp)
                            .size(20.dp),
                        tint = if (shown.audible) look.accent else faint,
                    )
                }
                Text(
                    shown?.name ?: "–",
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = if (usable && shown?.audible != false) look.accent else faint,
                )
                if (state.anyMuted) {
                    Box(
                        Modifier
                            .padding(end = 6.dp)
                            .size(8.dp)
                            .clip(CircleShape)
                            .background(Color(0xFFF0C040)),
                    )
                }
                Icon(ChippyIcons.ChevronDown, null, Modifier.size(18.dp), tint = faint)
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            state.mix.forEachIndexed { i, p ->
                PartRow(state, p, i, store) {
                    open = false
                    onPick(p.pick)
                }
            }
            Row(
                Modifier
                    .widthIn(max = 360.dp)
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(if (state.canMute) R.string.tab_mute_hint else R.string.tab_mute_needs_midi),
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (state.canMute) faint else Color(0xFFF0C040),
                )
                if (state.anyMuted) {
                    TextButton(onClick = { store.send { clearTabMutes() } }) {
                        Text(stringResource(R.string.tab_clear_mutes))
                    }
                }
            }
        }
    }
}

/** A row of the list of parts: pick it (unless it is the drums), mute it, solo it. */
@Composable
private fun PartRow(state: TabsState, p: TabPart, i: Int, store: Store, onPick: () -> Unit) {
    val look = LocalLook.current
    val faint = MaterialTheme.colorScheme.onSurfaceVariant
    DropdownMenuItem(
        enabled = p.pick >= 0,
        text = {
            Text(
                if (p.pick >= 0) p.name else stringResource(R.string.tab_no_tab, p.name),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                color = if (p.audible) MaterialTheme.colorScheme.onSurface else faint.copy(alpha = 0.6f),
            )
        },
        onClick = onPick,
        leadingIcon = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (p.pick >= 0 && p.pick == state.part.toInt()) {
                    Icon(ChippyIcons.Check, null, Modifier.size(20.dp), tint = look.accent)
                } else {
                    Spacer(Modifier.size(20.dp))
                }
                Icon(
                    pathIcon(p.iconLine, p.iconSolid),
                    p.kind,
                    Modifier
                        .padding(start = 6.dp)
                        .size(24.dp),
                    tint = if (p.audible) MaterialTheme.colorScheme.onSurface else faint.copy(alpha = 0.6f),
                )
            }
        },
        trailingIcon = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                MixToggle("M", p.muted, Color(0xFFE5484D), stringResource(R.string.tab_mute, p.name)) {
                    store.send { toggleTabMute(i.toUInt()) }
                }
                MixToggle("S", p.soloed, Color(0xFFF0C040), stringResource(R.string.tab_solo, p.name)) {
                    store.send { toggleTabSolo(i.toUInt()) }
                }
            }
        },
    )
}

/** A note on the fretboard at a moment. */
private data class Dot(val string: Int, val fret: Int, val name: String, val now: Boolean)

/** The notes of the bar playing at `pos`, those sounding marked. */
private fun dotsAt(f: Fretboard, pos: Long): List<Dot> {
    val g = f.groups.indexOfFirst { pos >= it.startMs.toLong() && pos < it.endMs.toLong() }
    if (g < 0) return emptyList()
    val out = LinkedHashMap<Pair<Int, Int>, Dot>()
    for (n in f.notes) {
        if (n.group.toInt() != g) continue
        val key = n.string.toInt() to n.fret.toInt()
        val now = pos >= n.startMs.toLong() && pos < n.endMs.toLong()
        val had = out[key]
        out[key] = if (had == null) Dot(key.first, key.second, n.name, now) else had.copy(now = had.now || now)
    }
    return out.values.toList()
}

/**
 * A guitar's neck, the strings across it (the highest on top, as in the
 * tab), with the notes of the bar playing on it: those sounding now lit,
 * the rest dimmed, each with its name.
 */
@Composable
private fun FretboardView(f: Fretboard, pos: Long) {
    val measurer = rememberTextMeasurer()
    val look = LocalLook.current
    val lineColor = MaterialTheme.colorScheme.outlineVariant
    val faint = MaterialTheme.colorScheme.onSurfaceVariant
    val neckColor = MaterialTheme.colorScheme.surfaceContainerHigh
    val dimDot = MaterialTheme.colorScheme.surfaceContainerHighest
    val dots = dotsAt(f, pos)
    val n = f.strings.size.coerceAtLeast(1)
    val frets = f.frets.toInt().coerceAtLeast(1)
    val gap = 20.dp
    Canvas(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .height(gap * (n - 1) + 34.dp),
    ) {
        val g = gap.toPx()
        val names = 22.dp.toPx()
        val open = 24.dp.toPx()
        val left = names + open
        val fw = (size.width - left - 4.dp.toPx()) / frets
        val top = 10.dp.toPx()
        val h = (n - 1) * g
        val small = TextStyle(fontSize = 11.sp, color = faint)
        val tiny = TextStyle(fontSize = 9.sp, color = faint)
        drawRoundRect(
            neckColor,
            topLeft = Offset(left, top - 6.dp.toPx()),
            size = Size(fw * frets, h + 12.dp.toPx()),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(4.dp.toPx()),
        )
        val inlay = 3.5.dp.toPx()
        for (fr in listOf(3, 5, 7, 9, 15, 17, 19, 21)) {
            if (fr <= frets) drawCircle(lineColor, inlay, Offset(left + (fr - 0.5f) * fw, top + h / 2))
        }
        for (fr in listOf(12, 24)) {
            if (fr <= frets) {
                drawCircle(lineColor, inlay, Offset(left + (fr - 0.5f) * fw, top + h / 2 - g))
                drawCircle(lineColor, inlay, Offset(left + (fr - 0.5f) * fw, top + h / 2 + g))
            }
        }
        drawLine(faint, Offset(left, top - 6.dp.toPx()), Offset(left, top + h + 6.dp.toPx()), 4.dp.toPx())
        for (fr in 1..frets) {
            val x = left + fr * fw
            drawLine(lineColor, Offset(x, top - 6.dp.toPx()), Offset(x, top + h + 6.dp.toPx()), 2.dp.toPx())
            val m = measurer.measure(fr.toString(), tiny)
            drawText(m, topLeft = Offset(x - fw / 2 - m.size.width / 2f, top + h + 8.dp.toPx()))
        }
        f.strings.forEachIndexed { i, name ->
            val y = top + (n - 1 - i) * g
            drawLine(faint, Offset(names + 4.dp.toPx(), y), Offset(left + fw * frets, y), if (i < 2) 2f else 1.2f)
            val m = measurer.measure(name, small)
            drawText(m, topLeft = Offset((names - m.size.width) / 2f, y - m.size.height / 2f))
        }
        val r = minOf(10.dp.toPx(), g / 2 - 0.5f, fw / 2 - 1.5f)
        for (d in dots) {
            val cx = if (d.fret == 0) names + open / 2 else left + (d.fret - 0.5f) * fw
            val cy = top + (n - 1 - d.string) * g
            if (d.now) {
                drawCircle(look.accent, r, Offset(cx, cy))
            } else {
                drawCircle(dimDot, r, Offset(cx, cy))
                drawCircle(lineColor, r, Offset(cx, cy), style = androidx.compose.ui.graphics.drawscope.Stroke(1.dp.toPx()))
            }
            val style = TextStyle(
                fontSize = (r * (if (d.name.length > 1) 0.95f else 1.15f)).toSp(),
                color = if (d.now) Color.Black else faint,
                fontWeight = FontWeight.Medium,
            )
            val m = measurer.measure(d.name, style)
            drawText(m, topLeft = Offset(cx - m.size.width / 2f, cy - m.size.height / 2f))
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

/**
 * One line of tab: the strings, the frets, the bar lines, and the playhead
 * if playing. A tap on a bar chooses it (a long press, the bars up to it); a
 * double tap plays from it. Without bars, a tap plays from the line.
 */
@Composable
private fun TabLineView(
    line: TabLine,
    strings: List<String>,
    digits: Int,
    playhead: Float?,
    onClick: () -> Unit,
    onBar: (UInt, Boolean) -> Unit,
    onBarSeek: (UInt) -> Unit,
) {
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
            .pointerInput(line, digits) {
                val barAt = { x: Float ->
                    val f = (x - TabLeft.toPx()) / (digits * Digit.toPx())
                    line.bars.firstOrNull { f >= it.x0 && f < it.x1 }
                        ?: line.bars.lastOrNull()?.takeIf { f >= it.x1 }
                        ?: line.bars.firstOrNull()?.takeIf { f < it.x0 }
                }
                detectTapGestures(
                    onTap = { o -> barAt(o.x)?.let { onBar(it.index, false) } ?: onClick() },
                    onDoubleTap = { o -> barAt(o.x)?.let { onBarSeek(it.index) } ?: onClick() },
                    onLongPress = { o -> barAt(o.x)?.let { onBar(it.index, true) } },
                )
            }
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
            val softFret = TextStyle(fontSize = 13.sp, color = faint, fontWeight = FontWeight.Normal)
            strings.forEachIndexed { i, name ->
                val y = top + (n - 1 - i) * g
                drawLine(lineColor, Offset(left, y), Offset(size.width - 6.dp.toPx(), y), 1f)
                val m = measurer.measure(name, small)
                drawText(m, topLeft = Offset((left - m.size.width) / 2f, y - m.size.height / 2f))
            }
            drawLine(lineColor, Offset(left, top), Offset(left, top + (n - 1) * g), 2f)
            // Bar lines and numbers, and the chosen bars marked above and
            // below the strings.
            val number = TextStyle(fontSize = 9.sp, color = faint)
            val marked = TextStyle(fontSize = 9.sp, color = look.accent)
            for (bar in line.bars) {
                val x0 = left + bar.x0 * span
                val x1 = left + bar.x1 * span
                if (bar.line && bar.x0 > 0f) {
                    drawLine(faint, Offset(x0, top), Offset(x0, top + (n - 1) * g), 1.5.dp.toPx())
                }
                if (bar.x0 > 0f || bar.selected || bar.nudged) {
                    val m = measurer.measure(bar.label, if (bar.selected || bar.nudged) marked else number)
                    drawText(m, topLeft = Offset(x0 + 2.dp.toPx(), top - m.size.height - 1.dp.toPx()))
                }
                if (bar.selected) {
                    val w = 3.dp.toPx()
                    drawLine(look.accent, Offset(x0 + w, top - 4.dp.toPx()), Offset(x1 - w, top - 4.dp.toPx()), w)
                    val bottom = top + (n - 1) * g + 4.dp.toPx()
                    drawLine(look.accent, Offset(x0 + w, bottom), Offset(x1 - w, bottom), w)
                }
            }
            for (note in line.notes) {
                val m = if (note.soft) {
                    measurer.measure("(${note.fret})", softFret)
                } else {
                    measurer.measure(note.fret.toString(), fret)
                }
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
    // Another part or instrument keeps the same bars in view: the time the
    // top line starts at, kept until the new lines come.
    val keep = remember { mutableStateOf<Long?>(null) }
    // The line playing when the view was put back: no need to follow it.
    val kept = remember { intArrayOf(-2) }
    val keepView = {
        keep.value = state.lines.getOrNull(list.firstVisibleItemIndex)?.startMs?.toLong()
    }
    LaunchedEffect(state.lines) {
        val ms = keep.value ?: return@LaunchedEffect
        val i = state.lines.indexOfFirst { ms + 1 < it.endMs.toLong() }
        kept[0] = current
        keep.value = null
        if (i >= 0) list.scrollToItem(i, list.firstVisibleItemScrollOffset)
    }
    LaunchedEffect(current, state.lines.size) {
        val held = kept[0] == current
        kept[0] = -2
        if (held) return@LaunchedEffect
        if (current >= 0) list.animateScrollToItem((current - 1).coerceAtLeast(0))
    }
    // The fretboard: hidden at first.
    var fretboard by rememberSaveable { mutableStateOf(false) }
    SheetPanel(
        store,
        stringResource(R.string.tab_title, state.title),
        onClose,
        actions = {
            if (state.lines.isNotEmpty() && state.fretboard != null) {
                IconButton(onClick = { fretboard = !fretboard }) {
                    Icon(
                        ChippyIcons.Fretboard,
                        stringResource(if (fretboard) R.string.tab_fretboard_hide else R.string.tab_fretboard_show),
                        tint = if (fretboard) LocalLook.current.accent else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (state.lines.isNotEmpty()) {
                TextButton(onClick = { clipboard.setText(AnnotatedString(state.text)) }) {
                    Text(stringResource(R.string.copy))
                }
            }
            if (state.lines.isNotEmpty() && state.hasBars) {
                var saving by remember { mutableStateOf(false) }
                Box {
                    TextButton(onClick = { saving = true }) { Text(stringResource(R.string.tab_save)) }
                    DropdownMenu(expanded = saving, onDismissRequest = { saving = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.tab_save_musicxml)) },
                            onClick = { saving = false; store.send { saveTab(false) } },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.tab_save_gp)) },
                            onClick = { saving = false; store.send { saveTab(true) } },
                        )
                    }
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
                PartDropdown(
                    state,
                    enabled = !state.busy,
                    modifier = Modifier.weight(1f),
                    store = store,
                ) { i -> keepView(); store.send { setTabPart(i.toUInt()) } }
                IconButton(
                    onClick = {
                        val i = Math.floorMod(state.part.toInt() - 1, parts.coerceAtLeast(1))
                        keepView()
                        store.send { setTabPart(i.toUInt()) }
                    },
                    enabled = parts > 1 && !state.busy,
                ) { Icon(ChippyIcons.ChevronLeft, stringResource(R.string.tab_previous_part)) }
                IconButton(
                    onClick = {
                        val i = Math.floorMod(state.part.toInt() + 1, parts.coerceAtLeast(1))
                        keepView()
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
                ) { i -> keepView(); store.send { setTabTuning(i.toUInt()) } }
                // Under the part's arrows: the part merged with another.
                var merging by remember { mutableStateOf(false) }
                Box(Modifier.width(96.dp), contentAlignment = Alignment.Center) {
                    TextButton(
                        onClick = { merging = true },
                        enabled = state.mergeItems.isNotEmpty() && !state.busy,
                    ) { Text(stringResource(R.string.tab_merge)) }
                    DropdownMenu(expanded = merging, onDismissRequest = { merging = false }) {
                        state.mergeItems.forEachIndexed { i, item ->
                            DropdownMenuItem(
                                text = { Text(item) },
                                onClick = {
                                    merging = false
                                    keepView()
                                    store.send { mergeTabPart(i.toUInt()) }
                                },
                            )
                        }
                    }
                }
            }
            // Moved by hand: by octaves, and along the neck.
            val ready = !state.busy && state.lines.isNotEmpty()
            val signed = { n: Int -> if (n > 0) "+$n" else if (n < 0) "−${-n}" else "0" }
            Row(Modifier.padding(bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.tab_octave), style = labelStyle, color = labelColor)
                IconButton(
                    onClick = { store.send { shiftTabOctave(-1) } },
                    enabled = ready && state.octave > -3,
                ) { Icon(ChippyIcons.Minus, stringResource(R.string.tab_octave_down)) }
                Text(
                    signed(state.octave),
                    style = labelStyle,
                    color = if (state.octave != 0) LocalLook.current.accent else labelColor,
                )
                IconButton(
                    onClick = { store.send { shiftTabOctave(1) } },
                    enabled = ready && state.octave < 3,
                ) { Icon(ChippyIcons.Plus, stringResource(R.string.tab_octave_up)) }
                Spacer(Modifier.width(8.dp))
                Text(
                    state.selection.ifEmpty { stringResource(R.string.tab_neck) },
                    style = labelStyle,
                    color = if (state.selection.isEmpty()) labelColor else LocalLook.current.accent,
                )
                IconButton(
                    onClick = { store.send { nudgeTab(-1) } },
                    enabled = ready && state.canDown,
                ) { Icon(ChippyIcons.ChevronLeft, stringResource(R.string.tab_neck_down)) }
                Text(
                    signed(state.position),
                    style = labelStyle,
                    color = if (state.position != 0) LocalLook.current.accent else labelColor,
                )
                IconButton(
                    onClick = { store.send { nudgeTab(1) } },
                    enabled = ready && state.canUp,
                ) { Icon(ChippyIcons.ChevronRight, stringResource(R.string.tab_neck_up)) }
                Spacer(Modifier.weight(1f))
                IconButton(
                    onClick = { store.send { resetTabAdjust() } },
                    enabled = ready && state.adjusted,
                ) {
                    Icon(
                        ChippyIcons.Refresh,
                        stringResource(if (state.selection.isEmpty()) R.string.tab_reset else R.string.tab_reset_bars),
                    )
                }
            }
            // Where the bars fall, put right by hand: the time signature,
            // the beat twice or half as fast, the bar lines a beat earlier
            // or later.
            if (state.hasBars) {
                Row(Modifier.padding(bottom = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.tab_bars), label, style = labelStyle, color = labelColor)
                    Dropdown(
                        state.sigs,
                        state.sig.toInt(),
                        enabled = ready,
                        modifier = Modifier.width(104.dp),
                    ) { i -> store.send { setTabSig(i.toUInt()) } }
                    IconButton(
                        onClick = { store.send { stepTabSpeed(-1) } },
                        enabled = ready && state.speed > -2,
                    ) { Icon(ChippyIcons.Minus, stringResource(R.string.tab_speed_down)) }
                    Text(
                        when (state.speed) { 1 -> "×2"; 2 -> "×4"; -1 -> "½"; -2 -> "¼"; else -> "×1" },
                        style = labelStyle,
                        color = if (state.speed != 0) LocalLook.current.accent else labelColor,
                    )
                    IconButton(
                        onClick = { store.send { stepTabSpeed(1) } },
                        enabled = ready && state.speed < 2,
                    ) { Icon(ChippyIcons.Plus, stringResource(R.string.tab_speed_up)) }
                    IconButton(
                        onClick = { store.send { shiftTabBars(-1) } },
                        enabled = ready,
                    ) { Icon(ChippyIcons.ChevronLeft, stringResource(R.string.tab_bars_earlier)) }
                    IconButton(
                        onClick = { store.send { shiftTabBars(1) } },
                        enabled = ready,
                    ) { Icon(ChippyIcons.ChevronRight, stringResource(R.string.tab_bars_later)) }
                    Spacer(Modifier.weight(1f))
                    IconButton(
                        onClick = { store.send { resetTabMeter() } },
                        enabled = ready && state.meterAdjusted,
                    ) { Icon(ChippyIcons.Refresh, stringResource(R.string.tab_bars_reset)) }
                }
                Text(
                    state.meterText,
                    Modifier.padding(start = 40.dp, bottom = 6.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = labelColor,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
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
        val board = state.fretboard
        if (fretboard && board != null) {
            FretboardView(board, pos)
        }
        BoxWithConstraints(Modifier.fillMaxSize()) {
            // As many digits as fit beside the strings' names: the engine
            // lays the lines out so no two fret numbers touch.
            val digits = floor(((maxWidth - 24.dp - TabLeft - 12.dp) / Digit).toDouble()).toInt().coerceAtLeast(12)
            LaunchedEffect(digits) { store.send { setTabWidth(digits.toFloat()) } }
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                itemsIndexed(state.lines) { i, line ->
                    TabLineView(
                        line,
                        state.strings,
                        digits,
                        if (i == current) playhead else null,
                        onClick = { store.send { seekTo(line.startMs) } },
                        onBar = { bar, extend -> store.send { selectTabBar(bar, extend) } },
                        onBarSeek = { bar -> store.send { seekTabBar(bar) } },
                    )
                }
                if (state.detail.isNotEmpty() || state.hasBars) {
                    item {
                        Text(
                            listOf(state.detail, if (state.hasBars) stringResource(R.string.tab_bars_hint) else "")
                                .filter { it.isNotEmpty() }
                                .joinToString(" "),
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
