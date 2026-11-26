package com.emucam.develop

import com.emucam.model.EmuJson
import org.junit.Assert.assertEquals
import org.junit.Test

class OutputFormatTest {
    @Test fun defaultIsJpegMax() {
        assertEquals(OutputFormat.JPEG_MAX, OutputFormat.DEFAULT)
        assertEquals(100, OutputFormat.JPEG_MAX.quality)
        assertEquals("image/jpeg", OutputFormat.JPEG_MAX.mimeType)
    }

    @Test fun pngIsLossless() {
        assertEquals("png", OutputFormat.PNG.extension)
        assertEquals("image/png", OutputFormat.PNG.mimeType)
        assertEquals(1, OutputFormat.PNG.nativeCode)
    }

    @Test fun unknownPersistedNameFallsBackToDefault() {
        assertEquals(OutputFormat.DEFAULT, OutputFormat.fromName(null))
        assertEquals(OutputFormat.DEFAULT, OutputFormat.fromName("WEBP"))
        assertEquals(OutputFormat.PNG, OutputFormat.fromName("PNG"))
    }

    @Test fun oldShotJsonWithoutFormatIsJpeg() {
        val shot = EmuJson.decodeFromString(Shot.serializer(), """{"id":"a","createdAt":1,"state":"DONE"}""")
        assertEquals("jpg", shot.format.extension)
    }

    @Test fun shotFormatRoundTrips() {
        val shot = Shot("a", 1, format = OutputFormat.PNG)
        assertEquals(shot, EmuJson.decodeFromString(Shot.serializer(), EmuJson.encodeToString(Shot.serializer(), shot)))
    }
}
