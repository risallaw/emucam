package com.emucam.app

import android.app.Application
import com.emucam.develop.DevelopQueue
import com.emucam.develop.ShotStore

/** Manual DI: app-scoped singletons. */
class EmucamApp : Application() {
    lateinit var engine: LookEngine
        private set
    lateinit var store: ShotStore
        private set
    lateinit var queue: DevelopQueue
        private set

    override fun onCreate() {
        super.onCreate()
        engine = LookEngine()
        store = ShotStore(this)
        queue = DevelopQueue(this, store) { engine.describe(it) }
    }
}
