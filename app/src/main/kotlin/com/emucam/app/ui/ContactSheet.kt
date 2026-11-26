package com.emucam.app.ui

import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.net.toUri
import com.emucam.app.EmucamApp
import com.emucam.app.UiState
import com.emucam.app.ViewfinderViewModel
import com.emucam.develop.OutputFormat
import com.emucam.develop.Shot
import com.emucam.develop.ShotState
import com.emucam.ds.ActionButton
import com.emucam.ds.Chip
import com.emucam.ds.DevelopBar
import com.emucam.ds.Emu
import com.emucam.ds.FilmCard
import com.emucam.ds.GlyphButton
import com.emucam.ds.PrimaryButton
import com.emucam.ds.parseHex
import com.emucam.model.Recipe
import com.emucam.model.withStock
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The darkroom: a contact sheet of every frame (newest first) with live
 * develop status, and a full-screen viewer that swipes between frames and
 * holds the actions (share, open, develop again, delete).
 *
 * Back closes the topmost layer: sheet → viewer → contact sheet → camera.
 */
@Composable
fun ContactSheet(s: UiState, vm: ViewfinderViewModel) {
    var openId by rememberSaveable { mutableStateOf<String?>(null) }
    var formatPicker by rememberSaveable { mutableStateOf(false) }

    Box(Modifier.fillMaxSize().background(Emu.Ink)) {
        SheetGrid(
            s,
            onOpen = { openId = it },
            onBack = { vm.showGallery(false) },
            onFormat = { formatPicker = true },
        )

        AnimatedVisibility(openId != null && s.shots.isNotEmpty(), enter = fadeIn(), exit = fadeOut()) {
            ShotViewer(
                s, vm,
                openId = openId,
                onPage = { openId = it },
                onClose = { openId = null },
            )
        }

        BottomPanel(formatPicker, onDismiss = { formatPicker = false }) {
            PanelHeader("Save developed photos as") { formatPicker = false }
            OutputFormat.entries.forEach { f ->
                FormatRow(f, f == s.outputFormat) { vm.setOutputFormat(f); formatPicker = false }
            }
            Text(
                "Applies to new frames and re-develops. Saved to Pictures/emucam.",
                style = Emu.Body,
                modifier = Modifier.padding(horizontal = 18.dp, vertical = 10.dp),
            )
        }

        MessageToast(s.message, vm, Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 96.dp))
    }
}

// ------------------------------------------------------------------ grid

@Composable
private fun SheetGrid(s: UiState, onOpen: (String) -> Unit, onBack: () -> Unit, onFormat: () -> Unit) {
    val total = s.shots.size
    val inDarkroom = s.shots.count { it.state == ShotState.PENDING || it.state == ShotState.DEVELOPING }
    val failed = s.shots.count { it.state == ShotState.FAILED }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        // ---- header: back · title · output format
        Row(
            Modifier.fillMaxWidth().padding(start = 10.dp, end = 14.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            GlyphButton("‹", onBack)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text("CONTACT SHEET", style = Emu.Title)
                Text(
                    listOfNotNull(
                        "$total ${if (total == 1) "FRAME" else "FRAMES"}",
                        failed.takeIf { it > 0 }?.let { "$it FAILED" },
                    ).joinToString("  ·  "),
                    style = Emu.Label,
                )
            }
            Chip("${s.outputFormat.label}  ▾", false, onFormat)
        }

        // ---- darkroom status
        if (inDarkroom > 0) {
            Column(Modifier.fillMaxWidth().background(Emu.Graphite).padding(horizontal = 14.dp, vertical = 8.dp)) {
                val p = s.developing?.progress
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("DARKROOM", style = Emu.Label.copy(color = Emu.Amber))
                    Spacer(Modifier.width(10.dp))
                    Text(
                        if (p != null) "Developing · ${(p * 100).toInt()}%" else "Preparing…",
                        style = Emu.Mono.copy(color = Emu.Bone),
                    )
                    Spacer(Modifier.weight(1f))
                    if (inDarkroom > 1) Text("${inDarkroom - 1} waiting", style = Emu.Mono.copy(color = Emu.Muted))
                }
                Spacer(Modifier.height(6.dp))
                DevelopBar(p)
            }
        }

        if (total == 0) {
            EmptySheet(onBack)
        } else {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                contentPadding = PaddingValues(start = 10.dp, end = 10.dp, top = 10.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                itemsIndexed(s.shots, key = { _, it -> it.id }) { i, shot ->
                    Frame(shot, total - i, s.developing?.takeIf { it.shotId == shot.id }?.progress) { onOpen(shot.id) }
                }
                item(span = { GridItemSpan(maxLineSpan) }) { Spacer(Modifier.navigationBarsPadding()) }
            }
        }
    }
}

