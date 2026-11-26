#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Composite: sharp/blur by CoC → glow/veil/vignette → exposure → leak → IR →
// halation → develop LUT → grain → print LUT → frame. Mirrors pipeline.rs.
//#include common
uniform samplerExternalOES uCamera;
uniform sampler2D uLens;      // gathered bokeh (low res), rgb expanded
uniform sampler2D uBlur;      // wide low-res blur of the linear image
uniform highp sampler3D uLutDevelop;
uniform highp sampler3D uLutPrint;
uniform float uLutSize;
in vec2 vUv;
out vec4 fragColor;

const float LOG_MIN = -10.0;
const float LOG_MAX = 6.0;

vec3 cam(vec2 uv) { return srgbToLinear(texture(uCamera, (uTexMatrix * vec4(uv, 0.0, 1.0)).xy).rgb); }
vec3 lut(highp sampler3D t, vec3 c) {
    vec3 x = clamp(c, 0.0, 1.0) * ((uLutSize - 1.0) / uLutSize) + 0.5 / uLutSize;
    return texture(t, x).rgb;
}
vec3 shaper(vec3 lin) { return clamp((log2(max(lin, 1e-8) / 0.18) - LOG_MIN) / (LOG_MAX - LOG_MIN), 0.0, 1.0); }

float vegIndex(vec3 c) {
    float s = c.r + c.g + c.b + 1e-6;
    vec3 k = c / s;
    return smoothstep(uIrVeg.x, uIrVeg.y, 2.0 * k.g - k.r - k.b) * smoothstep(0.01, 0.06, luma(c));
}
float skyIndex(vec3 c, float v) {
    float s = c.r + c.g + c.b + 1e-6;
    vec3 k = c / s;
    return smoothstep(0.04, 0.14, k.b - max(k.r, k.g)) * smoothstep(0.02, 0.1, luma(c)) * smoothstep(0.9, 0.3, v);
}
vec3 hueRotate(vec3 c, float deg) {
    float a = radians(deg);
    float s = sin(a), co = cos(a);
    float k = 0.57735027;
    float oc = 1.0 - co;
    mat3 m = mat3(
        co + k * k * oc, k * k * oc + k * s, k * k * oc - k * s,
        k * k * oc - k * s, co + k * k * oc, k * k * oc + k * s,
        k * k * oc + k * s, k * k * oc - k * s, co + k * k * oc);
    return m * c;
}

