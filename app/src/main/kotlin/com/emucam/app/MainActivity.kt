package com.emucam.app

import android.os.Bundle
import android.view.KeyEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import com.emucam.app.ui.EmucamRoot
import com.emucam.ds.EmuTheme

class MainActivity : ComponentActivity() {
    private val vm: ViewfinderViewModel by viewModels { ViewfinderViewModel.factory(application as EmucamApp) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { EmuTheme { EmucamRoot(vm) } }
    }

    /** Volume keys act as a shutter release. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean {
        if (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN || keyCode == KeyEvent.KEYCODE_VOLUME_UP || keyCode == KeyEvent.KEYCODE_CAMERA) {
            if (event?.repeatCount == 0) vm.shutter()
            return true
        }
        return super.onKeyDown(keyCode, event)
    }
}