@Composable
private fun EmptySheet(onBack: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Box(Modifier.size(72.dp, 96.dp).border(1.dp, Emu.Line, RoundedCornerShape(4.dp)), contentAlignment = Alignment.Center) {
            Text("0", style = Emu.Mono.copy(color = Emu.Muted, fontSize = 20.sp))
        }
        Spacer(Modifier.height(18.dp))
        Text("NO FRAMES YET", style = Emu.Title)
        Spacer(Modifier.height(6.dp))
        Text(
            "Every photo you take is developed here at full resolution with its film and lens, then saved to your gallery.",
            style = Emu.Body,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(20.dp))
        Chip("‹  Back to camera", true, onBack, accent = Emu.Signal)
    }
}

/** One frame: 3:4 print with status overlay, frame number and film below (like an edge code). */
@Composable
private fun Frame(shot: Shot, number: Int, progress: Float?, onClick: () -> Unit) {
    val app = LocalContext.current.applicationContext as EmucamApp
    val bmp = produceState<ImageBitmap?>(null, shot.id, shot.state) {
        val f = if (shot.state == ShotState.DONE) app.store.developed(shot) else app.store.original(shot.id)
        value = loadThumb(f, 480) ?: if (shot.state == ShotState.DONE) loadThumb(app.store.original(shot.id), 480) else null
    }.value
    val recipe = rememberRecipe(shot.id)
    val failed = shot.state == ShotState.FAILED
    Column(Modifier.clip(RoundedCornerShape(6.dp)).clickable(onClick = onClick)) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(3f / 4f)
                .clip(RoundedCornerShape(6.dp))
                .background(Emu.Graphite)
                .border(1.dp, if (failed) Emu.Signal else Emu.Line, RoundedCornerShape(6.dp)),
            contentAlignment = Alignment.Center,
        ) {
            if (bmp != null) Image(bmp, null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
            when (shot.state) {
                ShotState.DONE -> if (shot.format == OutputFormat.PNG) {
                    Text(
                        "PNG",
                        style = Emu.Label.copy(color = Emu.Bone),
                        modifier = Modifier.align(Alignment.TopEnd).padding(6.dp)
                            .background(Emu.Ink.copy(alpha = 0.7f), RoundedCornerShape(3.dp)).padding(horizontal = 5.dp, vertical = 2.dp),
                    )
                }
                ShotState.FAILED -> Column(
                    Modifier.fillMaxSize().background(Emu.Ink.copy(alpha = 0.6f)),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    Text("FAILED", style = Emu.Title.copy(color = Emu.Signal))
                    Text("Open to retry", style = Emu.Body)
                }
                else -> {
                    Box(Modifier.fillMaxSize().background(Emu.Ink.copy(alpha = 0.55f)))
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(
                            if (progress != null) "${(progress * 100).toInt()}%" else "IN QUEUE",
                            style = if (progress != null) Emu.Title.copy(color = Emu.Amber, fontSize = 22.sp) else Emu.Label.copy(color = Emu.Amber),
                        )
                        if (progress != null) Text("DEVELOPING", style = Emu.Label.copy(color = Emu.Amber))
                    }
                    DevelopBar(progress ?: 0f, Modifier.align(Alignment.BottomCenter))
                }
            }
        }
        Row(Modifier.padding(top = 6.dp, start = 2.dp, end = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("%02d".format(number), style = Emu.Mono.copy(color = Emu.Amber))
            Spacer(Modifier.width(8.dp))
            Text(
                recipe?.let { lookShort(app, it) }?.uppercase() ?: "",
                style = Emu.Label,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

// ---------------------------------------------------------------- viewer

@Composable
private fun ShotViewer(
    s: UiState,
    vm: ViewfinderViewModel,
    openId: String?,
    onPage: (String) -> Unit,
    onClose: () -> Unit,
) {
    val shots = s.shots
    val currentShots by rememberUpdatedState(shots)
    val currentOnPage by rememberUpdatedState(onPage)
    val openIdState = rememberUpdatedState(openId)
    val start = shots.indexOfFirst { it.id == openId }.coerceAtLeast(0)
    val pager = rememberPagerState(initialPage = start) { currentShots.size }
    var zoomed by remember { mutableStateOf(false) }
    var developSheet by rememberSaveable { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf<Shot?>(null) }

    // openId is null while the viewer fades out
    BackHandler(enabled = openId != null && !developSheet) { onClose() }
    LaunchedEffect(pager.currentPage) { zoomed = false }

    // swipes → selection
    LaunchedEffect(pager) {
        snapshotFlow { pager.settledPage }.collect { p ->
            if (openIdState.value != null) currentShots.getOrNull(p)?.let { currentOnPage(it.id) }
        }
    }
    // selection (a new re-develop, a deleted frame) → page
    LaunchedEffect(openId, shots) {
        if (openId == null) return@LaunchedEffect
        if (shots.isEmpty()) {
            onClose(); return@LaunchedEffect
        }
        val idx = shots.indexOfFirst { it.id == openId }
        when {
            idx < 0 -> onPage(shots[pager.currentPage.coerceIn(0, shots.size - 1)].id)
            idx != pager.currentPage -> if (kotlin.math.abs(idx - pager.currentPage) <= 2) pager.animateScrollToPage(idx) else pager.scrollToPage(idx)
        }
    }

    val shot = shots.getOrNull(pager.currentPage) ?: return
    val ctx = LocalContext.current
    val app = ctx.applicationContext as EmucamApp
    val recipe = rememberRecipe(shot.id)

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(
            state = pager,
            key = { shots.getOrNull(it)?.id ?: it },
            userScrollEnabled = !zoomed,
            beyondViewportPageCount = 1,
            modifier = Modifier.fillMaxSize(),
        ) { page ->
            shots.getOrNull(page)?.let { p ->
                ViewerPage(
                    p,
                    progress = s.developing?.takeIf { it.shotId == p.id }?.progress,
                    ahead = shots.count { (it.state == ShotState.PENDING || it.state == ShotState.DEVELOPING) && it.createdAt < p.createdAt },
                    onZoom = { if (page == pager.currentPage) zoomed = it },
                    onRetry = { vm.retry(p.id) },
                )
            }
        }

        // ---- top bar
        Row(
            Modifier
                .fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.75f), Color.Transparent)))
                .statusBarsPadding()
                .padding(start = 10.dp, end = 14.dp, top = 6.dp, bottom = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            GlyphButton("‹", onClose)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text("${shots.size - pager.currentPage} / ${shots.size}", style = Emu.Mono.copy(color = Emu.Bone))
                Text(dateText(shot.createdAt), style = Emu.Label)
            }
            Text(
                when (shot.state) {
                    ShotState.DONE -> shot.format.label.uppercase()
                    ShotState.DEVELOPING -> "DEVELOPING"
                    ShotState.PENDING -> "IN QUEUE"
                    ShotState.FAILED -> "FAILED"
                },
                style = Emu.Label.copy(color = if (shot.state == ShotState.FAILED) Emu.Signal else if (shot.state == ShotState.DONE) Emu.Muted else Emu.Amber),
            )
        }

        // ---- bottom: look + actions
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.85f), Color.Black)))
                .navigationBarsPadding()
                .padding(top = 28.dp, bottom = 10.dp),
        ) {
            recipe?.let { r ->
                val (film, rest) = lookParts(app, r)
                Text(film.uppercase(), style = Emu.Title, modifier = Modifier.padding(horizontal = 18.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (rest.isNotEmpty()) Text(rest.uppercase(), style = Emu.Label, modifier = Modifier.padding(horizontal = 18.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), horizontalArrangement = Arrangement.SpaceEvenly) {
                val uri = shot.uri?.takeIf { shot.state == ShotState.DONE }
                ActionButton("↗", "Share", {
                    uri?.let {
                        val send = Intent(Intent.ACTION_SEND).setType(shot.format.mimeType).putExtra(Intent.EXTRA_STREAM, it.toUri())
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        ctx.startActivity(Intent.createChooser(send, "Share photo"))
                    }
                }, enabled = uri != null)
                ActionButton("⤢", "Gallery", {
                    uri?.let {
                        val view = Intent(Intent.ACTION_VIEW).setDataAndType(it.toUri(), shot.format.mimeType)
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        runCatching { ctx.startActivity(view) }
                    }
                }, enabled = uri != null)
                ActionButton("◐", "Develop again", { developSheet = true }, accent = Emu.Amber)
                ActionButton("✕", "Delete", { confirmDelete = shot }, accent = Emu.Signal)
            }
        }

        // ---- develop-again sheet
        BottomPanel(developSheet, onDismiss = { developSheet = false }) {
            DevelopAgain(s, shot, recipe, vm, onDone = { newId ->
                developSheet = false
                onPage(newId)
            }, onClose = { developSheet = false })
        }
    }

    confirmDelete?.let { victim ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            containerColor = Emu.Graphite,
            title = { Text("DELETE THIS FRAME?", style = Emu.Title) },
            text = {
                Text(
                    if (victim.uri != null) "Removes the original and the developed photo from Pictures/emucam. This can't be undone."
                    else "Removes the original and stops its develop. This can't be undone.",
                    style = Emu.Body.copy(color = Emu.Bone),
                )
            },
            confirmButton = {
                TextButton({ vm.deleteShot(victim.id); confirmDelete = null }) { Text("DELETE", style = Emu.Label.copy(color = Emu.Signal)) }
            },
            dismissButton = {
                TextButton({ confirmDelete = null }) { Text("CANCEL", style = Emu.Label.copy(color = Emu.Bone)) }
            },
        )
    }
}