void main() {
    vec2 p = imgPx(vUv);
    vec3 c;
    if (abs(uCaLateral) > 1e-5) {
        vec2 ax = axisPx();
        vec2 d = p - ax;
        float rho2 = dot(d, d) / (halfDiag() * halfDiag());
        c = vec3(cam(uvFromImgPx(ax + d * (1.0 + uCaLateral * rho2))).r, cam(vUv).g,
                 cam(uvFromImgPx(ax + d * (1.0 - uCaLateral * rho2))).b);
    } else {
        c = cam(vUv);
    }
#ifdef LENS_BLUR
    float r = coc(p);
    if (r > 0.5) c = mix(c, compressC(texture(uLens, vUv).rgb), smoothstep(0.5, 1.5, r));
#endif
    vec3 wide = texture(uBlur, vUv).rgb;
#ifdef LENS_GLOW
    float g = clamp(uGlow.x, 0.0, 1.0);
    float bloom = smoothstep(0.35, 1.0, luma(wide));
    c = c * (1.0 - 0.55 * g) + wide * 0.55 * g + wide * bloom * g * 0.6;
#endif
    if (uVeil > 0.0) c = c * (1.0 - 0.5 * uVeil) + vec3(luma(wide)) * uVeil * 0.6;
    // vignette (lens + fx)
    vec2 dv = (p - 0.5 * uOutSize) / halfDiag();
    float r2 = dot(dv, dv);
    if (uVignette > 0.0) { float f = 1.0 / (1.0 + 1.6 * uVignette * r2); c *= f * f; }
    if (uFxVignette > 0.0) { float f = 1.0 / (1.0 + 2.5 * uFxVignette * r2 * r2 * 4.0); c *= f; }

    c *= uExposure;
#ifdef FX_LEAK
    {
        vec2 lc = vec2(1.05, 0.35 + 0.3 * fract(uSeed * 0.618));
        vec2 dl = (vUv * vec2(uOutSize.x / uOutSize.y, 1.0) - lc * vec2(uOutSize.x / uOutSize.y, 1.0)) / uLeak.y;
        c += uLeakColor * exp(-dot(dl, dl) * 2.5) * uLeak.x;
    }
#endif
#ifdef FILM
#ifdef IR
    {
        float veg = vegIndex(wide);
        float sky = skyIndex(wide, 1.0 - vUv.y);
        float base = max(0.75 * c.r + 0.25 * c.g + 0.4 * (c.r - c.b), 0.0);
        float ir = mix(base, max(c.g * uIr.y, base), veg) * (1.0 - uIr.z * sky);
        int v = int(uIr.x + 0.5);
        if (v == 0) c = vec3(ir, c.r, c.g);
        else if (v == 1) c = vec3(ir * 1.05, c.r * 0.95, 0.6 * c.g + 0.2 * c.r);
        else if (v == 2) c = vec3(ir);
        else if (v == 3) c = vec3(ir, 0.35 * c.g + 0.55 * ir, 0.9 * c.b + 0.1 * c.r);
        else c = mix(max(hueRotate(c, -12.0), 0.0), max(hueRotate(c, 150.0), 0.0) * 1.5, veg);
    }
#endif
#ifdef HALATION
    {
        float t = clamp(uHalation.y, 0.0, 0.98);
        float e = max(luma(wide * uExposure) - t, 0.0) / (1.0 - t);
        c += uHalation.x * uHalationTint * (e / (1.0 + 0.25 * e)) * 4.0;
    }
#endif
    vec3 y = lut(uLutDevelop, shaper(c));
#ifdef GRAIN
    {
        float sigmaTrue = uGrain.y / 36000.0 * uOutSize.x;
        float s = max(sigmaTrue, 0.5);
        float amp = uGrain.x * 0.24 * clamp(sigmaTrue / 0.6, 0.15, 1.0) * 1.6;
        vec2 gp = p / s;
        float frame = mod(uTime, 64.0);
        vec3 n0 = hash3(gp, frame) - 0.5;
        vec3 n1 = hash3(gp * 0.5 + 17.0, frame + 64.0) - 0.5;
        float mono = (n0.x + n0.y + n0.z) * 0.6 + (n1.x + n1.y + n1.z) * 0.35;
        vec3 n = uMono > 0.5 ? vec3(mono) : mix(vec3(mono), (n0 + n1) * 1.2, uGrain.z);
        y += amp * n * sqrt(clamp(y * (1.0 - y), 0.0, 0.25)) * 2.0;
    }
#endif
    y = lut(uLutPrint, y);
#else
    vec3 y = linearToSrgb(c);
#endif
    if (uFrame.x > 0.0) {
        vec2 uv = vec2(vUv.x, 1.0 - vUv.y);
        float aspect = uOutSize.x / uOutSize.y;
        float du = min(uv.x, 1.0 - uv.x) * aspect;
        float dvv = min(uv.y, 1.0 - uv.y);
        float m = 1.0 - smoothstep(uFrame.x, uFrame.x + 0.002, du) * smoothstep(uFrame.x, uFrame.x + 0.002, dvv);
        vec3 fc = uFrame.y > 1.5 ? vec3(0.85, 0.45, 0.12) : (uFrame.y > 0.5 ? vec3(0.95, 0.94, 0.91) : vec3(0.02));
        y = mix(y, fc, m);
    }
    fragColor = vec4(clamp(y, 0.0, 1.0), 1.0);
}
