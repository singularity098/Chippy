package io.github.singularity098.chippy.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.BrowserState
import io.github.singularity098.chippy.core.FileItem
import io.github.singularity098.chippy.core.FileKind
import kotlinx.coroutines.delay

private fun FileKind.icon(): ImageVector = when (this) {
    FileKind.FOLDER -> ChippyIcons.Folder
    FileKind.ARCHIVE -> ChippyIcons.Archive
    FileKind.AUDIO, FileKind.TRACK -> ChippyIcons.Note
    FileKind.MULTI_TRACK -> ChippyIcons.Disc
    FileKind.IMAGE -> ChippyIcons.Image
    FileKind.OTHER -> ChippyIcons.File
}

private val FileKind.opens: Boolean
    get() = this == FileKind.FOLDER || this == FileKind.ARCHIVE || this == FileKind.MULTI_TRACK

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
fun BrowserScreen(store: Store, state: BrowserState?, onClose: () -> Unit) {
    // Folders opened since the browser came in: Back retraces them, then
    // closes the browser.
    var depth by rememberSaveable { mutableIntStateOf(0) }
    // The field filters this folder, or searches below it.
    var searching by rememberSaveable { mutableStateOf(false) }
    var text by rememberSaveable { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    val haptics = LocalHapticFeedback.current
    val listState = rememberLazyListState()
    var refreshing by remember { mutableStateOf(false) }

    fun navigated() {
        depth++
        text = ""
        searching = false
        focus.clearFocus()
    }

    BackHandler {
        when {
            text.isNotEmpty() -> {
                text = ""
                store.send { if (searching) setSearch("") else setFilter("") }
            }
            depth > 0 -> {
                depth--
                store.send { goBack() }
            }
            else -> onClose()
        }
    }

    // Another folder: to its top, or to the entry to show. (Listing
    // another folder also empties the field, as the engine does.)
    LaunchedEffect(state?.serial) {
        val s = state ?: return@LaunchedEffect
        if (s.filter.isEmpty() && s.search.isEmpty()) {
            text = ""
            searching = false
        }
        if (s.selected >= 0) listState.scrollToItem((s.selected - 2).coerceAtLeast(0))
        else listState.scrollToItem(0)
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
                    IconButton(onClick = onClose) {
                        Icon(ChippyIcons.Back, stringResource(R.string.close_browser))
                    }
                },
                title = {
                    Column {
                        Text(state?.title ?: "", maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            state?.status ?: "",
                            style = MaterialTheme.typography.bodySmall,
                            color = if (state?.statusError == true) MaterialTheme.colorScheme.error
                            else MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                },
                actions = {
                    IconButton(onClick = { navigated(); store.send { goUp() } }, enabled = state?.canUp == true) {
                        Icon(ChippyIcons.Up, stringResource(R.string.up))
                    }
                    IconButton(onClick = { navigated(); store.send { goHome() } }) {
                        Icon(ChippyIcons.Home, stringResource(R.string.home))
                    }
                    IconButton(onClick = { menu = true }) {
                        Icon(ChippyIcons.More, stringResource(R.string.more))
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.refresh)) },
                            leadingIcon = { Icon(ChippyIcons.Refresh, null) },
                            onClick = { menu = false; store.send { refresh() } },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.add_to_music_folders)) },
                            leadingIcon = { Icon(ChippyIcons.Folder, null) },
                            onClick = { menu = false; store.send { addBrowserDir() } },
                        )
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.forward)) },
                            leadingIcon = { Icon(ChippyIcons.Forward, null) },
                            enabled = state?.canForward == true,
                            onClick = { menu = false; depth++; store.send { goForward() } },
                        )
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            // The music folders, to switch between.
            val dirs = state?.musicDirs.orEmpty()
            if (dirs.isNotEmpty()) {
                LazyRow(
                    contentPadding = PaddingValues(horizontal = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    itemsIndexed(dirs) { i, d ->
                        FilterChip(
                            selected = d.current,
                            onClick = { navigated(); store.send { goMusicDir(i.toUInt()) } },
                            label = { Text(d.label) },
                            leadingIcon = if (d.home) {
                                { Icon(ChippyIcons.Home, null, Modifier.size(FilterChipDefaults.IconSize)) }
                            } else null,
                        )
                    }
                }
            }

            // The path, a level a tap.
            val crumbScroll = rememberScrollState()
            LaunchedEffect(state?.crumbs) { crumbScroll.scrollTo(crumbScroll.maxValue) }
            Row(
                Modifier
                    .fillMaxWidth()
                    .horizontalScroll(crumbScroll)
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val crumbs = state?.crumbs.orEmpty()
                crumbs.forEachIndexed { i, c ->
                    if (i > 0) {
                        Icon(
                            ChippyIcons.ChevronRight, null, Modifier.size(14.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    TextButton(
                        onClick = { navigated(); store.send { goPath(c.path) } },
                        enabled = i < crumbs.size - 1,
                        contentPadding = PaddingValues(horizontal = 8.dp),
                    ) {
                        Text(c.label, maxLines = 1)
                    }
                }
            }

            // Filter this folder, or search below it.
            OutlinedTextField(
                value = text,
                onValueChange = {
                    text = it
                    store.send { if (searching) setSearch(it) else setFilter(it) }
                },
                placeholder = {
                    Text(stringResource(if (searching) R.string.search_below else R.string.filter_folder))
                },
                leadingIcon = { Icon(if (searching) ChippyIcons.Search else ChippyIcons.Filter, null) },
                trailingIcon = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (text.isNotEmpty()) {
                            IconButton(onClick = {
                                text = ""
                                store.send { if (searching) setSearch("") else setFilter("") }
                            }) { Icon(ChippyIcons.Close, stringResource(R.string.clear)) }
                        }
                        TextButton(onClick = {
                            // Carry the words over to the other mode.
                            val words = text
                            store.send {
                                if (searching) {
                                    setSearch("")
                                    setFilter(words)
                                } else {
                                    setFilter("")
                                    setSearch(words)
                                    runSearch()
                                }
                            }
                            searching = !searching
                        }) {
                            Text(stringResource(if (searching) R.string.this_folder else R.string.below))
                        }
                    }
                },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = if (searching) ImeAction.Search else ImeAction.Done),
                keyboardActions = KeyboardActions(
                    onSearch = { store.send { runSearch() }; focus.clearFocus() },
                    onDone = { focus.clearFocus() },
                ),
                shape = RoundedCornerShape(28.dp),
                colors = OutlinedTextFieldDefaults.colors(
                    unfocusedBorderColor = MaterialTheme.colorScheme.outlineVariant,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
                ),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 4.dp),
            )

            PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = {
                    refreshing = true
                    store.send { refresh() }
                },
                modifier = Modifier.weight(1f),
            ) {
                LaunchedEffect(refreshing) {
                    if (refreshing) {
                        delay(400)
                        refreshing = false
                    }
                }
                val items = state?.items.orEmpty()
                if (items.isEmpty() && state != null) {
                    Box(Modifier.fillMaxSize()) {
                        EmptyMessage(
                            if (state.inResults) ChippyIcons.Search else ChippyIcons.Folder,
                            when {
                                state.inResults && state.searching -> stringResource(R.string.searching)
                                state.inResults -> stringResource(R.string.nothing_found)
                                state.filter.isNotEmpty() -> stringResource(R.string.nothing_matches, state.filter)
                                state.statusError -> state.status
                                else -> stringResource(R.string.nothing_here)
                            },
                            if (state.statusError || state.inResults || state.filter.isNotEmpty()) ""
                            else stringResource(R.string.nothing_here_hint),
                        )
                    }
                } else {
                    LazyColumn(
                        state = listState,
                        contentPadding = PaddingValues(bottom = 24.dp),
                        modifier = Modifier.fillMaxSize(),
                    ) {
                        itemsIndexed(items, key = { i, it -> "${state?.serial}:$i:${it.stem}" }) { i, item ->
                            FileRow(
                                item,
                                selected = i == state?.selected,
                                onOpen = {
                                    if (item.kind.opens) navigated()
                                    store.send { activate(i.toUInt()) }
                                },
                                onAdd = {
                                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                    store.send { addToPlaylist(i.toUInt()) }
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun FileRow(item: FileItem, selected: Boolean, onOpen: () -> Unit, onAdd: () -> Unit) {
    val look = LocalLook.current
    var menu by remember { mutableStateOf(false) }
    val playable = item.kind == FileKind.AUDIO || item.kind == FileKind.TRACK || item.kind == FileKind.MULTI_TRACK
    Box {
        ListItem(
            modifier = Modifier
                .combinedClickable(
                    onClick = onOpen,
                    onLongClick = { if (playable || item.kind.opens) menu = true },
                )
                .then(
                    if (selected) Modifier.background(look.paintB.copy(alpha = 0.10f)) else Modifier
                ),
            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
            leadingContent = {
                Box(
                    Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(MaterialTheme.colorScheme.surfaceContainerHigh),
                    contentAlignment = Alignment.Center,
                ) {
                    if (item.playing) {
                        PaintedIcon(ChippyIcons.Bars, Modifier.size(22.dp), stringResource(R.string.playing))
                    } else {
                        Icon(
                            item.kind.icon(), null, Modifier.size(22.dp),
                            tint = if (item.kind.opens || playable) look.paintA
                            else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            },
            headlineContent = {
                Text(
                    item.stem,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    color = if (item.playing) look.accent else MaterialTheme.colorScheme.onSurface,
                )
            },
            supportingContent = if (item.detail.isNotEmpty()) {
                { Text(item.detail, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            } else null,
            trailingContent = if (item.ext.isNotEmpty()) {
                {
                    Text(
                        item.ext.uppercase(),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            } else null,
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(if (item.kind.opens) R.string.open else R.string.play)) },
                leadingIcon = { Icon(if (item.kind.opens) ChippyIcons.Folder else ChippyIcons.Play, null) },
                onClick = { menu = false; onOpen() },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.add_to_playlist)) },
                leadingIcon = { Icon(ChippyIcons.Queue, null) },
                onClick = { menu = false; onAdd() },
            )
        }
    }
}
