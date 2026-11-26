package com.emucam.app.ui

import android.content.Intent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import com.emucam.app.EmucamApp
import com.emucam.app.UiState
import com.emucam.app.ViewfinderViewModel
import com.emucam.develop.Shot
import com.emucam.develop.ShotState
import com.emucam.ds.Chip
import com.emucam.ds.Emu

/** Contact sheet of developed frames; tap a frame for actions. */
@Composable
fun ContactSheet(s: UiState, vm: ViewfinderViewModel) {
    var selected by remember { mutableStateOf<Shot?>(null) }
    Column(Modifier.fillMaxSize().background(Emu.Ink).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Chip("‹ Camera", false, { vm.showGallery(false) })
            Spacer(Modifier.weight(1f))
            Text("CONTACT SHEET  ·  ${s.shots.size} FRAMES", style = Emu.Label)
        }
        selected?.let { shot -> ShotDetail(shot, vm) { selected = null } }
        LazyVerticalGrid(
            columns = GridCells.Fixed(3),
            contentPadding = PaddingValues(8.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            items(s.shots, key = { it.id }) { shot ->
                Frame(shot, s.developing?.takeIf { it.shotId == shot.id }?.progress) { selected = shot }
            }
        }
    }
}

@Composable
private fun Frame(shot: Shot, progress: Float?, onClick: () -> Unit) {
    val app = LocalContext.current.applicationContext as EmucamApp
    val bmp = produceState<ImageBitmap?>(null, shot.id, shot.state) {
        value = if (shot.state == ShotState.DONE) loadThumb(app.store.developed(shot.id), 360) else loadThumb(app.store.original(shot.id), 360)
    }.value
    Box(Modifier.aspectRatio(3f / 4f).background(Emu.Graphite).clickable(onClick = onClick), contentAlignment = Alignment.Center) {
        if (bmp != null) Image(bmp, null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        when (shot.state) {
            ShotState.DONE -> {}
            ShotState.FAILED -> Text("FAILED", style = Emu.Label.copy(color = Emu.Signal))
            else -> Box(Modifier.fillMaxSize().background(Emu.Ink.copy(alpha = 0.55f)), contentAlignment = Alignment.Center) {
                Text(progress?.let { "DEVELOPING ${(it * 100).toInt()}%" } ?: "IN QUEUE", style = Emu.Label.copy(color = Emu.Amber))
            }
        }
    }
}

@Composable
private fun ShotDetail(shot: Shot, vm: ViewfinderViewModel, onClose: () -> Unit) {
    val ctx = LocalContext.current
    val app = ctx.applicationContext as EmucamApp
    val recipe = remember(shot.id) { runCatching { app.store.recipe(shot.id) }.getOrNull() }
    Column(Modifier.fillMaxWidth().background(Emu.Graphite).padding(14.dp)) {
        Text(recipe?.let { app.engine.describe(it) }?.uppercase() ?: "", style = Emu.Mono)
        shot.error?.let { Text(it, style = Emu.Body.copy(color = Emu.Signal)) }
        Spacer(Modifier.padding(4.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            shot.uri?.let { uri ->
                Chip("Open", false, {
                    ctx.startActivity(Intent(Intent.ACTION_VIEW, uri.toUri()).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
                })
                Chip("Share", false, {
                    val send = Intent(Intent.ACTION_SEND).setType("image/jpeg").putExtra(Intent.EXTRA_STREAM, uri.toUri())
                    ctx.startActivity(Intent.createChooser(send, "Share").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
                })
            }
            Chip("Re-develop", false, { vm.redevelop(shot.id); onClose() }, accent = Emu.Amber)
            Chip("Delete", false, { vm.deleteShot(shot.id); onClose() }, accent = Emu.Signal)
            Chip("Close", false, onClose)
        }
    }
}
