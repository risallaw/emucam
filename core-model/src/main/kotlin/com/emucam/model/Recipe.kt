package com.emucam.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject

/**
 * Mirror of the Rust `Recipe` (rust/emucore/src/recipe.rs). Rust is the source
 * of truth: unknown fields are ignored on decode and defaults match serde.
 */
@Serializable
data class Recipe(
    val version: Int = 1,
    val body: String? = null,
    val film: String? = null,
    val lens: String? = null,
    val params: UserParams = UserParams(),
    /** User-added effects, free-form (kind + params) so new kinds need no app change. */
    val effects: List<JsonObject> = emptyList(),
    @SerialName("disabled_effects") val disabledEffects: List<String> = emptyList(),
    val seed: Long = 0,
) {
    fun toJson(): String = EmuJson.encodeToString(serializer(), this)

    companion object {
        fun fromJson(s: String): Recipe = EmuJson.decodeFromString(serializer(), s)
    }
}

@Serializable
data class UserParams(
    val exposure: Float = 0f,
    val warmth: Float = 0f,
    val punch: Float = 0f,
    val grain: Float = 1f,
    val halation: Float = 1f,
    val blur: Float = 1f,
    @SerialName("focus_center") val focusCenter: List<Float>? = null,
    @SerialName("focus_size") val focusSize: Float = 1f,
    @SerialName("focus_angle") val focusAngle: Float = 0f,
    @SerialName("aperture_shape") val apertureShape: String? = null,
    val values: Map<String, Float> = emptyMap(),
)

val EmuJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
}
