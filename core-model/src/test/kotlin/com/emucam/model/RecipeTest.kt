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

    @Test
    fun focusFollowsDeviceRotation() {
        val p = UserParams(focusCenter = listOf(0.2f, 0.3f), focusAngle = 10f)
        assertEquals(p, p.forCaptureRotation(0))
        // top of the phone pointing left: screen right edge becomes the photo's top
        assertEquals(listOf(0.3f, 0.8f), p.forCaptureRotation(1).focusCenter)
        assertEquals(280f, p.forCaptureRotation(1).focusAngle)
        assertEquals(listOf(0.7f, 0.2f), p.forCaptureRotation(3).focusCenter)
        assertEquals(100f, p.forCaptureRotation(3).focusAngle)
        val q = p.forCaptureRotation(2).focusCenter!!
        assertEquals(0.8f, q[0], 1e-6f); assertEquals(0.7f, q[1], 1e-6f)
    }

    @Test
    fun redevelopKeepsTheShotsFocusButTakesTheNewLook() {
        val shot = Recipe(film = "tri-400", lens = "helios-58", params = UserParams(focusCenter = listOf(0.7f, 0.2f), focusAngle = 90f, focusSize = 1.5f))
        val look = Recipe(film = "aero-ir", lens = "nocti-50", params = UserParams(focusCenter = listOf(0.5f, 0.5f), grain = 0.3f))
        val r = shot.redevelopWith(look)
        assertEquals("aero-ir", r.film)
        assertEquals("nocti-50", r.lens)
        assertEquals(0.3f, r.params.grain)
        assertEquals(listOf(0.7f, 0.2f), r.params.focusCenter)
        assertEquals(90f, r.params.focusAngle)
        assertEquals(1.5f, r.params.focusSize)
    }
}
