package com.emucam.develop

import android.content.Context
import com.emucam.model.EmuJson
import com.emucam.model.Recipe
import kotlinx.serialization.Serializable
import java.io.File
import java.util.UUID

@Serializable
enum class ShotState { PENDING, DEVELOPING, DONE, FAILED }

@Serializable
data class Shot(
    val id: String,
    val createdAt: Long,
    val state: ShotState = ShotState.PENDING,
    /** content:// URI of the developed photo in MediaStore. */
    val uri: String? = null,
    val error: String? = null,
)

/**
 * Durable per-shot storage: `files/shots/<id>/{original.jpg, recipe.json,
 * shot.json, developed.jpg}`. Originals + recipes are kept so any shot can be
 * re-developed later with a different film/lens.
 */
class ShotStore(context: Context) {
    private val root = File(context.filesDir, "shots").apply { mkdirs() }

    fun newShot(): Pair<Shot, File> {
        val id = System.currentTimeMillis().toString(36) + "-" + UUID.randomUUID().toString().take(6)
        val dir = File(root, id).apply { mkdirs() }
        val shot = Shot(id = id, createdAt = System.currentTimeMillis())
        write(shot)
        return shot to File(dir, "original.jpg")
    }

    fun dir(id: String) = File(root, id)
    fun original(id: String) = File(dir(id), "original.jpg")
    fun developed(id: String) = File(dir(id), "developed.jpg")
    /** Raw capture-depth network output (little-endian f32), reused by re-develops. */
    fun depth(id: String) = File(dir(id), "depth.f32")
    /** Raw segmentation-network logits (little-endian f32, class-major), reused by re-develops. */
    fun veg(id: String) = File(dir(id), "veg.f32")

    fun saveRecipe(id: String, recipe: Recipe) = atomicWrite(File(dir(id), "recipe.json"), recipe.toJson())
    fun recipe(id: String): Recipe = Recipe.fromJson(File(dir(id), "recipe.json").readText())

    fun write(shot: Shot) = atomicWrite(File(dir(shot.id), "shot.json"), EmuJson.encodeToString(Shot.serializer(), shot))

    fun read(id: String): Shot? = runCatching {
        EmuJson.decodeFromString(Shot.serializer(), File(dir(id), "shot.json").readText())
    }.getOrNull()

    /** Newest first. */
    fun all(): List<Shot> = (root.listFiles() ?: emptyArray())
        .mapNotNull { read(it.name) }
        .sortedByDescending { it.createdAt }

    fun delete(id: String) = dir(id).deleteRecursively()

    private fun atomicWrite(f: File, text: String) {
        val tmp = File(f.parentFile, f.name + ".tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(f)) {
            f.writeText(text); tmp.delete()
        }
    }
}
