// Shared uniforms + functions. Formulas mirror rust/emucore by hand (keep in
// sync: no automated parity test; look for "parity mirror" comments). Uniform values come from
// EmuNative.previewParamsJson — Kotlin uploads them by name.
precision highp float;
precision highp int;

uniform vec2 uOutSize;          // output (full) size in px
uniform mat4 uTexMatrix;        // OES texture transform
uniform float uTime;            // frame counter (grain animation)

// lens
uniform float uField;           // 0 none, 1 uniform, 2 sweet spot, 3 slice
uniform vec2 uFocusCenter;      // normalized, image space (y down)
uniform float uFocusAngle;      // radians
uniform float uS0;
uniform float uFalloff;
uniform float uRmax;            // fraction of width
uniform vec3 uAperture;         // kind, a, b
uniform float uApArea;
uniform float uSwirl;
uniform float uCatsEye;
uniform float uBubble;
uniform float uOnion;
uniform float uCaLateral;
uniform vec2 uGlow;             // strength, radius
uniform float uVignette;
uniform float uVeil;
uniform vec2 uHighlightK;       // k, knee
// depth defocus (mirrors rust lens::Defocus / depth::defocus)
uniform sampler2D uDepth;       // nearness (r), GL uv space; always bound (unit 5)
uniform float uDepthOn;         // 1 when a live depth map is present and wanted
uniform float uFocusNear;       // nearness of the focused surface
uniform vec3 uDepthParams;      // depth_scale, field_optical, wanted
uniform vec2 uDepthDelta;       // sharp delta, full-blur delta
// film
uniform float uExposure;
uniform vec4 uIr;               // variant, k_veg, k_sky, k_skin
uniform vec2 uIrVeg;            // lo, hi
uniform vec3 uHalation;         // strength, threshold, radius
uniform vec3 uHalationTint;
uniform vec3 uGrain;            // amount, size_um, chroma
uniform float uMono;
uniform float uSeed;
uniform vec2 uLeak;             // strength, size
uniform vec3 uLeakColor;
uniform float uFxVignette;
uniform vec2 uFrame;            // thickness, style

const float PI = 3.14159265;

float luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
vec3 srgbToLinear(vec3 v) {
    return mix(v / 12.92, pow((v + 0.055) / 1.055, vec3(2.4)), step(0.04045, v));
}
vec3 linearToSrgb(vec3 v) {
    v = max(v, 0.0);
    return mix(v * 12.92, 1.055 * pow(v, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, v));
}

// image-space pixel position (y down) from GL uv (y up)
vec2 imgPx(vec2 uv) { return vec2(uv.x, 1.0 - uv.y) * uOutSize; }
vec2 uvFromImgPx(vec2 p) { return vec2(p.x / uOutSize.x, 1.0 - p.y / uOutSize.y); }
float halfDiag() { return 0.5 * length(uOutSize); }
vec2 focusPx() { return uFocusCenter * uOutSize; }
vec2 axisPx() { return uField > 1.5 && uField < 2.5 ? focusPx() : 0.5 * uOutSize; }

// position-based blur, 0..1
float fieldCoc(vec2 p) {
    if (uField < 1.5) return 1.0;
    if (uField < 2.5) {
        float d = length(p - focusPx()) / halfDiag();
        return smoothstep(uS0, uS0 + uFalloff, d);
    }
    vec2 n = vec2(-sin(uFocusAngle), cos(uFocusAngle));
    float d = abs(dot(p - focusPx(), n)) / halfDiag();
    return smoothstep(uS0, uS0 + uFalloff, d);
}

float nearAt(vec2 p) { return textureLod(uDepth, uvFromImgPx(p), 0.0).r; }

float depthDefocus(float near) {
    float s = uDepthParams.x;
    if (s <= 0.0) return 0.0;
    float full = max(uDepthDelta.y / max(s, 0.05), uDepthDelta.x * 2.0);
    return smoothstep(uDepthDelta.x, full, abs(near - uFocusNear));
}

float coc(vec2 p) {
    float rmax = uRmax * uOutSize.x;
    if (rmax <= 0.0 || uField < 0.5) return 0.0;
    if (uDepthOn > 0.5) {
        float dz = depthDefocus(nearAt(p));
        if (uDepthParams.y > 0.5) {
            float f = fieldCoc(p);
            return rmax * min(sqrt(f * f + dz * dz), 1.0);
        }
        return rmax * dz;
    }
    return rmax * fieldCoc(p);
}

// ---- highlight expansion (hue preserving, inverse Reinhard above knee)
float expandV(float v) {
    float knee = uHighlightK.y;
    if (v <= knee) return max(v, 0.0);
    float x = min((v - knee) / (1.0 - knee), 0.999);
    return knee + (1.0 - knee) * x / (1.0 - uHighlightK.x * x);
}
float compressV(float v) {
    float knee = uHighlightK.y;
    if (v <= knee) return max(v, 0.0);
    float y = (v - knee) / (1.0 - knee);
    return knee + (1.0 - knee) * y / (1.0 + uHighlightK.x * y);
}
vec3 expandC(vec3 c) { float m = max(c.r, max(c.g, c.b)); return m > uHighlightK.y ? c * (expandV(m) / m) : c; }
vec3 compressC(vec3 c) { float m = max(c.r, max(c.g, c.b)); return m > uHighlightK.y ? c * (compressV(m) / m) : c; }

