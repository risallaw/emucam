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

    @Test
    fun withStockSwapsFilmKeepsLensControls() {
        val r = Recipe(film = "portrait-400", lens = "helios-58", params = UserParams(blur = 2f, values = mapOf("swirl" to 3f)))
        val n = r.withStock(null, "aero-ir", "helios-58", currentLens = "helios-58")
        assertEquals("aero-ir", n.film)
        assertEquals(r.params, n.params)
    }

    @Test
    fun withStockNewLensResetsControls() {
        val r = Recipe(film = "portrait-400", lens = "helios-58", params = UserParams(blur = 2f, exposure = 1f, values = mapOf("swirl" to 3f)))
        val n = r.withStock(null, "portrait-400", "trioplan-100", currentLens = "helios-58")
        assertEquals("trioplan-100", n.lens)
        assertEquals(emptyMap<String, Float>(), n.params.values)
        assertEquals(1f, n.params.blur)
        assertEquals(1f, n.params.exposure)
    }

    @Test
    fun withStockBodyClearsFilmAndLens() {
        val r = Recipe(film = "portrait-400", lens = "helios-58", disabledEffects = listOf("leak"))
        val n = r.withStock("toy-cam", null, null, currentLens = "helios-58")
        assertEquals("toy-cam", n.body)
        assertEquals(null, n.film)
        assertEquals(null, n.lens)
        assertEquals(emptyList<String>(), n.disabledEffects)
    }

    @Test
    fun omniRoundTripsThroughRecipeJson() {
        val o = Omni(wand = "swirl", position = 200f, reach = 0.6f, strength = 0.8f)
        val r = Recipe(lens = "standard").withOmni(o)
        assertEquals(o, Recipe.fromJson(r.toJson()).omni())
        assertTrue(r.toJson(), r.toJson().contains("\"kind\":\"omni\""))
        assertEquals(null, r.withOmni(null).omni())
        assertEquals(1, r.withOmni(o.copy(wand = "facet")).effects.size)
    }

    @Test
    fun omniPositionFollowsDeviceRotation() {
        val r = Recipe().withOmni(Omni(position = 10f))
        assertEquals(10f, r.forCaptureRotation(0).omni()!!.position)
        assertEquals(280f, r.forCaptureRotation(1).omni()!!.position)
        assertEquals(190f, r.forCaptureRotation(2).omni()!!.position)
        assertEquals(100f, r.forCaptureRotation(3).omni()!!.position)
        assertEquals(null, Recipe().forCaptureRotation(1).omni())
    }

    @Test
    fun placedEffectsFollowDeviceRotationToo() {
        val leak = kotlinx.serialization.json.buildJsonObject {
            put("kind", kotlinx.serialization.json.JsonPrimitive("light_leak"))
            put("id", kotlinx.serialization.json.JsonPrimitive("user.light_leak"))
        }
        val r = Recipe(effects = listOf(leak)).withEffectPosition("user.light_leak", 370f)
        assertEquals(10f, r.effectPosition("user.light_leak"))
        assertEquals(280f, r.forCaptureRotation(1).effectPosition("user.light_leak"))
        assertEquals(null, r.withEffectPosition("user.fog", 5f).effectPosition("user.fog"))
        // unplaced effects are left alone
        assertEquals(leak, Recipe(effects = listOf(leak)).forCaptureRotation(1).effects[0])
    }

    /** A wheel callback built when the filter was "rods at 0°" must not revert a later wand / position change. */
    @Test
    fun updateOmniChangesOneSettingOfTheCurrentFilter() {
        val r = Recipe().withOmni(Omni(wand = "swirl", position = 180f, reach = 0.45f, strength = 0.7f))
        val u = r.updateOmni { it.copy(reach = 0.8f) }
        assertEquals(Omni(wand = "swirl", position = 180f, reach = 0.8f, strength = 0.7f), u.omni())
        assertEquals(1, u.effects.size)
        // off stays off
        assertEquals(Recipe(), Recipe().updateOmni { it.copy(reach = 0.8f) })
    }

    @Test
    fun captureRotationIsRecordedAndUsedWhenRedeveloping() {
        val look = Recipe(lens = "standard").withOmni(Omni(position = 0f))
        assertEquals(0, Recipe.fromJson("""{"lens":"standard"}""").captureRotation)
        val shot = look.forCaptureRotation(1)
        assertEquals(1, shot.captureRotation)
        assertTrue(shot.toJson(), shot.toJson().contains("\"capture_rotation\":1"))
        assertEquals(1, look.forCaptureRotation(5).captureRotation)
        // re-developing that landscape shot with the (portrait) viewfinder look
        val again = shot.redevelopWith(look.withOmni(Omni(position = 10f)))
        assertEquals(280f, again.omni()!!.position)
        assertEquals(1, again.captureRotation)
        assertEquals(shot.params.focusCenter, again.params.focusCenter)
        // an upright shot keeps the look as composed
        assertEquals(10f, look.forCaptureRotation(0).redevelopWith(look.withOmni(Omni(position = 10f))).omni()!!.position)
    }

    /** Contact sheet "other film" builds the look from the shot's own (already turned) recipe. */
    @Test
    fun redevelopWithTheShotsOwnLookDoesNotTurnPlacedEffectsTwice() {
        val shot = Recipe(lens = "standard").withOmni(Omni(position = 10f)).forCaptureRotation(1)
        assertEquals(280f, shot.omni()!!.position)
        val again = shot.redevelopWith(shot.withStock(null, "portrait-400", "standard", "standard"))
        assertEquals(280f, again.omni()!!.position)
        assertEquals(1, again.captureRotation)
        // a look from another landscape shot (turned the other way) lands in this shot's frame
        val other = Recipe().withOmni(Omni(position = 10f)).forCaptureRotation(3)
        assertEquals(280f, shot.redevelopWith(other).omni()!!.position)
    }
}
