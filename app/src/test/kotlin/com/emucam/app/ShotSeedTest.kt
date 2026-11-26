package com.emucam.app

import com.emucam.model.Recipe
import org.junit.Assert.assertEquals
import org.junit.Test

class ShotSeedTest {
    @Test
    fun shotKeepsTheSeedThePreviewShowedAndLiveRecipeRolls() {
        val (shot, next) = nextShot(Recipe(seed = 77), 0) { 1234L }
        assertEquals(77L, shot.seed)
        assertEquals(1234L, next.seed)
    }

    @Test
    fun unsetSeedIsRolledOnceAndSetSeedKept() {
        assertEquals(9L, Recipe(seed = 0).withSeedIfUnset { 9L }.seed)
        assertEquals(5L, Recipe(seed = 5).withSeedIfUnset { 9L }.seed)
    }
}
