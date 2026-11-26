package com.emucam.develop

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** Publishes a developed photo to Pictures/emucam (no storage permission on API 29+). */
object MediaStoreSaver {
    fun save(context: Context, file: File, takenAt: Long, format: OutputFormat = OutputFormat.DEFAULT): Uri {
        val name = "EMU_" + SimpleDateFormat("yyyyMMdd_HHmmss_SSS", Locale.US).format(Date(takenAt)) + "." + format.extension
        val values = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, name)
            put(MediaStore.Images.Media.MIME_TYPE, format.mimeType)
            put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/emucam")
            put(MediaStore.Images.Media.DATE_TAKEN, takenAt)
            put(MediaStore.Images.Media.IS_PENDING, 1)
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY), values)
            ?: error("MediaStore insert failed")
        try {
            resolver.openOutputStream(uri)!!.use { out -> file.inputStream().use { it.copyTo(out) } }
            resolver.update(uri, ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) }, null, null)
        } catch (t: Throwable) {
            resolver.delete(uri, null, null)
            throw t
        }
        return uri
    }

    /**
     * Remove a photo this app published. Returns false if it could not be
     * deleted (e.g. the app was reinstalled and no longer owns it); the
     * photo then stays in the gallery.
     */
    fun delete(context: Context, uri: Uri): Boolean = try {
        context.contentResolver.delete(uri, null, null) > 0
    } catch (e: SecurityException) {
        false
    }
}
