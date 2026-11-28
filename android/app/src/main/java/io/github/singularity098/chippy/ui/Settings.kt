package io.github.singularity098.chippy.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuAnchorType
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.SettingsState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    store: Store,
    state: SettingsState?,
    onClose: () -> Unit,
    onPickFolder: (String) -> Unit,
) {
    BackHandler(onBack = onClose)
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
                    IconButton(onClick = onClose) { Icon(ChippyIcons.Back, stringResource(R.string.close)) }
                },
                title = { Text(stringResource(R.string.settings)) },
            )
        },
    ) { padding ->
        val s = state ?: return@Scaffold
        LazyColumn(
            Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(bottom = 32.dp),
        ) {
            // -------------------------------------------------- music folders
            item { SectionTitle(stringResource(R.string.music_folders)) }
            item { Hint(stringResource(R.string.music_folders_hint)) }
            if (s.musicDirs.isEmpty()) {
                item { Hint(stringResource(R.string.music_folders_none)) }
            }
            s.musicDirs.forEachIndexed { i, d ->
                item(key = "dir:$i:${d.path}") {
                    // The name and its buttons on one line, the path under
                    // them across the whole width.
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clickable(role = Role.RadioButton) { store.send { setHomeDir(i.toUInt()) } }
                            .padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 10.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = d.home, onClick = { store.send { setHomeDir(i.toUInt()) } })
                            Text(
                                d.label,
                                Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyLarge,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (s.musicDirs.size > 1) {
                                IconButton(
                                    onClick = { store.send { moveMusicDir(i.toUInt(), -1) } },
                                    enabled = i > 0,
                                ) { Icon(ChippyIcons.ChevronUp, stringResource(R.string.move_folder_up, d.label)) }
                                IconButton(
                                    onClick = { store.send { moveMusicDir(i.toUInt(), 1) } },
                                    enabled = i < s.musicDirs.size - 1,
                                ) { Icon(ChippyIcons.ChevronDown, stringResource(R.string.move_folder_down, d.label)) }
                            }
                            IconButton(onClick = { store.send { removeMusicDir(i.toUInt()) } }) {
                                Icon(ChippyIcons.Trash, stringResource(R.string.remove_folder, d.label))
                            }
                        }
                        Text(
                            d.path,
                            Modifier.padding(horizontal = 12.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            item {
                Row(
                    Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    FilledTonalButton(onClick = { onPickFolder("music") }) {
                        Text(stringResource(R.string.add_folder))
                    }
                    OutlinedButton(onClick = { store.send { addBrowserDir() } }, enabled = s.browserDir.isNotEmpty()) {
                        Text(stringResource(R.string.browsers_folder))
                    }
                }
            }

            // ------------------------------------------------------ playback
            item { SectionTitle(stringResource(R.string.playback)) }
            item {
                SwitchRow(stringResource(R.string.resume), stringResource(R.string.resume_detail), s.restoreSession) {
                    store.send { setFlag("restore-session", it) }
                }
            }
            item {
                SwitchRow(stringResource(R.string.show_all), stringResource(R.string.show_all_detail), s.showAll) {
                    store.send { setFlag("show-all", it) }
                }
            }
            item {
                SwitchRow(stringResource(R.string.browser_playlist), stringResource(R.string.browser_playlist_detail), s.browserPlaylist) {
                    store.send { setFlag("browser-playlist", it) }
                }
            }

            // ----------------------------------------------------- chiptunes
            item { SectionTitle(stringResource(R.string.chiptunes)) }
            item {
                StepRow(stringResource(R.string.untimed_length), s.defaultLength) { d ->
                    store.send { step("default-length", d) }
                }
            }
            item { StepRow(stringResource(R.string.fade_out), s.fade) { d -> store.send { step("fade", d) } } }
            item { StepRow(stringResource(R.string.loops), s.loops) { d -> store.send { step("loops", d) } } }
            item {
                SwitchRow(stringResource(R.string.gme_accuracy), "", s.gmeAccuracy) {
                    store.send { setFlag("gme-accuracy", it) }
                }
            }
            item {
                SwitchRow(stringResource(R.string.usf_hle), "", s.usfHle) { store.send { setFlag("usf-hle", it) } }
            }
            item { Hint(stringResource(R.string.next_track_note)) }

            // ---------------------------------------------------------- MIDI
            item { SectionTitle(stringResource(R.string.midi)) }
            item { Hint(stringResource(R.string.midi_hint)) }
            item {
                ListItem(
                    colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                    headlineContent = { Text(stringResource(R.string.soundfont_folder)) },
                    supportingContent = {
                        Text(s.soundfontDir.ifEmpty { stringResource(R.string.none_chosen) })
                    },
                )
            }
            item {
                Row(
                    Modifier.padding(horizontal = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    FilledTonalButton(onClick = { onPickFolder("soundfont") }) { Text(stringResource(R.string.choose)) }
                    OutlinedButton(onClick = { store.send { soundfontBrowserDir() } }) {
                        Text(stringResource(R.string.browsers_folder))
                    }
                }
            }
            if (s.soundfontDir.isNotEmpty() && s.soundfonts.isEmpty()) {
                item { Hint(stringResource(R.string.no_soundfonts)) }
            }
            if (s.soundfonts.isNotEmpty()) {
                item { SoundFontPicker(s.soundfonts, s.soundfont) { i -> store.send { pickSoundfont(i.toUInt()) } } }
            }
            if (s.soundfontStatus.isNotEmpty()) {
                item { Hint(s.soundfontStatus) }
            }

            // ------------------------------------------------------- display
            item { SectionTitle(stringResource(R.string.display)) }
            item { ThemePicker(s.theme) { name -> store.send { setTheme(name) } } }
            item {
                SwitchRow(stringResource(R.string.embedded_art), "", s.embeddedArt) {
                    store.send { setFlag("embedded-art", it) }
                }
            }
            item { StepRow(stringResource(R.string.spectrum_bars), s.bars) { d -> store.send { step("bars", d) } } }

            // --------------------------------------------------------- about
            item { SectionTitle(stringResource(R.string.about)) }
            item {
                ListItem(
                    colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                    headlineContent = { Text("Chippy ${s.version}") },
                    supportingContent = { Text(stringResource(R.string.formats)) },
                )
            }
            item { Hint(stringResource(R.string.sound_output, s.device)) }
            item { Hint(stringResource(R.string.settings_file, s.configPath)) }
        }
    }
}

/** The SoundFont in use, as a drop-down of the folder's SF2 files. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SoundFontPicker(fonts: List<String>, picked: Int, onPick: (Int) -> Unit) {
    var open by remember { mutableStateOf(false) }
    ExposedDropdownMenuBox(
        expanded = open,
        onExpandedChange = { open = it },
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        OutlinedTextField(
            value = fonts.getOrNull(picked) ?: stringResource(R.string.no_soundfont),
            onValueChange = {},
            readOnly = true,
            singleLine = true,
            label = { Text(stringResource(R.string.soundfont)) },
            trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded = open) },
            modifier = Modifier
                .fillMaxWidth()
                .menuAnchor(MenuAnchorType.PrimaryNotEditable),
        )
        ExposedDropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            fonts.forEachIndexed { i, name ->
                DropdownMenuItem(
                    text = { Text(name) },
                    trailingIcon = if (i == picked) { { Icon(ChippyIcons.Check, null) } } else null,
                    onClick = {
                        open = false
                        onPick(i)
                    },
                )
            }
        }
    }
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun SwitchRow(title: String, detail: String, on: Boolean, onChange: (Boolean) -> Unit) {
    ListItem(
        modifier = Modifier.clickable(role = Role.Switch) { onChange(!on) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        headlineContent = { Text(title) },
        supportingContent = if (detail.isNotEmpty()) {
            { Text(detail) }
        } else null,
        trailingContent = { Switch(checked = on, onCheckedChange = onChange) },
    )
}

@Composable
private fun StepRow(title: String, value: String, onStep: (Int) -> Unit) {
    ListItem(
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        headlineContent = { Text(title) },
        trailingContent = { Stepper(value, onStep, title) },
    )
}

/** The looks, as swatches of their paint. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ThemePicker(current: String, onPick: (String) -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text(stringResource(R.string.theme), style = MaterialTheme.typography.bodyLarge)
        FlowRow(
            Modifier
                .fillMaxWidth()
                .padding(top = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Looks.forEach { look ->
                val picked = look.name == current
                Column(
                    Modifier
                        .width(64.dp)
                        .clip(MaterialTheme.shapes.small)
                        .clickable(role = Role.RadioButton) { onPick(look.name) }
                        .semantics {
                            selected = picked
                            contentDescription = look.name
                        }
                        .padding(vertical = 4.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Box(
                        Modifier
                            .size(44.dp)
                            .clip(CircleShape)
                            .background(look.paint())
                            .border(
                                if (picked) 3.dp else 1.dp,
                                if (picked) Color.White else Color.White.copy(alpha = 0.15f),
                                CircleShape,
                            ),
                        contentAlignment = Alignment.Center,
                    ) {
                        if (picked) Icon(ChippyIcons.Check, null, Modifier.size(20.dp), tint = Color.Black)
                    }
                    Text(
                        look.name,
                        Modifier.padding(top = 4.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = if (picked) look.accent else MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                        maxLines = 1,
                    )
                }
            }
        }
    }
}
