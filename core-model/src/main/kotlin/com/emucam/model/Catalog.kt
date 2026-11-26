package com.emucam.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** UI catalog produced by Rust `Catalog::to_ui_json()`. */
@Serializable
data class Catalog(
    val films: List<CatalogEntry> = emptyList(),
    val lenses: List<CatalogEntry> = emptyList(),
    val bodies: List<CatalogEntry> = emptyList(),
    val shapes: List<String> = emptyList(),
    @SerialName("effect_kinds") val effectKinds: List<String> = emptyList(),
    /** OMNI wands the engine knows (id + label), in display order. */
    val wands: List<WandEntry> = emptyList(),
) {
    fun film(id: String?) = films.firstOrNull { it.id == id }
    fun lens(id: String?) = lenses.firstOrNull { it.id == id }
    fun body(id: String?) = bodies.firstOrNull { it.id == id }

    companion object {
        fun fromJson(s: String): Catalog = EmuJson.decodeFromString(serializer(), s)
    }
}

@Serializable
data class WandEntry(
    val id: String,
    val label: String,
    /** "crystal" | "mirror" | "film" (UI grouping). */
    val family: String = "crystal",
)

@Serializable
data class CatalogEntry(
    val id: String,
    val name: String,
    val note: String = "",
    val category: String = "",
    val color: String = "",
    val controls: List<Control> = emptyList(),
    val iso: Int? = null,
    val focal: Int? = null,
    val fstop: Float? = null,
    /** Ids of effects bundled by this preset (user can toggle them). */
    val effects: List<String> = emptyList(),
    /** Lenses only: geometry for the viewfinder reticle. */
    val optics: LensOptics? = null,
    /** Bodies only: default film / lens. */
    val film: String? = null,
    val lens: String? = null,
)

@Serializable
data class LensOptics(
    /** "none" | "uniform" | "sweet_spot" | "slice" */
    val field: String = "none",
    val s0: Float = 0f,
    val falloff: Float = 0.5f,
    /** Lens produces positional blur (bokeh shapes apply). */
    val blur: Boolean = false,
)

@Serializable
data class Control(
    val key: String,
    val label: String,
    val min: Float,
    val max: Float,
    val default: Float,
    val step: Float = 0f,
)
