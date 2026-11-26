package com.emucam.app.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.emucam.app.ViewfinderViewModel
import com.emucam.ds.Chip
import com.emucam.ds.Emu

@Composable
fun EmucamRoot(vm: ViewfinderViewModel) {
    val ctx = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) launcher.launch(Manifest.permission.CAMERA) }

    val state by vm.state.collectAsStateWithLifecycle()
    Box(Modifier.fillMaxSize().background(Emu.Ink)) {
        if (!granted) {
            Column(
                Modifier.align(Alignment.Center).padding(32.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text("EMUCAM", style = Emu.Title)
                Spacer(Modifier.height(8.dp))
                Text("Film & lens emulation needs the camera.", style = Emu.Body, textAlign = TextAlign.Center)
                Spacer(Modifier.height(16.dp))
                Chip("Allow camera", true, { launcher.launch(Manifest.permission.CAMERA) }, accent = Emu.Signal)
            }
        } else if (state.showGallery) {
            BackHandler { vm.showGallery(false) }
            ContactSheet(state, vm)
        } else {
            ViewfinderScreen(state, vm)
        }
    }
}
