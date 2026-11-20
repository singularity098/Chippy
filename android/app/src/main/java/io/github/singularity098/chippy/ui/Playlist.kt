package io.github.singularity098.chippy.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.PlRow
import io.github.singularity098.chippy.core.PlaylistState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaylistScreen(
    store: Store,
    state: PlaylistState?,
    onClose: () -> Unit,
    onOpenBrowser: () -> Unit,
    onInfo: (Int) -> Unit,
    onExport: (Int?) -> Unit,
) {
    BackHandler(onBack = onClose)
    var confirmClear by remember { mutableStateOf(false) }
    val rows = state?.rows.orEmpty()
    val listState = rememberLazyListState()
    // Another track started: bring it into view.
    LaunchedEffect(state?.revealSerial) {
        val p = state?.playing ?: return@LaunchedEffect
        if (p < 0) return@LaunchedEffect
        val visible = listState.layoutInfo.visibleItemsInfo
        if (visible.none { it.index == p } || visible.isEmpty()) {
            listState.animateScrollToItem((p - 2).coerceAtLeast(0))
        }
    }
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            TopAppBar(
                scrollBehavior = scrollBehavior,
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface,
                    scrolledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
                ),
                navigationIcon = {
                    IconButton(onClick = onClose) { Icon(ChippyIcons.ChevronDown, stringResource(R.string.close)) }
                },
                title = {
                    Column {
                        Text(stringResource(R.string.playlist))
                        if (!state?.summary.isNullOrEmpty()) {
                            Text(
                                state!!.summary,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = { onExport(null) }, enabled = rows.isNotEmpty()) {
                        Icon(ChippyIcons.Export, stringResource(R.string.export_all))
                    }
                    IconButton(onClick = { confirmClear = true }, enabled = rows.isNotEmpty()) {
                        Icon(ChippyIcons.Trash, stringResource(R.string.clear_playlist))
                    }
                },
            )
        },
    ) { padding ->
        if (rows.isEmpty()) {
            Box(
                Modifier
                    .fillMaxSize()
                    .padding(padding),
                contentAlignment = Alignment.Center,
            ) {
                EmptyMessage(
                    ChippyIcons.Queue,
                    stringResource(R.string.playlist_empty),
                    stringResource(R.string.playlist_empty_hint),
                ) {
                    FilledTonalButton(onClick = onOpenBrowser) {
                        Icon(ChippyIcons.Folder, null, Modifier.size(18.dp))
                        Text(stringResource(R.string.file_browser), Modifier.padding(start = 8.dp))
                    }
                }
            }
        } else {
            LazyColumn(
                state = listState,
                contentPadding = PaddingValues(bottom = 24.dp),
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding),
            ) {
                itemsIndexed(rows, key = { i, r -> "$i:${r.title}" }) { i, row ->
                    PlaylistRow(
                        i, row,
                        onPlay = { store.send { playRow(i.toUInt()) } },
                        onRemove = { store.send { removeRow(i.toUInt()) } },
                        onReveal = { store.send { revealRow(i.toUInt()) } },
                        onInfo = { onInfo(i) },
                        onExport = { onExport(i) },
                    )
                }
            }
        }
    }
    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text(stringResource(R.string.clear_playlist_title)) },
            text = { Text(stringResource(R.string.clear_playlist_text, state?.summary ?: "")) },
            confirmButton = {
                TextButton(onClick = {
                    confirmClear = false
                    store.send { clearPlaylist() }
                }) { Text(stringResource(R.string.clear)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmClear = false }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PlaylistRow(
    index: Int,
    row: PlRow,
    onPlay: () -> Unit,
    onRemove: () -> Unit,
    onReveal: () -> Unit,
    onInfo: () -> Unit,
    onExport: () -> Unit,
) {
    val look = LocalLook.current
    var menu by remember { mutableStateOf(false) }
    val dismiss = rememberSwipeToDismissBoxState(
        confirmValueChange = {
            if (it != SwipeToDismissBoxValue.Settled) onRemove()
            it != SwipeToDismissBoxValue.Settled
        },
    )
    SwipeToDismissBox(
        state = dismiss,
        backgroundContent = {
            Row(
                Modifier
                    .fillMaxSize()
                    .background(MaterialTheme.colorScheme.error.copy(alpha = 0.18f))
                    .padding(horizontal = 24.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = if (dismiss.dismissDirection == SwipeToDismissBoxValue.StartToEnd)
                    Arrangement.Start else Arrangement.End,
            ) {
                Icon(ChippyIcons.Trash, stringResource(R.string.remove), tint = MaterialTheme.colorScheme.error)
            }
        },
    ) {
        ListItem(
            modifier = Modifier
                .background(MaterialTheme.colorScheme.surface)
                .clickable(onClick = onPlay)
                .then(
                    if (row.playing) Modifier.background(look.paintB.copy(alpha = 0.10f)) else Modifier
                ),
            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
            leadingContent = {
                Box(Modifier.width(32.dp), contentAlignment = Alignment.Center) {
                    if (row.playing) {
                        PaintedIcon(ChippyIcons.Bars, Modifier.size(22.dp), stringResource(R.string.playing))
                    } else {
                        Text(
                            "${index + 1}",
                            style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            textAlign = TextAlign.Center,
                        )
                    }
                }
            },
            headlineContent = {
                Text(
                    row.title,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = when {
                        row.playing -> look.accent
                        row.pending -> MaterialTheme.colorScheme.onSurfaceVariant
                        else -> MaterialTheme.colorScheme.onSurface
                    },
                )
            },
            supportingContent = if (row.detail.isNotEmpty()) {
                { Text(row.detail, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            } else null,
            trailingContent = {
                Box {
                    IconButton(onClick = { menu = true }) {
                        Icon(ChippyIcons.More, stringResource(R.string.more))
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.show_in_browser)) },
                            leadingIcon = { Icon(ChippyIcons.Folder, null) },
                            onClick = { menu = false; onReveal() },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.properties)) },
                            leadingIcon = { Icon(ChippyIcons.Info, null) },
                            onClick = { menu = false; onInfo() },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.export)) },
                            leadingIcon = { Icon(ChippyIcons.Export, null) },
                            onClick = { menu = false; onExport() },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.remove)) },
                            leadingIcon = { Icon(ChippyIcons.Trash, null) },
                            onClick = { menu = false; onRemove() },
                        )
                    }
                }
            },
        )
    }
}