/** One full-screen frame: fitted image, pinch / double-tap zoom, and develop status. */
@Composable
private fun ViewerPage(shot: Shot, progress: Float?, ahead: Int, onZoom: (Boolean) -> Unit, onRetry: () -> Unit) {
    val app = LocalContext.current.applicationContext as EmucamApp
    val done = shot.state == ShotState.DONE
    val bmp = produceState<ImageBitmap?>(null, shot.id, shot.state) {
        value = (if (done) loadThumb(app.store.developed(shot), 1440) else null) ?: loadThumb(app.store.original(shot.id), 1080)
    }.value
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    val zoomCb by rememberUpdatedState(onZoom)
    fun setScale(v: Float, size: androidx.compose.ui.unit.IntSize) {
        scale = v.coerceIn(1f, 5f)
        val mx = size.width * (scale - 1f) / 2f
        val my = size.height * (scale - 1f) / 2f
        offset = if (scale <= 1f) Offset.Zero else Offset(offset.x.coerceIn(-mx, mx), offset.y.coerceIn(-my, my))
        zoomCb(scale > 1f)
    }

    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        if (bmp != null) {
            Image(
                bmp, null,
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .fillMaxSize()
                    .pointerInput(shot.id) {
                        detectTapGestures(onDoubleTap = { tap ->
                            if (scale > 1f) {
                                setScale(1f, size)
                            } else {
                                // zoom towards the tapped point
                                val c = Offset(size.width / 2f, size.height / 2f)
                                offset = (c - tap) * 1.5f
                                setScale(2.5f, size)
                            }
                        })
                    }
                    .pointerInput(shot.id) {
                        // pinch always; one-finger pan only while zoomed (else the pager swipes)
                        awaitEachGesture {
                            awaitFirstDown(requireUnconsumed = false)
                            do {
                                val ev = awaitPointerEvent()
                                val fingers = ev.changes.count { it.pressed }
                                if (fingers >= 2 || scale > 1f) {
                                    offset += ev.calculatePan()
                                    setScale(scale * ev.calculateZoom(), size)
                                    ev.changes.forEach { if (it.positionChanged()) it.consume() }
                                }
                            } while (ev.changes.any { it.pressed })
                        }
                    }
                    .graphicsLayer {
                        scaleX = scale; scaleY = scale
                        translationX = offset.x; translationY = offset.y
                    },
            )
        }
        when (shot.state) {
            ShotState.DONE -> {}
            ShotState.FAILED -> Column(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.6f)).padding(32.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text("DEVELOP FAILED", style = Emu.Title.copy(color = Emu.Signal))
                Spacer(Modifier.height(6.dp))
                Text(shot.error ?: "Unknown error", style = Emu.Body, textAlign = TextAlign.Center, maxLines = 4, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(16.dp))
                Chip("Try again", true, onRetry, accent = Emu.Amber)
            }
            else -> Column(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.5f)).padding(48.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                if (progress != null) {
                    Text("${(progress * 100).toInt()}%", style = Emu.Title.copy(color = Emu.Amber, fontSize = 34.sp))
                    Text("DEVELOPING", style = Emu.Label.copy(color = Emu.Amber))
                } else {
                    Text("IN QUEUE", style = Emu.Title.copy(color = Emu.Amber))
                    Text(
                        if (ahead > 0) "$ahead ${if (ahead == 1) "frame" else "frames"} ahead" else "Starting…",
                        style = Emu.Body,
                    )
                }
                Spacer(Modifier.height(14.dp))
                DevelopBar(progress ?: 0f, Modifier.width(180.dp))
                Spacer(Modifier.height(10.dp))
                Text("Showing the original until it's developed.", style = Emu.Body)
            }
        }
    }
}

