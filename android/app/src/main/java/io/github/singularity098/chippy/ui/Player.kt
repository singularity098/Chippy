package io.github.singularity098.chippy.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.basicMarquee
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.Frame
import io.github.singularity098.chippy.core.NowState
import io.github.singularity098.chippy.core.PlayState
import io.github.singularity098.chippy.core.RepeatMode
import kotlinx.coroutines.delay

/**
 * The position and the spectrum, asked of the engine on every display
 * frame while something moves; a few times a second once all is still.
 */
@Composable
fun rememberFrame(store: Store, playing: Boolean): Frame? {
    var frame by remember { mutableStateOf<Frame?>(null) }
    LaunchedEffect(playing) {
        while (true) {
            withFrameNanos { }
            val f = store.engine?.frame() ?: continue
            frame = f
            val still = f.bars.all { it <= 0.001f } && f.peaks.all { it <= 0.001f }
            if (!playing && still) delay(250)
        }
    }
    return frame
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlayerScreen(
    store: Store,
    now: NowState?,
    onOpenBrowser: () -> Unit,
    onOpenPlaylist: () -> Unit,
    onOpenSettings: () -> Unit,
    onLyrics: () -> Unit = {},
    onTabs: () -> Unit = {},
) {
    val cover by store.cover.collectAsStateWithLifecycle()
    val playing = now?.state == PlayState.PLAYING
    val frame = rememberFrame(store, playing)
    Scaffold(
        containerColor = Color.Transparent,
        topBar = {
            CenterAlignedTopAppBar(
                colors = TopAppBarDefaults.centerAlignedTopAppBarColors(containerColor = Color.Transparent),
                navigationIcon = {
                    IconButton(onClick = onOpenBrowser) {
                        Icon(ChippyIcons.Folder, stringResource(R.string.file_browser))
                    }
                },
                title = {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(
                            stringResource(R.string.now_playing).uppercase(),
                            style = MaterialTheme.typography.labelMedium,
                            letterSpacing = 1.8.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (!now?.location.isNullOrEmpty()) {
                            Text(
                                now!!.location,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.7f),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = onLyrics, enabled = now?.hasTrack == true) {
                        Icon(ChippyIcons.Lyrics, stringResource(R.string.lyrics))
                    }
                    IconButton(onClick = onTabs, enabled = now?.hasTrack == true) {
                        Icon(ChippyIcons.Guitar, stringResource(R.string.guitar_tab))
                    }
                    IconButton(onClick = onOpenPlaylist) {
                        Icon(ChippyIcons.Queue, stringResource(R.string.playlist))
                    }
                    IconButton(onClick = onOpenSettings) {
                        Icon(ChippyIcons.Gear, stringResource(R.string.settings))
                    }
                },
            )
        },
    ) { padding ->
        BoxWithConstraints(
            Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            val wide = maxWidth > maxHeight
            if (wide) {
                Row(
                    Modifier
                        .fillMaxSize()
                        .padding(horizontal = 24.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(32.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    CoverArt(cover, now?.hasTrack == true, Modifier.weight(1f).fillMaxSize())
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                        Controls(store, now, frame, onOpenPlaylist)
                    }
                }
            } else {
                Column(
                    Modifier
                        .fillMaxSize()
                        .padding(horizontal = 24.dp),
                ) {
                    CoverArt(
                        cover,
                        now?.hasTrack == true,
                        Modifier
                            .weight(1f)
                            .fillMaxWidth()
                            .padding(vertical = 12.dp),
                    )
                    Controls(store, now, frame, onOpenPlaylist)
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ColumnScope.Controls(store: Store, now: NowState?, frame: Frame?, onOpenPlaylist: () -> Unit) {
    val look = LocalLook.current
    val haptics = LocalHapticFeedback.current
    val hasTrack = now?.hasTrack == true

    // Title, artist, album; the SoundFont beside them for MIDI.
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(
                if (hasTrack) now!!.title else stringResource(R.string.nothing_playing),
                style = MaterialTheme.typography.headlineSmall,
                maxLines = 1,
                modifier = Modifier
                    .basicMarquee(initialDelayMillis = 2500)
                    .clickable(enabled = hasTrack, onClick = onOpenPlaylist),
            )
            val by = listOfNotNull(
                now?.artist?.takeIf { it.isNotEmpty() },
                now?.album?.takeIf { it.isNotEmpty() },
            ).joinToString("  ·  ")
            Text(
                if (hasTrack) by else stringResource(R.string.nothing_playing_hint),
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (now?.midi == true) {
            SoundFontSwitch(store, now.soundfont)
        }
    }
    if (!now?.info.isNullOrEmpty()) {
        Text(
            now!!.info,
            Modifier.padding(top = 2.dp),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.75f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }

    Spectrum(
        frame?.bars ?: emptyList(),
        frame?.peaks ?: emptyList(),
        Modifier
            .fillMaxWidth()
            .height(52.dp)
            .padding(top = 14.dp),
    )

    // The timeline.
    var dragging by remember { mutableStateOf<Float?>(null) }
    val fraction = dragging ?: frame?.fraction ?: 0f
    Slider(
        value = fraction,
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
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 6.dp)
            .semantics { contentDescription = "Position" },
    )
    Row(Modifier.fillMaxWidth()) {
        val elapsed = dragging?.let { f ->
            now?.durationMs?.let { formatMs((it.toDouble() * f).toLong()) }
        } ?: frame?.elapsed ?: "0:00"
        Text(elapsed, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.weight(1f))
        Text(now?.total ?: "–:––", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }

    // The transport.
    Row(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 14.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconToggleButton(checked = now?.shuffle == true, onCheckedChange = { store.send { toggleShuffle() } }) {
            Icon(
                ChippyIcons.Shuffle,
                stringResource(R.string.shuffle),
                tint = if (now?.shuffle == true) look.accent else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        IconButton(onClick = { store.send { previous() } }, modifier = Modifier.size(56.dp)) {
            Icon(ChippyIcons.Previous, stringResource(R.string.previous), Modifier.size(30.dp))
        }
        PaintedCircle(
            76.dp,
            onClick = {
                haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                store.send { playPause() }
            },
            onClickLabel = stringResource(if (now?.state == PlayState.PLAYING) R.string.pause else R.string.play),
        ) {
            Crossfade(now?.state == PlayState.PLAYING, label = "play") { isPlaying ->
                Icon(
                    if (isPlaying) ChippyIcons.Pause else ChippyIcons.Play,
                    null,
                    Modifier.size(34.dp),
                    tint = Color.Black,
                )
            }
        }
        IconButton(onClick = { store.send { next() } }, modifier = Modifier.size(56.dp)) {
            Icon(ChippyIcons.Next, stringResource(R.string.next), Modifier.size(30.dp))
        }
        val repeat = now?.repeat ?: RepeatMode.OFF
        IconToggleButton(checked = repeat != RepeatMode.OFF, onCheckedChange = { store.send { cycleRepeat() } }) {
            Icon(
                if (repeat == RepeatMode.ONE) ChippyIcons.RepeatOne else ChippyIcons.Repeat,
                stringResource(
                    when (repeat) {
                        RepeatMode.OFF -> R.string.repeat_off
                        RepeatMode.ALL -> R.string.repeat_all
                        RepeatMode.ONE -> R.string.repeat_one
                    }
                ),
                tint = if (repeat != RepeatMode.OFF) look.accent else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }

    // What comes next: a tap shows the playlist.
    AnimatedVisibility(!now?.upNext.isNullOrEmpty(), enter = fadeIn(), exit = fadeOut()) {
        Surface(
            onClick = onOpenPlaylist,
            shape = RoundedCornerShape(16.dp),
            color = MaterialTheme.colorScheme.surfaceContainerLow,
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 16.dp),
        ) {
            Row(
                Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(ChippyIcons.Queue, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.width(12.dp))
                Text(
                    stringResource(R.string.up_next).uppercase(),
                    style = MaterialTheme.typography.labelMedium,
                    letterSpacing = 1.4.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.width(10.dp))
                Text(
                    now?.upNext ?: "",
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Icon(ChippyIcons.ChevronRight, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    if (now?.upNext.isNullOrEmpty()) Spacer(Modifier.height(16.dp))
}

/** ‹ SoundFont › beside a MIDI track's title. */
@Composable
private fun SoundFontSwitch(store: Store, name: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { store.send { cycleSoundfont(-1) } }, modifier = Modifier.size(36.dp)) {
            Icon(ChippyIcons.ChevronLeft, stringResource(R.string.previous_soundfont), Modifier.size(18.dp))
        }
        Box(Modifier.width(88.dp), contentAlignment = Alignment.Center) {
            Text(
                name.ifEmpty { stringResource(R.string.no_soundfont) },
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
            )
        }
        IconButton(onClick = { store.send { cycleSoundfont(1) } }, modifier = Modifier.size(36.dp)) {
            Icon(ChippyIcons.ChevronRight, stringResource(R.string.next_soundfont), Modifier.size(18.dp))
        }
    }
}

/** 61000 → "1:01", as the engine writes times. */
fun formatMs(ms: Long): String {
    val s = ms / 1000
    val h = s / 3600
    val m = (s / 60) % 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}
