package com.emucam.app

import com.emucam.gl.PreviewConfig
import com.emucam.model.Catalog
import com.emucam.model.EmuJson
import com.emucam.model.Recipe
import com.emucam.nativebridge.EmuNative
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.float
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Kotlin façade over the Rust engine (catalog, preview configs, naming). */
class LookEngine {
    val catalog: Catalog by lazy { Catalog.fromJson(EmuNative.catalogJson()) }
    val version: String by lazy { EmuNative.version() }

    private var version0 = 0L
    private var lastLutKey: String? = null
    private var lastLuts: Pair<ByteBuffer, ByteBuffer>? = null
    private var lastLutSize = 0

    /**
     * Build the GPU preview config for [recipe]. LUTs are only re-baked when a
     * film-affecting field changed (lens/focus edits reuse them).
     */
    @Synchronized
    fun previewConfig(recipe: Recipe): PreviewConfig {
        val json = recipe.toJson()
        val p = EmuJson.parseToJsonElement(EmuNative.previewParamsJson(json)).jsonObject
        val uniforms = p["uniforms"]!!.jsonObject.mapValues { (_, v) ->
            v.jsonArray.map { it.jsonPrimitive.float }.toFloatArray()
        }
        val defines = p["defines"]!!.jsonArray.map { it.jsonPrimitive.content }
        val size = p["lut_size"]!!.jsonPrimitive.int
        val lutKey = lutKey(recipe)
        val luts = if (lutKey == lastLutKey && lastLutSize == size && lastLuts != null) {
            lastLuts!!
        } else {
            val bytes = EmuNative.lutBytes(size)
            val d = ByteBuffer.allocateDirect(bytes).order(ByteOrder.nativeOrder())
            val pr = ByteBuffer.allocateDirect(bytes).order(ByteOrder.nativeOrder())
            EmuNative.bakeLuts(json, d, pr)
            version0++
            lastLutKey = lutKey
            lastLutSize = size
            (d to pr).also { lastLuts = it }
        }
        return PreviewConfig(
            uniforms = uniforms,
            defines = defines,
            programKey = p["program_key"]!!.jsonPrimitive.content,
            lutSize = size,
            lutDevelop = luts.first,
            lutPrint = luts.second,
            version = version0,
        )
    }

    /** Only fields that change the film LUTs. */
    private fun lutKey(r: Recipe): String {
        val pp = r.params
        return listOf(r.body, r.film, pp.warmth, pp.punch).joinToString("|")
    }

    fun filmId(r: Recipe): String? = r.film ?: catalog.body(r.body)?.film
    fun lensId(r: Recipe): String? = r.lens ?: catalog.body(r.body)?.lens

    fun describe(r: Recipe): String {
        val film = catalog.film(filmId(r))?.name ?: "Digital"
        val lens = catalog.lens(lensId(r))?.name ?: "Standard"
        val body = catalog.body(r.body)?.name
        return listOfNotNull(body, film, lens).joinToString(" · ")
    }

    @Suppress("unused")
    private fun JsonObject.str(k: String) = this[k]?.jsonPrimitive?.content
}
