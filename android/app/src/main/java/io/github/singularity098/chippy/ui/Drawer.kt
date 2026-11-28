package io.github.singularity098.chippy.ui

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.sign

/**
 * How far the side panels are pulled out: the file browser from the left
 * (0 to 1) or the settings from the right (0 to -1). Only one is out at a
 * time, so one number says both. It follows the finger while dragged and
 * springs to where it settles.
 */
@Stable
class PanelPull(initial: Float, private val scope: CoroutineScope) {
    var value by mutableFloatStateOf(initial)
        private set
    private var job: Job? = null
    private var lo = -1f
    private var hi = 1f

    /** A drag starts: an open panel can only be pushed back, not through
     * to the other side. */
    fun grab(open: Float) {
        job?.cancel()
        lo = if (open > 0f) 0f else -1f
        hi = if (open < 0f) 0f else 1f
    }

    fun dragBy(dx: Float, width: Float) {
        if (width > 0f) value = (value + dx / width).coerceIn(lo, hi)
    }

    /** Where a drag let go at `velocity` (px/s) ends: 1, 0 or -1. Flung
     * either way it goes that way, else to the nearer end. */
    fun target(velocity: Float): Float {
        val side = if (value > 0f) 1f else if (value < 0f) -1f else sign(velocity)
        val v = velocity * side
        return when {
            v > Fling -> side
            v < -Fling -> 0f
            abs(value) > 0.5f -> side
            else -> 0f
        }
    }

    fun settle(to: Float, velocity: Float = 0f, width: Float = 1f) {
        job?.cancel()
        job = scope.launch {
            animate(
                value, to,
                initialVelocity = if (width > 0f) velocity / width else 0f,
                animationSpec = spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = 500f),
            ) { v, _ -> value = v }
        }
    }

    private companion object {
        /** px/s: a flick, not a slow drag let go. */
        const val Fling = 900f
    }
}

/**
 * Horizontal drags here move the panels with the finger. `open` is the
 * panel out when the drag starts (1 the browser, -1 the settings, 0
 * neither); `width` the panel's in px; `done` gets the velocity let go at.
 * Drags something inside takes for itself (a slider, a scrolling row) are
 * left to it.
 */
fun Modifier.pullPanels(
    pull: PanelPull,
    key: Any?,
    open: () -> Float,
    width: () -> Float,
    done: (Float) -> Unit,
): Modifier = pointerInput(pull, key) {
    val tracker = VelocityTracker()
    var total = 0f
    detectHorizontalDragGestures(
        onDragStart = {
            tracker.resetTracking()
            total = 0f
            pull.grab(open())
        },
        onHorizontalDrag = { change, amount ->
            change.consume()
            total += amount
            // The finger's travel, not its place in this (moving) panel.
            tracker.addPosition(change.uptimeMillis, Offset(total, 0f))
            pull.dragBy(amount, width())
        },
        onDragEnd = { done(tracker.calculateVelocity().x) },
        onDragCancel = { done(0f) },
    )
}