// ---------------------------------------------------------- develop again

@Composable
private fun DevelopAgain(
    s: UiState,
    shot: Shot,
    shotRecipe: Recipe?,
    vm: ViewfinderViewModel,
    onDone: (String) -> Unit,
    onClose: () -> Unit,
) {
    val app = LocalContext.current.applicationContext as EmucamApp
    val engine = app.engine
    val shotLens = shotRecipe?.let { engine.lensId(it) }
    var body by remember(shot.id, shotRecipe) { mutableStateOf(shotRecipe?.body) }
    var film by remember(shot.id, shotRecipe) { mutableStateOf(shotRecipe?.let { engine.filmId(it) }) }
    var lens by remember(shot.id, shotRecipe) { mutableStateOf(shotLens) }
    var cameras by remember(shot.id) { mutableStateOf(shotRecipe?.body != null) }
    var fromViewfinder by remember(shot.id) { mutableStateOf(false) }

    PanelHeader("Develop again") { onClose() }
    Text(
        "Makes a new frame from the same original. This frame is kept.",
        style = Emu.Body,
        modifier = Modifier.padding(horizontal = 18.dp),
    )
    Spacer(Modifier.height(10.dp))
    Row(Modifier.padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip("Film & lens", !cameras && !fromViewfinder, { cameras = false; fromViewfinder = false })
        Chip("Cameras", cameras && !fromViewfinder, { cameras = true; fromViewfinder = false })
        Chip("Viewfinder look", fromViewfinder, { fromViewfinder = true }, accent = Emu.Amber)
    }
    Spacer(Modifier.height(10.dp))
    if (fromViewfinder) {
        Text(
            engine.describe(s.recipe).uppercase(),
            style = Emu.Mono,
            modifier = Modifier.padding(horizontal = 18.dp, vertical = 8.dp),
        )
        Text(
            "Uses the camera's current film, lens, sliders and effects; keeps this frame's focus point.",
            style = Emu.Body,
            modifier = Modifier.padding(horizontal = 18.dp),
        )
    } else if (cameras) {
        StockRow(s.catalog.bodies.map { StockItem(it.id, it.name, null, it.note, it.color) }, body) {
            body = it
        }
    } else {
        val films = remember(s.catalog) { s.catalog.films.sortedWith(compareBy({ categoryOrder(it.category) }, { it.name })) }
        Text("FILM", style = Emu.Label, modifier = Modifier.padding(horizontal = 18.dp))
        Spacer(Modifier.height(6.dp))
        StockRow(films.map { StockItem(it.id, it.name, it.iso, it.note, it.color) }, film.takeIf { body == null }) {
            film = it; body = null
        }
        Spacer(Modifier.height(10.dp))
        Text("LENS", style = Emu.Label, modifier = Modifier.padding(horizontal = 18.dp))
        Spacer(Modifier.height(6.dp))
        val lenses = remember(s.catalog) { s.catalog.lenses.sortedBy { it.category + it.name } }
        StockRow(
            lenses.map { StockItem(it.id, it.name, null, "${it.focal ?: ""}mm ƒ/${it.fstop ?: ""} · ${it.category}\n${it.note}", it.color) },
            lens.takeIf { body == null },
        ) {
            lens = it; body = null
        }
    }
    Spacer(Modifier.height(12.dp))
    Row(Modifier.padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("SAVE AS", style = Emu.Label)
        Spacer(Modifier.width(4.dp))
        OutputFormat.entries.forEach { f -> Chip(f.label, f == s.outputFormat, { vm.setOutputFormat(f) }) }
    }
    Spacer(Modifier.height(14.dp))
    PrimaryButton(
        "Develop new frame",
        onClick = {
            val look = when {
                fromViewfinder -> null
                shotRecipe == null -> null
                cameras && body != null -> shotRecipe.withStock(body, null, null, shotLens)
                else -> shotRecipe.withStock(null, film, lens, shotLens)
            }
            vm.redevelop(shot.id, look, onDone)
        },
        enabled = fromViewfinder || shotRecipe != null,
        modifier = Modifier.padding(horizontal = 18.dp),
    )
}

