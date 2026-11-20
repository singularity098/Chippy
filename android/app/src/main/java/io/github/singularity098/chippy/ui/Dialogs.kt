package io.github.singularity098.chippy.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import io.github.singularity098.chippy.R
import io.github.singularity098.chippy.Store
import io.github.singularity098.chippy.core.ExportFormat
import io.github.singularity098.chippy.core.ExportState
import io.github.singularity098.chippy.core.TrackInfo

/** Exporting playlist tracks to WAV, FLAC, MP3 or Opus files. */
@Composable
fun ExportDialog(store: Store, state: ExportState?, onPickFolder: () -> Unit, onClose: () -> Unit) {
    val s = state ?: return
    AlertDialog(
        // Exporting goes on with the dialog closed; it says when done.
        onDismissRequest = onClose,
        icon = { PaintedIcon(ChippyIcons.Export) },
        title = { Text(stringResource(R.string.export_title, s.what)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                val formats = listOf(
                    ExportFormat.WAV to "WAV", ExportFormat.FLAC to "FLAC",
                    ExportFormat.MP3 to "MP3", ExportFormat.OPUS to "Opus",
                )
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    formats.forEachIndexed { i, (f, label) ->
                        SegmentedButton(
                            selected = s.format == f,
                            onClick = { store.send { setExportFormat(f) } },
                            shape = SegmentedButtonDefaults.itemShape(i, formats.size),
                            enabled = !s.running,
                            icon = {},
                        ) { Text(label) }
                    }
                }
                Text(s.formatDetail, style = MaterialTheme.typography.bodyMedium)
                if (s.format == ExportFormat.MP3 || s.format == ExportFormat.OPUS) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.bitrate), Modifier.weight(1f))
                        Stepper("${s.bitrate} kbps", { d -> store.send { stepExportBitrate(d) } }, stringResource(R.string.bitrate))
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.export_folder))
                        Text(
                            s.folder.ifEmpty { stringResource(R.string.none_chosen) },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Spacer(Modifier.width(8.dp))
                    OutlinedButton(onClick = onPickFolder, enabled = !s.running) {
                        Text(stringResource(R.string.choose))
                    }
                }
                if (s.running || s.status.isNotEmpty()) {
                    LinearProgressIndicator(
                        progress = { s.progress },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Text(s.status, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            if (s.running) {
                TextButton(onClick = { store.send { cancelExport() } }) { Text(stringResource(R.string.cancel)) }
            } else {
                TextButton(onClick = { store.send { startExport() } }, enabled = s.folder.isNotEmpty()) {
                    Text(stringResource(R.string.export))
                }
            }
        },
        dismissButton = {
            TextButton(onClick = onClose) {
                Text(stringResource(if (s.running) R.string.hide else R.string.close))
            }
        },
    )
}

/** A track's properties, from its tags and its file. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InfoSheet(info: TrackInfo, onClose: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Row(
            Modifier.padding(horizontal = 24.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(info.title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
            TextButton(onClick = {
                clipboard.setText(AnnotatedString(info.fields.joinToString("\n") { "${it.label}: ${it.value}" }))
            }) { Text(stringResource(R.string.copy_all)) }
        }
        SelectionContainer {
            LazyColumn(
                Modifier
                    .fillMaxWidth()
                    .navigationBarsPadding(),
            ) {
                items(info.fields) { f ->
                    Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp)) {
                        Text(
                            f.label,
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text(f.value, style = MaterialTheme.typography.bodyLarge)
                    }
                }
            }
        }
    }
}
