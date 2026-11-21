package io.github.singularity098.chippy.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
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
import io.github.singularity098.chippy.core.PlayState
import io.github.singularity098.chippy.core.TabLine
import io.github.singularity098.chippy.core.TabsState

/** Where playback is, in ms, followed every display frame. */
@Composable
private fun rememberPosition(store: Store): Long {
    val now by store.now.collectAsStateWithLifecycle()
    val frame = rememberFrame(store, now?.state == PlayState.PLAYING)
    return frame?.positionMs?.toLong() ?: 0L
}

/** The playing track's lyrics; timed ones follow the song. */
@OptIn(ExperimentalMaterial3Api::class)
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
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Row(Modifier.padding(horizontal = 24.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                stringResource(R.string.lyrics_title, info.title),
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (info.lines.isNotEmpty()) {
                TextButton(onClick = {
                    clipboard.setText(AnnotatedString(info.lines.joinToString("\n") { it.text }))
                }) { Text(stringResource(R.string.copy)) }
            }
        }
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
                Modifier
                    .padding(24.dp)
                    .navigationBarsPadding(),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            return@ModalBottomSheet
        }
        LazyColumn(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.85f)
                .navigationBarsPadding(),
            state = list,
        ) {
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

/** Previous / next around a choice ("Voice 3 · Flute"). */
@Composable
private fun Choice(label: String, text: String, enabled: Boolean, onStep: (Int) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(end = 4.dp),
        )
        IconButton(onClick = { onStep(-1) }, enabled = enabled) {
            Icon(ChippyIcons.ChevronLeft, null)
        }
        Text(
            text,
            Modifier.weight(1f),
            style = MaterialTheme.typography.titleSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = LocalLook.current.accent,
        )
        IconButton(onClick = { onStep(1) }, enabled = enabled) {
            Icon(ChippyIcons.ChevronRight, null)
        }
    }
}

/** One line of tab: the strings, the frets, and the playhead if playing. */
@Composable
private fun TabLineView(line: TabLine, strings: List<String>, playhead: Float?, onClick: () -> Unit) {
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
            Modifier.padding(start = 30.dp),
            style = MaterialTheme.typography.labelSmall,
            color = if (playhead != null) look.accent else faint,
        )
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(gap * (n - 1).coerceAtLeast(1) + 16.dp),
        ) {
            val g = gap.toPx()
            val left = 30.dp.toPx()
            val span = size.width - left - 16.dp.toPx()
            val top = 8.dp.toPx()
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
                val x = left + 6.dp.toPx() + note.x * span
                val y = top + (n - 1 - note.string.toInt()) * g
                drawRect(
                    back,
                    topLeft = Offset(x - m.size.width / 2f - 2f, y - m.size.height / 2f),
                    size = Size(m.size.width + 4f, m.size.height.toFloat()),
                )
                drawText(m, topLeft = Offset(x - m.size.width / 2f, y - m.size.height / 2f))
            }
            if (playhead != null) {
                val x = left + 6.dp.toPx() + playhead * span
                drawLine(look.accent, Offset(x, top - 6.dp.toPx()), Offset(x, top + (n - 1) * g + 6.dp.toPx()), 2.dp.toPx())
            }
        }
    }
}

/** A guitar (or bass) tab of the playing track, following it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TabsSheet(store: Store, state: TabsState, onClose: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    val pos = rememberPosition(store)
    val current = state.lines.indexOfFirst { pos < it.endMs.toLong() }
    val playhead = state.lines.getOrNull(current)?.let {
        ((pos - it.startMs.toLong()).toFloat() / (it.endMs - it.startMs).toFloat().coerceAtLeast(1f)).coerceIn(0f, 1f)
    }
    val list = rememberLazyListState()
    LaunchedEffect(current, state.lines.size) {
        if (current >= 0) list.animateScrollToItem((current - 1).coerceAtLeast(0))
    }
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Row(Modifier.padding(horizontal = 24.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                stringResource(R.string.tab_title, state.title),
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (state.lines.isNotEmpty()) {
                TextButton(onClick = { clipboard.setText(AnnotatedString(state.text)) }) {
                    Text(stringResource(R.string.copy))
                }
            }
        }
        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(0.dp)) {
            val parts = state.parts.size
            Choice(
                stringResource(R.string.tab_part),
                state.parts.getOrNull(state.part.toInt())?.let { "$it (${state.part.toInt() + 1}/$parts)" } ?: "–",
                enabled = parts > 1 && !state.busy,
            ) { d ->
                val i = Math.floorMod(state.part.toInt() + d, parts.coerceAtLeast(1))
                store.send { setTabPart(i.toUInt()) }
            }
            Choice(
                stringResource(R.string.tab_instrument),
                state.tunings.getOrNull(state.tuning.toInt()) ?: "",
                enabled = parts > 0 && !state.busy,
            ) { d ->
                val i = Math.floorMod(state.tuning.toInt() + d, state.tunings.size.coerceAtLeast(1))
                store.send { setTabTuning(i.toUInt()) }
            }
        }
        if (state.busy || state.lines.isEmpty()) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(24.dp)
                    .navigationBarsPadding(),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    if (state.busy) stringResource(R.string.tab_busy) else state.message,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            return@ModalBottomSheet
        }
        LazyColumn(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.8f)
                .navigationBarsPadding(),
            state = list,
        ) {
            itemsIndexed(state.lines) { i, line ->
                TabLineView(line, state.strings, if (i == current) playhead else null) {
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