// ---- aperture shapes (ids: star, heart, sparkle, crescent, flower, diamond, bird)
float sdStar(vec2 p, float r, float n, float m) {
    float an = PI / n;
    float en = PI / m;
    vec2 acs = vec2(cos(an), sin(an));
    vec2 ecs = vec2(cos(en), sin(en));
    float bn = mod(atan(p.y, p.x), 2.0 * an) - an;
    p = length(p) * vec2(cos(bn), abs(sin(bn)));
    p -= r * acs;
    p += ecs * clamp(-dot(p, ecs), 0.0, r * acs.y / ecs.y);
    return length(p) * sign(p.x);
}
float sdHeart(vec2 p) {
    p.x = abs(p.x);
    if (p.y + p.x > 1.0) return sqrt(dot(p - vec2(0.25, 0.75), p - vec2(0.25, 0.75))) - sqrt(2.0) / 4.0;
    vec2 a = p - vec2(0.0, 1.0);
    vec2 b = p - 0.5 * max(p.x + p.y, 0.0);
    return sqrt(min(dot(a, a), dot(b, b))) * sign(p.x - p.y);
}
float shapeSdf(int id, vec2 o) {
    vec2 p = vec2(o.x, -o.y);
    if (id == 0) return sdStar(p, 0.95, 5.0, 2.6);
    if (id == 1) { vec2 q = vec2(p.x * 0.58, p.y * 0.58 + 0.53); return sdHeart(q) / 0.58; }
    if (id == 2) return (sqrt(abs(p.x)) + sqrt(abs(p.y)) - 0.98) * 0.6;
    if (id == 3) return max(length(p) - 0.95, -(length(p - vec2(0.42, 0.18)) - 0.78));
    if (id == 4) return length(p) - (0.72 + 0.26 * cos(5.0 * atan(p.y, p.x)));
    if (id == 5) return (abs(p.x) * 0.95 + abs(p.y) * 0.7) - 0.7;
    if (id == 6) {
        float x = abs(p.x);
        float wing = abs(p.y - (0.35 - 1.2 * (x - 0.45) * (x - 0.45))) - 0.17;
        return max(wing, x - 0.95);
    }
    return length(p) - 1.0;
}
float apWeight(vec2 o) {
    float k = uAperture.x;
    if (k < 0.5) return 1.0 - smoothstep(0.92, 1.0, length(o));
    if (k < 1.5) {
        float n = max(uAperture.y, 3.0);
        float th = atan(o.y, o.x) - uAperture.z;
        float seg = 2.0 * PI / n;
        float rp = cos(PI / n) / cos(mod(th, seg) - PI / n);
        return 1.0 - smoothstep(rp - 0.08, rp, length(o));
    }
    if (k < 2.5) return 1.0 - smoothstep(-0.08, 0.0, shapeSdf(int(uAperture.y + 0.5), o));
    float s = sin(uAperture.z), c = cos(uAperture.z);
    vec2 q = vec2(c * o.x + s * o.y, -s * o.x + c * o.y);
    return 1.0 - smoothstep(0.92, 1.0, length(vec2(q.x, q.y / max(uAperture.y, 0.05))));
}

// Position-dependent bokeh kernel (mirror of bokeh.rs `kernel`).
float kernelW(vec2 o, vec2 srcPx) {
    vec2 a = srcPx - axisPx();
    float al = length(a);
    float e = min(al / halfDiag(), 1.2);
    vec2 u = al > 1e-3 ? a / al : vec2(1.0, 0.0);
    vec2 t = vec2(-u.y, u.x);
    vec2 q = o;
    if (uSwirl > 0.0) {
        float s = uSwirl * pow(e, 1.7);
        float r2 = dot(o, u) / max(1.0 - 0.3 * s, 0.35);
        float t2 = dot(o, t) / (1.0 + s);
        q = r2 * u + t2 * t;
    }
    float w = apWeight(q);
    if (w <= 0.0) return 0.0;
    if (uCatsEye > 0.0) {
        float d = 0.5 * uCatsEye * e;
        float qr = dot(q, u);
        float qt = dot(q, t);
        float A = length(vec2(qr + d, qt));
        float B = length(vec2(qr - d, qt));
        w *= (1.0 - smoothstep(0.92, 1.0, A)) * (1.0 - smoothstep(0.92, 1.0, B));
    }
    float rho = min(length(q), 1.0);
    if (uBubble > 0.0) {
        float r4 = rho * rho * rho * rho;
        w *= 1.0 + uBubble * (0.2 + 2.2 * r4 - 1.0);
    } else if (uBubble < 0.0) {
        w *= 1.0 + (-uBubble) * ((1.0 - rho * rho) - 1.0);
    }
    if (uOnion > 0.0) w *= 1.0 + uOnion * 0.18 * sin(40.0 * rho);
    return max(w, 0.0);
}

// ---- hashing (pcg3d, same as rust noise.rs)
uvec3 pcg3d(uvec3 v) {
    v = v * 1664525u + 1013904223u;
    v.x += v.y * v.z; v.y += v.z * v.x; v.z += v.x * v.y;
    v ^= v >> 16u;
    v.x += v.y * v.z; v.y += v.z * v.x; v.z += v.x * v.y;
    return v;
}
vec3 hash3(vec2 p, float layer) {
    uvec3 h = pcg3d(uvec3(uvec2(ivec2(floor(p))), uint(layer)));
    return vec3(h >> 8u) * (1.0 / 16777216.0);
}
