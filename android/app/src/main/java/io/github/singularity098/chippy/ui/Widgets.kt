package io.github.singularity098.chippy.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.singularity098.chippy.Cover
import io.github.singularity098.chippy.R

/** Paints what is drawn (an icon, say) with a brush instead of a colour. */
fun Modifier.paintWith(brush: Brush): Modifier = this
    .graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)
    .drawWithContent {
        drawContent()
        drawRect(brush, blendMode = BlendMode.SrcIn)
    }

/** An icon in the theme's paint. */
@Composable
fun PaintedIcon(icon: ImageVector, modifier: Modifier = Modifier, contentDescription: String? = null) {
    Icon(
        icon,
        contentDescription,
        modifier.paintWith(LocalLook.current.paint()),
        tint = Color.White,
    )
}

/**
 * The spectrum: bars that jump up and settle, with peak caps, painted with
 * the theme (as on the desktop).
 */
@Composable
fun Spectrum(bars: List<Float>, peaks: List<Float>, modifier: Modifier = Modifier) {
    val look = LocalLook.current
    Canvas(modifier.semantics { contentDescription = "Spectrum" }) {
        val n = bars.size
        if (n == 0) return@Canvas
        val gap = (size.width / n * 0.28f).coerceIn(1.5f, 6.dp.toPx())
        val w = (size.width - gap * (n - 1)) / n
        val cap = 2.5.dp.toPx()
        val brush = Brush.verticalGradient(
            0f to look.paintA, 0.5f to look.paintB, 1f to look.paintC,
            startY = 0f, endY = size.height,
        )
        val radius = CornerRadius(w / 3, w / 3)
        for (i in 0 until n) {
            val x = i * (w + gap)
            // A faint floor, so the shape is there while nothing plays.
            drawRoundRect(
                Color.White.copy(alpha = 0.06f), Offset(x, size.height - cap), Size(w, cap), radius,
            )
            val h = (bars[i].coerceIn(0f, 1f) * (size.height - cap * 2))
            if (h > 0.5f) {
                drawRoundRect(brush, Offset(x, size.height - h), Size(w, h), radius)
            }
            val p = peaks.getOrElse(i) { 0f }.coerceIn(0f, 1f) * (size.height - cap * 2)
            if (p > 0.5f) {
                drawRoundRect(
                    look.accent.copy(alpha = 0.9f), Offset(x, size.height - p - cap * 1.8f), Size(w, cap), radius,
                )
            }
        }
    }
}

/** The cover, or a placeholder in the theme's paint when there is none. */
@Composable
fun CoverArt(cover: Cover?, hasTrack: Boolean, modifier: Modifier = Modifier, corner: Dp = 22.dp) {
    val look = LocalLook.current
    BoxWithConstraints(modifier, contentAlignment = Alignment.Center) {
        val aspect = (cover?.aspect ?: 1f).coerceIn(0.6f, 1.6f)
        val w = minOf(maxWidth, maxHeight * aspect)
        val shape = RoundedCornerShape(corner)
        Box(
            Modifier
                .size(w, w / aspect)
                .shadow(if (cover != null) 24.dp else 0.dp, shape, clip = false)
                .clip(shape)
                .background(if (cover != null) Color.Black else MaterialTheme.colorScheme.surfaceContainerLow)
                .then(
                    if (cover == null) Modifier.border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape)
                    else Modifier
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (cover != null) {
                Image(
                    cover.image,
                    contentDescription = stringResource(R.string.album_art),
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Crop,
                )
            } else {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(
                            Brush.radialGradient(
                                listOf(look.paintC.copy(alpha = 0.2f), Color.Transparent),
                            )
                        ),
                )
                PaintedIcon(
                    if (hasTrack) ChippyIcons.Disc else ChippyIcons.ChipIso,
                    Modifier
                        .size(w * 0.38f)
                        .graphicsLayer(alpha = 0.6f),
                )
            }
        }
    }
}

/** A section's title in a list of settings. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text.uppercase(),
        modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 6.dp),
        style = MaterialTheme.typography.labelLarge,
        letterSpacing = 1.6.sp,
        color = LocalLook.current.accent,
    )
}

/** − value + */
@Composable
fun Stepper(value: String, onStep: (Int) -> Unit, label: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        FilledTonalIconButton(onClick = { onStep(-1) }, modifier = Modifier.size(36.dp)) {
            Text("−", style = MaterialTheme.typography.titleLarge)
        }
        Text(
            value,
            Modifier
                .widthIn(min = 48.dp)
                .semantics { contentDescription = "$label: $value" },
            style = MaterialTheme.typography.titleMedium,
            textAlign = TextAlign.Center,
        )
        FilledTonalIconButton(onClick = { onStep(1) }, modifier = Modifier.size(36.dp)) {
            Text("+", style = MaterialTheme.typography.titleLarge)
        }
    }
}

/** The big round play button, in the theme's paint. */
@Composable
fun PaintedCircle(
    size: Dp,
    onClick: () -> Unit,
    onClickLabel: String,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    val look = LocalLook.current
    Box(
        modifier
            .size(size)
            .shadow(10.dp, CircleShape, ambientColor = look.paintB, spotColor = look.paintB)
            .clip(CircleShape)
            .background(look.paint())
            .clickable(onClickLabel = onClickLabel, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { content() }
}

/** Fills the width with a message, for empty lists. */
@Composable
fun EmptyMessage(icon: ImageVector, title: String, detail: String, modifier: Modifier = Modifier, action: (@Composable () -> Unit)? = null) {
    androidx.compose.foundation.layout.Column(
        modifier
            .fillMaxWidth()
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        PaintedIcon(icon, Modifier.size(56.dp).graphicsLayer(alpha = 0.7f))
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        if (detail.isNotEmpty()) {
            Text(
                detail,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
        action?.invoke()
    }
}
