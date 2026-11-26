package com.emucam.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RecipeTest {
    @Test
    fun roundTrip() {
        val r = Recipe(film = "aero-ir", lens = "petzval-58", params = UserParams(values = mapOf("bokeh_control" to 6f)))
        assertEquals(r, Recipe.fromJson(r.toJson()))
    }

    @Test
    fun usesRustFieldNames() {
        val json = Recipe(params = UserParams(focusCenter = listOf(0.3f, 0.6f))).toJson()
        assertTrue(json, json.contains("\"focus_center\""))
        assertTrue(json, json.contains("\"disabled_effects\""))
    }

    @Test
    fun ignoresUnknownFieldsFromNewerRust() {
        val r = Recipe.fromJson("""{"film":"tri-400","future_field":1,"params":{"grain":0.5,"new":true}}""")
        assertEquals("tri-400", r.film)
        assertEquals(0.5f, r.params.grain)
    }
}
