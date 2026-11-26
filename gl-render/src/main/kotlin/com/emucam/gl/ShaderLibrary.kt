package com.emucam.gl

import android.content.res.AssetManager
import android.opengl.GLES30

/** Loads GLSL from assets and builds `#define` variants on demand (cached per key). */
internal class ShaderLibrary(private val assets: AssetManager) {
    private val sources = HashMap<String, String>()
    private val programs = HashMap<String, Program>()

    private fun src(name: String): String = sources.getOrPut(name) {
        assets.open("shaders/$name").bufferedReader().use { it.readText() }
    }

    /** Insert defines + common.glsl at the `//#include common` marker. */
    private fun assemble(name: String, defines: List<String>): String {
        val header = buildString {
            defines.forEach { append("#define ").append(it).append('\n') }
            append(src("common.glsl"))
        }
        return src(name).replace("//#include common", header)
    }

    fun program(frag: String, defines: List<String>): Program {
        val key = frag + "|" + defines.joinToString("+")
        return programs.getOrPut(key) {
            Program(GlUtil.link(src("quad.vert"), assemble(frag, defines)))
        }
    }

    fun release() {
        programs.values.forEach { GLES30.glDeleteProgram(it.id) }
        programs.clear()
    }
}

internal class Program(val id: Int) {
    private val locations = HashMap<String, Int>()

    fun use() = GLES30.glUseProgram(id)

    fun loc(name: String): Int = locations.getOrPut(name) { GLES30.glGetUniformLocation(id, name) }

    fun set(name: String, v: FloatArray) {
        val l = loc(name)
        if (l < 0) return
        when (v.size) {
            1 -> GLES30.glUniform1f(l, v[0])
            2 -> GLES30.glUniform2f(l, v[0], v[1])
            3 -> GLES30.glUniform3f(l, v[0], v[1], v[2])
            4 -> GLES30.glUniform4f(l, v[0], v[1], v[2], v[3])
            16 -> GLES30.glUniformMatrix4fv(l, 1, false, v, 0)
        }
    }

    fun setf(name: String, vararg v: Float) = set(name, v)

    fun setInt(name: String, v: Int) {
        val l = loc(name)
        if (l >= 0) GLES30.glUniform1i(l, v)
    }
}
