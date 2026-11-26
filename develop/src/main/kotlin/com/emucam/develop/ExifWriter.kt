package com.emucam.develop

import androidx.exifinterface.media.ExifInterface
import java.io.File

/** Copies camera EXIF from the original to the developed photo and records the look. */
object ExifWriter {
    private val COPY = listOf(
        ExifInterface.TAG_MAKE, ExifInterface.TAG_MODEL,
        ExifInterface.TAG_DATETIME, ExifInterface.TAG_DATETIME_ORIGINAL, ExifInterface.TAG_DATETIME_DIGITIZED,
        ExifInterface.TAG_OFFSET_TIME, ExifInterface.TAG_OFFSET_TIME_ORIGINAL,
        ExifInterface.TAG_SUBSEC_TIME_ORIGINAL,
        ExifInterface.TAG_F_NUMBER, ExifInterface.TAG_EXPOSURE_TIME, ExifInterface.TAG_PHOTOGRAPHIC_SENSITIVITY,
        ExifInterface.TAG_FOCAL_LENGTH, ExifInterface.TAG_FOCAL_LENGTH_IN_35MM_FILM,
        ExifInterface.TAG_EXPOSURE_BIAS_VALUE, ExifInterface.TAG_FLASH, ExifInterface.TAG_WHITE_BALANCE,
        ExifInterface.TAG_GPS_LATITUDE, ExifInterface.TAG_GPS_LATITUDE_REF,
        ExifInterface.TAG_GPS_LONGITUDE, ExifInterface.TAG_GPS_LONGITUDE_REF,
        ExifInterface.TAG_GPS_ALTITUDE, ExifInterface.TAG_GPS_ALTITUDE_REF,
        ExifInterface.TAG_GPS_TIMESTAMP, ExifInterface.TAG_GPS_DATESTAMP,
    )

    fun copy(original: File, developed: File, description: String, recipeJson: String, software: String) {
        val src = ExifInterface(original)
        val dst = ExifInterface(developed)
        for (tag in COPY) src.getAttribute(tag)?.let { dst.setAttribute(tag, it) }
        // Rust already applied the orientation to the pixels.
        dst.setAttribute(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL.toString())
        dst.setAttribute(ExifInterface.TAG_IMAGE_DESCRIPTION, description)
        dst.setAttribute(ExifInterface.TAG_SOFTWARE, software)
        dst.setAttribute(ExifInterface.TAG_USER_COMMENT, "emucam:$recipeJson")
        dst.saveAttributes()
    }
}
