package io.github.singularity098.chippy

import android.app.Application

/**
 * The process: the engine lives here, not in the activity, so the music
 * (and the media notification) carry on while no activity is showing, and
 * an activity that comes back finds everything as it was.
 */
class ChippyApplication : Application() {
    lateinit var store: Store
        private set

    override fun onCreate() {
        super.onCreate()
        NativeContext.ensure(this)
        store = Store(this)
        MediaBridge.init(this, store)
    }
}
