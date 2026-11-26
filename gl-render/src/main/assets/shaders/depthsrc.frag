#version 300 es
// Small sRGB copy of the scene for the live depth network (read back on the CPU).
//#include common
uniform sampler2D uSrc;   // linearize output (linear, mipmapped)
uniform float uLod;
in vec2 vUv;
out vec4 fragColor;

void main() {
    vec3 c = textureLod(uSrc, vUv, uLod).rgb;
    fragColor = vec4(linearToSrgb(clamp(c, 0.0, 1.0)), 1.0);
}
