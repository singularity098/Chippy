package io.github.singularity098.chippy

import android.content.Context

/**
 * Hands the process's JavaVM and Context to the Rust library, whose sound
 * output (cpal on Oboe) calls into Java with them. Without it the engine
 * finds no sound device and plays silently. Must run before the engine
 * starts.
 */
object NativeContext {
    @Volatile private var done = false

    fun ensure(context: Context) {
        if (done) return
        synchronized(this) {
            if (done) return
            // The same library UniFFI loads through JNA; loading it here
            // as well lets the JVM find init() below.
            System.loadLibrary("chippy_android")
            init(context.applicationContext)
            done = true
        }
    }

    private external fun init(context: Context)
}
