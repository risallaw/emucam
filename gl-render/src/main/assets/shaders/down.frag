#version 300 es
// 4-tap box downsample (used to build the low-res blur for glow/halation/IR masks).
//#include common
uniform sampler2D uSrc;
uniform vec2 uSrcTexel;
in vec2 vUv;
out vec4 fragColor;
void main() {
    vec2 o = uSrcTexel; // taps at ±1 texel corners → 4x4 box (4x reduction)
    vec3 c = texture(uSrc, vUv + vec2(-o.x, -o.y)).rgb + texture(uSrc, vUv + vec2(o.x, -o.y)).rgb
           + texture(uSrc, vUv + vec2(-o.x, o.y)).rgb + texture(uSrc, vUv + vec2(o.x, o.y)).rgb;
#ifdef LENS_BLUR
    fragColor = vec4(compressC(c * 0.25), 1.0);
#else
    fragColor = vec4(c * 0.25, 1.0);
#endif
}