private data class StockItem(val id: String, val name: String, val iso: Int?, val note: String, val color: String)

@Composable
private fun StockRow(items: List<StockItem>, selected: String?, onSelect: (String) -> Unit) {
    val listState = androidx.compose.foundation.lazy.rememberLazyListState(
        initialFirstVisibleItemIndex = items.indexOfFirst { it.id == selected }.coerceAtLeast(0),
    )
    LazyRow(state = listState, contentPadding = PaddingValues(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        items(items, key = { it.id }) { f ->
            FilmCard(f.name, f.iso, f.note, parseHex(f.color), f.id == selected, { onSelect(f.id) })
        }
    }
}

// ---------------------------------------------------------------- pieces

/** Slide-up glass panel over the screen; tapping the dimmed area or Back dismisses it. */
@Composable
private fun BottomPanel(visible: Boolean, onDismiss: () -> Unit, content: @Composable () -> Unit) {
    BackHandler(enabled = visible) { onDismiss() }
    Box(Modifier.fillMaxSize()) {
        AnimatedVisibility(visible, enter = fadeIn(), exit = fadeOut()) {
            Box(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.5f))
                    .clickable(remember { MutableInteractionSource() }, indication = null, onClick = onDismiss),
            )
        }
        AnimatedVisibility(
            visible,
            modifier = Modifier.align(Alignment.BottomCenter),
            enter = slideInVertically { it } + fadeIn(),
            exit = slideOutVertically { it } + fadeOut(),
        ) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp))
                    .background(Emu.Ink.copy(alpha = 0.96f))
                    .border(1.dp, Emu.Line, RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp))
                    // swallow taps so they don't reach the scrim
                    .clickable(remember { MutableInteractionSource() }, indication = null) {}
                    .navigationBarsPadding()
                    .padding(top = 8.dp, bottom = 14.dp),
            ) {
                Box(Modifier.align(Alignment.CenterHorizontally).size(36.dp, 4.dp).clip(RoundedCornerShape(2.dp)).background(Emu.Line))
                content()
            }
        }
    }
}

