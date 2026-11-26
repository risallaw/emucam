package com.emucam.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.floatOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

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
    /**
     * Device rotation of the shot (0..3 = Surface.ROTATION_*), set by
     * [forCaptureRotation]. Rust uses it to turn effects placed at random in
     * the viewfinder frame (a body's light leak) into the photo's frame.
     */
    @SerialName("capture_rotation") val captureRotation: Int = 0,
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
    /** Use depth (estimated per shot) for defocus when the lens supports it. */
    val depth: Boolean = true,
)

val EmuJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
}

/**
 * Map viewfinder-space focus geometry (portrait screen, y down) into the
 * saved photo's frame when the device was held at [surfaceRotation]
 * (0..3 = Surface.ROTATION_0/90/180/270). Sizes are diagonal-relative, so
 * only the centre and angle change.
 */
fun UserParams.forCaptureRotation(surfaceRotation: Int): UserParams {
    val r = ((surfaceRotation % 4) + 4) % 4
    if (r == 0) return this
    val c = focusCenter?.takeIf { it.size == 2 }?.let { (x, y) ->
        when (r) {
            1 -> listOf(y, 1f - x)
            2 -> listOf(1f - x, 1f - y)
            else -> listOf(1f - y, x)
        }
    } ?: focusCenter
    val a = when (r) {
        1 -> focusAngle - 90f
        2 -> focusAngle + 180f
        else -> focusAngle + 90f
    }
    return copy(focusCenter = c, focusAngle = ((a % 360f) + 360f) % 360f)
}

/** Degrees added to viewfinder-space angles for a shot saved at [surfaceRotation]. */
private fun captureAngleDelta(surfaceRotation: Int): Float = when (((surfaceRotation % 4) + 4) % 4) {
    1 -> -90f
    2 -> 180f
    3 -> 90f
    else -> 0f
}

private fun normDeg(a: Float) = ((a % 360f) + 360f) % 360f

/**
 * This recipe for a shot taken at [surfaceRotation]: focus geometry and
 * every placed effect (OMNI wand, light leak, …) move into the saved
 * photo's frame.
 */
fun Recipe.forCaptureRotation(surfaceRotation: Int): Recipe {
    val r = ((surfaceRotation % 4) + 4) % 4
    val d = captureAngleDelta(r)
    val p = params.forCaptureRotation(r)
    if (d == 0f) return copy(params = p, captureRotation = r)
    return copy(params = p, captureRotation = r, effects = effects.map { e ->
        val pos = e.position() ?: return@map e
        e.withPosition(normDeg(pos + d))
    })
}

/** User effects that can be placed around the frame, with their starting position (deg). */
val PLACEABLE_EFFECTS = mapOf("light_leak" to 0f, "fog" to 90f, "prism" to 0f)

private fun JsonObject.position(): Float? =
    runCatching { this["params"]?.jsonObject }.getOrNull()?.num("position")

private fun JsonObject.withPosition(deg: Float): JsonObject {
    val params = runCatching { this["params"]?.jsonObject }.getOrNull() ?: JsonObject(emptyMap())
    return JsonObject(this + ("params" to JsonObject(params + ("position" to JsonPrimitive(deg)))))
}

/** Position (deg) of the user effect [id], if it is on and placed. */
fun Recipe.effectPosition(id: String): Float? = effects.firstOrNull { it.str("id") == id }?.position()

/** Move the user effect [id] to [deg] (no-op when it is off). */
fun Recipe.withEffectPosition(id: String, deg: Float): Recipe =
    copy(effects = effects.map { if (it.str("id") == id) it.withPosition(normDeg(deg)) else it })

/**
 * OMNI-style creative filter held in front of the lens (Rust effect kind
 * "omni"): [wand] is one of [OMNI_WANDS], [position] degrees around the frame
 * (0 right, 90 bottom, 180 left, 270 top), [reach] how far it intrudes.
 */
data class Omni(
    val wand: String = "rods",
    val position: Float = 0f,
    val reach: Float = 0.45f,
    val strength: Float = 1f,
)

const val OMNI_ID = "user.omni"

/** Wand id → label. */
val OMNI_WANDS = listOf(
    "rods" to "Crystal rods",
    "cylinder" to "Cylinder",
    "facet" to "Facet",
    "swirl" to "Swirl",
    "rainbow" to "Rainbow",
)

private fun JsonObject.str(k: String) = (this[k] as? JsonPrimitive)?.content
private fun JsonObject.num(k: String) = (this[k] as? JsonPrimitive)?.floatOrNull

fun Recipe.omni(): Omni? {
    val e = effects.firstOrNull { it.str("id") == OMNI_ID } ?: return null
    val p = runCatching { e["params"]?.jsonObject }.getOrNull() ?: JsonObject(emptyMap())
    val d = Omni()
    return Omni(
        wand = p.str("wand") ?: d.wand,
        position = p.num("position") ?: d.position,
        reach = p.num("reach") ?: d.reach,
        strength = e.num("strength") ?: d.strength,
    )
}

/**
 * This recipe with its OMNI filter changed by [f] (no-op when it is off).
 * Reads the filter from this recipe, not from a copy a UI callback captured
 * earlier, so changing one setting never reverts the others.
 */
fun Recipe.updateOmni(f: (Omni) -> Omni): Recipe = omni()?.let { withOmni(f(it)) } ?: this

/** This recipe with the OMNI filter set to [o], or removed when null. */
fun Recipe.withOmni(o: Omni?): Recipe {
    val rest = effects.filterNot { it.str("id") == OMNI_ID }
    if (o == null) return copy(effects = rest)
    val e = buildJsonObject {
        put("kind", JsonPrimitive("omni"))
        put("id", JsonPrimitive(OMNI_ID))
        put("strength", JsonPrimitive(o.strength))
        put("params", buildJsonObject {
            put("wand", JsonPrimitive(o.wand))
            put("position", JsonPrimitive(o.position))
            put("reach", JsonPrimitive(o.reach))
        })
    }
    // keep its place in the list so effect order (and seeds) stay stable
    val i = effects.indexOfFirst { it.str("id") == OMNI_ID }
    return copy(effects = if (i < 0) rest + e else effects.toMutableList().also { it[i] = e })
}

/**
 * Re-develop this shot with another [look] (film, lens, body, effects,
 * sliders) while keeping the shot's own focus geometry: the viewfinder's
 * current focus point belongs to a different frame (and orientation). The
 * look's placed effects are turned the way this shot was taken, from the
 * frame they are in: the viewfinder's (rotation 0) or, for a look built from
 * a shot's recipe, that shot's.
 */
fun Recipe.redevelopWith(look: Recipe): Recipe {
    val turned = look.forCaptureRotation(captureRotation - look.captureRotation).copy(captureRotation = captureRotation)
    return turned.copy(
        params = turned.params.copy(
            focusCenter = params.focusCenter,
            focusAngle = params.focusAngle,
            focusSize = params.focusSize,
        ),
    )
}

/**
 * This recipe with other stock: a camera [body] (which brings its own film,
 * lens and effects), or else [film] and [lens] in place of the current ones.
 * [currentLens] is the lens this recipe resolves to now; switching to a
 * different lens resets the lens controls, which belong to the old lens.
 */
fun Recipe.withStock(body: String?, film: String?, lens: String?, currentLens: String?): Recipe =
    if (body != null) {
        copy(body = body, film = null, lens = null, disabledEffects = emptyList())
    } else {
        copy(
            body = null, film = film, lens = lens,
            params = if (lens != currentLens) params.copy(values = emptyMap(), blur = 1f) else params,
        )
    }
