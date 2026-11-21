package io.github.singularity098.chippy

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Environment
import android.util.Log
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import io.github.singularity098.chippy.core.BrowserState
import io.github.singularity098.chippy.core.Engine
import io.github.singularity098.chippy.core.ExportState
import io.github.singularity098.chippy.core.Listener
import io.github.singularity098.chippy.core.NowState
import io.github.singularity098.chippy.core.PlaylistState
import io.github.singularity098.chippy.core.SettingsState
import io.github.singularity098.chippy.core.TabsState
import io.github.singularity098.chippy.core.UiEvent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

/** The cover, decoded: for the screen, and a small copy for the system. */
class Cover(val image: ImageBitmap, val aspect: Float)

/**
 * The app's state, as the Rust engine last told it, for Compose to show;
 * and the one way to the engine. One per process (see [ChippyApplication]).
 *
 * Calls into the engine run in order on a thread of their own ([send]):
 * opening a folder (or an archive) reads it, which must not hold up the UI.
 * The engine's updates arrive on its threads and land in StateFlows.
 */
class Store(private val context: Context) : Listener {
    private val engineThread = Executors.newSingleThreadExecutor { r ->
        Thread(r, "engine-calls")
    }
    private val dispatcher = engineThread.asCoroutineDispatcher()
    val scope = CoroutineScope(SupervisorJob() + dispatcher)

    @Volatile
    var engine: Engine? = null
        private set

    private val _ready = MutableStateFlow(false)
    val ready: StateFlow<Boolean> = _ready
    private val _browser = MutableStateFlow<BrowserState?>(null)
    val browser: StateFlow<BrowserState?> = _browser
    private val _playlist = MutableStateFlow<PlaylistState?>(null)
    val playlist: StateFlow<PlaylistState?> = _playlist
    private val _now = MutableStateFlow<NowState?>(null)
    val now: StateFlow<NowState?> = _now
    private val _settings = MutableStateFlow<SettingsState?>(null)
    val settings: StateFlow<SettingsState?> = _settings
    private val _export = MutableStateFlow<ExportState?>(null)
    val export: StateFlow<ExportState?> = _export
    private val _tabs = MutableStateFlow<TabsState?>(null)
    val tabs: StateFlow<TabsState?> = _tabs
    private val _cover = MutableStateFlow<Cover?>(null)
    val cover: StateFlow<Cover?> = _cover
    val events = MutableSharedFlow<UiEvent>(extraBufferCapacity = 32)

    private val artThread = Executors.newSingleThreadExecutor { r -> Thread(r, "cover") }
    private val artGeneration = AtomicLong()

    // Last, once every field is set: the engine calls back from its start.
    init {
        engineThread.execute {
            try {
                engine = Engine(
                    context.filesDir.absolutePath,
                    Environment.getExternalStorageDirectory().absolutePath,
                    this,
                )
                _ready.value = true
            } catch (e: Throwable) {
                Log.e("chippy", "the engine did not start", e)
            }
        }
    }

    /** Runs [f] on the engine, in order with every other call. */
    fun send(f: Engine.() -> Unit) {
        engineThread.execute {
            val e = engine ?: return@execute
            try {
                e.f()
            } catch (t: Throwable) {
                Log.e("chippy", "engine call failed", t)
            }
        }
    }

    /** Asks the engine something, off the UI thread. */
    suspend fun <T> ask(f: Engine.() -> T): T? = withContext(dispatcher) {
        try {
            engine?.f()
        } catch (t: Throwable) {
            Log.e("chippy", "engine call failed", t)
            null
        }
    }

    /** Saves the settings and the session, and stops for good. */
    fun shutdown() {
        // Straight away, not behind other calls: the process is ending.
        try {
            engine?.shutdown()
        } catch (t: Throwable) {
            Log.e("chippy", "shutdown failed", t)
        }
    }

    // ------------------------------------------------- from the engine

    override fun browser(state: BrowserState) {
        _browser.value = state
    }

    override fun playlist(state: PlaylistState) {
        _playlist.value = state
    }

    override fun now(state: NowState) {
        _now.value = state
        MediaBridge.update(context, state)
    }

    override fun settings(state: SettingsState) {
        _settings.value = state
    }

    override fun export(state: ExportState) {
        _export.value = state
    }

    override fun tabs(state: TabsState) {
        _tabs.value = state
    }

    override fun art(data: ByteArray?) {
        val generation = artGeneration.incrementAndGet()
        if (data == null) {
            _cover.value = null
            MediaBridge.updateArt(null)
            return
        }
        // Decoded on a thread of its own, so the engine's updates go on.
        artThread.execute {
            val big = decode(data, 1400)
            if (artGeneration.get() != generation) return@execute
            _cover.value = big?.let { Cover(it.asImageBitmap(), it.width.toFloat() / it.height.coerceAtLeast(1)) }
            MediaBridge.updateArt(big?.let { scaleDown(it, 512) })
        }
    }

    override fun event(event: UiEvent) {
        events.tryEmit(event)
    }

    private fun decode(data: ByteArray, maxSide: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(data, 0, data.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= maxSide) sample *= 2
        val opts = BitmapFactory.Options().apply { inSampleSize = sample }
        return try {
            BitmapFactory.decodeByteArray(data, 0, data.size, opts)
        } catch (e: OutOfMemoryError) {
            null
        }
    }

    private fun scaleDown(b: Bitmap, side: Int): Bitmap {
        val longest = maxOf(b.width, b.height)
        if (longest <= side) return b
        val f = side.toFloat() / longest
        return Bitmap.createScaledBitmap(b, (b.width * f).toInt().coerceAtLeast(1), (b.height * f).toInt().coerceAtLeast(1), true)
    }
}