@Composable
private fun PanelHeader(title: String, onClose: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(start = 18.dp, end = 10.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title.uppercase(), style = Emu.Title, modifier = Modifier.weight(1f))
        GlyphButton("✕", onClose)
    }
}

@Composable
private fun FormatRow(f: OutputFormat, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(20.dp).clip(RoundedCornerShape(50)).border(1.5.dp, if (selected) Emu.Amber else Emu.Line, RoundedCornerShape(50)),
            contentAlignment = Alignment.Center,
        ) {
            if (selected) Box(Modifier.size(10.dp).clip(RoundedCornerShape(50)).background(Emu.Amber))
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(
                f.label.uppercase() + if (f == OutputFormat.DEFAULT) "  ·  DEFAULT" else "",
                style = Emu.Title.copy(color = if (selected) Emu.Bone else Emu.Muted),
            )
            Text(f.detail, style = Emu.Body)
        }
    }
}

@Composable
private fun MessageToast(message: String?, vm: ViewfinderViewModel, modifier: Modifier) {
    message ?: return
    LaunchedEffect(message) { delay(2500); vm.dismissMessage() }
    Text(
        message,
        style = Emu.Body.copy(color = Emu.Bone),
        modifier = modifier
            .padding(horizontal = 24.dp)
            .background(Emu.GraphiteHi, RoundedCornerShape(8.dp))
            .border(1.dp, Emu.Line, RoundedCornerShape(8.dp))
            .padding(horizontal = 14.dp, vertical = 10.dp),
    )
}

@Composable
private fun rememberRecipe(id: String): Recipe? {
    val app = LocalContext.current.applicationContext as EmucamApp
    return produceState<Recipe?>(null, id) {
        value = withContext(Dispatchers.IO) { runCatching { app.store.recipe(id) }.getOrNull() }
    }.value
}

/** (film or body, "lens · body") for a frame's caption. */
private fun lookParts(app: EmucamApp, r: Recipe): Pair<String, String> {
    val e = app.engine
    val film = e.catalog.film(e.filmId(r))?.name ?: "Digital"
    val lens = e.catalog.lens(e.lensId(r))?.name ?: "Standard"
    val body = e.catalog.body(r.body)?.name
    return film to listOfNotNull(lens, body).joinToString("  ·  ")
}

private fun lookShort(app: EmucamApp, r: Recipe): String {
    val (film, rest) = lookParts(app, r)
    return listOf(film, rest.substringBefore("  ·  ")).joinToString(" · ")
}

private fun dateText(t: Long): String = SimpleDateFormat("d MMM yyyy · HH:mm", Locale.getDefault()).format(Date(t)).uppercase()
