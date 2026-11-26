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
uniform sampler2D uVeg;       // live vegetation probability (r), GL uv space (unit 6)
uniform float uVegOn;         // 1 for IR films with a live segmentation map
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
    // parity mirror of rust infrared::veg_index (keep identical): the green-over-red
    // dominance term keeps dark olive paint/fabric from reading as foliage
    return smoothstep(uIrVeg.x, uIrVeg.y, 2.0 * k.g - k.r - k.b) * smoothstep(0.01, 0.06, luma(c))
        * smoothstep(0.03, 0.10, k.g - k.r);
}
float band(float x, float a, float b, float c, float d) {
    return smoothstep(a, b, x) * (1.0 - smoothstep(c, d, x));
}
// parity mirror of rust infrared::skin_index (keep identical)
float skinIndex(vec3 c) {
    float s = c.r + c.g + c.b + 1e-6;
    vec3 k = c / s;
    return band(k.r, 0.35, 0.41, 0.66, 0.72) * band(k.g, 0.16, 0.20, 0.33, 0.36)
        * smoothstep(-0.14, -0.08, k.g - k.b) * smoothstep(0.01, 0.04, luma(c));
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

// parity mirror of rust lens::projection::Warp::source (keep identical)
vec2 fisheyeUv(vec2 uv, out float cover) {
    vec2 c = 0.5 * uOutSize;
    float rOut = uProj.x > 1.5 ? 0.5 * min(uOutSize.x, uOutSize.y) : halfDiag();
    vec2 d = imgPx(uv) - c;
    float r = length(d);
    float rho = min(r / rOut, 1.0);
    float scale = r > 1e-4 ? tan(rho * uProj.y) / uProj.z * rOut / r : uProj.y / uProj.z;
    cover = uProj.x > 1.5 ? clamp((rOut - r) / max(rOut / 400.0, 1.0) + 0.5, 0.0, 1.0) : 1.0;
    return uvFromImgPx(c + d * scale);
}

// parity mirror of rust lens::vignette_factor, at output px
float lensVignette(vec2 pOut) {
    if (uVignette <= 0.0) return 1.0;
    vec2 dv = (pOut - 0.5 * uOutSize) / halfDiag();
    float f = 1.0 / (1.0 + 1.6 * uVignette * dot(dv, dv));
    return f * f;
}

// camera colour as seen at output uv (through the fisheye warp, black outside
// the image circle), like rust sampling the warped frame
vec3 camOut(vec2 uv) {
#ifdef LENS_FISHEYE
    float cv;
    vec2 s = fisheyeUv(uv, cv);
    return cam(s) * cv;
#else
    return cam(uv);
#endif
}

#ifdef FX_OMNI
uniform sampler2D uLin;          // half-res linear camera frame, mipmapped (unit 7)
uniform vec4 uOmni[2];           // per wand: mode, position (deg), reach, strength (0 = unused)
uniform vec4 uOmniTint[2];       // tint rgb, band half-width
uniform vec4 uOmniTint2[2];      // tint2 rgb, spectral (1) or gradient (0)
uniform float uOmniFire[2];      // dispersion of bright light
// rust omni refracts the lens-stage image (warped, vignetted); lin is the raw
// camera frame, so warp and vignette it here
vec3 omniSrc(vec2 q, float lod) {
    vec2 uv = uvFromImgPx(q);
    float k = lensVignette(q);
#ifdef LENS_FISHEYE
    float cv;
    uv = fisheyeUv(uv, cv);
    k *= cv;
#endif
    vec3 s = textureLod(uLin, uv, lod).rgb;
#ifdef LENS_BLUR
    // lin holds highlights expanded for the bokeh gather (linearize.frag);
    // rust omni reads the post-bokeh, compressed image
    s = compressC(s);
#endif
    return s * k;
}
vec3 spectrumRgb(float h) {
    return clamp(abs(fract(h + vec3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, 0.0, 1.0);
}
// rust omni::diamond_tex / omni::hole
float omniDiamond(float al, float ac) {
    return 0.5 * (abs(2.0 * fract((al + ac) / 0.05) - 1.0) + abs(2.0 * fract((al - ac) / 0.05) - 1.0));
}
float omniHole(float al, float ac) {
    vec2 uv = fract(vec2(al, ac) / 0.09) - 0.5;
    return 1.0 - smoothstep(0.006, 0.042, length(uv) * 0.09);
}
// parity mirror of rust effects::omni::Omni (weight + refract; keep identical)
vec3 omni(vec3 c, vec2 p, int k) {
    vec4 W = uOmni[k];
    if (W.w <= 0.0) return c;
    int mode = int(W.x + 0.5);
    float width = uOmniTint[k].w;
    float hd = halfDiag();
    float ang = radians(W.y);
    float sa = sin(ang), ca = cos(ang);
    float tx = abs(ca) > 1e-6 ? 0.5 * uOutSize.x / abs(ca) : 1e30;
    float ty = abs(sa) > 1e-6 ? 0.5 * uOutSize.y / abs(sa) : 1e30;
    vec2 a = 0.5 * uOutSize + vec2(ca, sa) * min(tx, ty);
    vec2 n = -vec2(ca, sa);
    vec2 t = vec2(-n.y, n.x);
    vec2 d = p - a;
    float along = dot(d, n) / hd;
    float across = dot(d, t) / hd;
    float r = W.z;
    float acn = across / width;
    float m = (1.0 - smoothstep(0.35 * r, r, along)) * exp(-acn * acn * 2.0);
    if (mode == 9) m *= 1.0 - omniHole(along, across);
    else if (mode == 13) m *= smoothstep(0.2, 0.45, abs(acn));
    m = min(m * W.w * 0.9, 1.0);
    if (m <= 1e-3) return c;
    float qa = 0.5 * r + 0.6 * (0.5 * r - along);
    // σ = SOFT·hd output px; lin texel = 2 px → mip level of a similar footprint
    float lod = max(log2(2.5 * 0.012 * hd / 2.0), 0.0);
    float dsp = 0.006 * hd;
    vec3 acc = vec3(0.0);
    vec3 fire = vec3(0.0);
    for (int i = 0; i < 8; i++) {
        float s = (float(i) + 0.5) / 8.0 - 0.5;
        vec2 q2;
        vec2 disp;
        if (mode == 6) {
            float cell = floor(across / 0.1);
            q2 = vec2(along + s * 0.02, cell * 0.1 * 0.35 + (across - cell * 0.1)); disp = t;
        } else if (mode == 7) { q2 = vec2(along * 0.9, across * 0.4 + s * 0.25); disp = t; }
        else if (mode == 8) {
            float cr = 0.45 * r;
            float dl = along - cr;
            float rr = length(vec2(dl, across)) / width;
            float kb = 0.35 + 0.5 * rr * rr + s * 0.03;
            q2 = vec2(cr - dl * kb, across * kb); disp = n;
        } else if (mode == 10) {
            float tx = omniDiamond(along, across);
            q2 = vec2(along + 0.02 * tx + s * 0.01, across + 0.02 * (1.0 - tx)); disp = t;
        } else if (mode == 11) { q2 = vec2(along, across + s * 0.01); disp = t; }
        else if (mode == 12) {
            bool ev = i % 2 == 0;
            q2 = vec2(qa * (ev ? 1.0 : 0.6) + s * 0.02, across + (ev ? 0.08 : -0.08)); disp = n;
        } else if (mode == 5) { q2 = vec2(qa + 0.05 * sin(across * 40.0 + s * 6.0), across + s * 0.1); disp = n; }
        else if (mode == 1) { q2 = vec2(qa, across + s * 0.16); disp = n; }
        else if (mode == 2 || mode == 13) {
            float kk = float(i % 4) * 1.5707964 + 0.6;
            float o = 0.05 + 0.02 * float(i / 4);
            q2 = vec2(qa + sin(kk) * o, across + cos(kk) * o); disp = t;
        } else if (mode == 3) {
            float g = 2.2 * clamp(1.0 - along / r, 0.0, 1.0) + s * 0.35;
            float sn = sin(g), cs = cos(g);
            q2 = vec2(qa * cs - across * sn, qa * sn + across * cs); disp = t;
        } else if (mode == 4 || mode == 9) { q2 = vec2(qa, across + s * 0.12); disp = n; }
        else { q2 = vec2(qa + s * 0.12, across); disp = t; }
        vec2 q = a + (n * q2.x + t * q2.y) * hd;
        vec3 mid = omniSrc(q, lod);
        acc += vec3(omniSrc(q + disp * dsp, lod).r, mid.g, omniSrc(q - disp * dsp, lod).b);
        float y = luma(mid);
        fire += spectrumRgb((float(i) + 0.5) / 8.0) * (smoothstep(0.6, 1.0, y) * y);
    }
    vec3 rc = acc / 8.0;
    float h = 0.5 + across / (2.0 * width);
    vec3 tint = uOmniTint2[k].w > 0.5
        ? 0.25 + 1.3 * spectrumRgb(h)
        : mix(uOmniTint[k].rgb, uOmniTint2[k].rgb, clamp(h, 0.0, 1.0));
    float fm = (fire.r + fire.g + fire.b) / 3.0;
    float tex = mode == 10 ? 0.85 + 0.3 * omniDiamond(along, across) : 1.0;
    rc = max(rc * tint * tex + uOmniFire[k] * 3.0 / 8.0 * (fire - fm), 0.0);
    rc = rc * 1.08 + 0.12 * luma(rc) + 0.01;
    return mix(c, rc, m);
}
#endif

#ifdef FX_LEAK
// parity mirror of rust effects::{leak_center, LightLeak::apply} (keep identical)
vec3 leakBlob(vec2 q, float aspect, vec4 b, vec3 col, vec2 pos) {
    if (b.w <= 0.0) return vec3(0.0);
    vec2 ctr = b.xy;
    if (pos.y > 0.5) {
        // edge_point(position, aspect, 1)
        float a = radians(pos.x);
        vec2 o = vec2(cos(a), sin(a));
        float t = min(abs(o.x) > 1e-6 ? 0.5 * aspect / abs(o.x) : 1e30, abs(o.y) > 1e-6 ? 0.5 / abs(o.y) : 1e30);
        vec2 an = vec2(0.5 * aspect, 0.5) + o * t;
        ctr = vec2((an.x - o.y * b.x + o.x * b.y) / aspect, an.y + o.x * b.x + o.y * b.y);
    }
    vec2 d = vec2((q.x - ctr.x) * aspect, q.y - ctr.y) / b.z;
    return col * exp(-dot(d, d) * 2.5) * b.w;
}
vec3 leak(vec2 uv) {
    vec2 q = vec2(uv.x, 1.0 - uv.y);   // image uv, y down
    float aspect = uOutSize.x / uOutSize.y;
    vec3 acc = vec3(0.0);
    for (int k = 0; k < 8; k++) acc += leakBlob(q, aspect, uLeakBlob[k], uLeakCol[k], uLeakPos[k]);
    return acc;
}
#endif

void main() {
    // uvS: where this output pixel looks in the camera frame (differs for fisheye)
    vec2 uvS = vUv;
    float cover = 1.0;
#ifdef LENS_FISHEYE
    uvS = fisheyeUv(vUv, cover);
#endif
    vec2 p = imgPx(uvS);
    vec2 pOut = imgPx(vUv);
    vec3 c;
    if (abs(uCaLateral) > 1e-5) {
        // lateral CA is radial in the output image (rust runs it after the warp)
        vec2 ax = axisPx();
        vec2 d = pOut - ax;
        float rho2 = dot(d, d) / (halfDiag() * halfDiag());
        c = vec3(camOut(uvFromImgPx(ax + d * (1.0 + uCaLateral * rho2))).r, cam(uvS).g,
                 camOut(uvFromImgPx(ax + d * (1.0 - uCaLateral * rho2))).b);
    } else {
        c = cam(uvS);
    }
#ifdef LENS_BLUR
    float r = coc(p);
    vec4 lens = texture(uLens, uvS);
    if (r > 0.5) c = mix(c, compressC(lens.rgb), smoothstep(0.5, 1.5, r));
    // sharp pixel: blurred content in front still spreads over it (alpha = coverage)
    else if (lens.a > 0.0) c = mix(c, compressC(lens.rgb), lens.a);
#endif
    vec3 wide = texture(uBlur, uvS).rgb * cover;
#ifdef LENS_GLOW
    float g = clamp(uGlow.x, 0.0, 1.0);
    float bloom = smoothstep(0.35, 1.0, luma(wide));
    c = c * (1.0 - 0.55 * g) + wide * 0.55 * g + wide * bloom * g * 0.6;
#endif
    if (uVeil > 0.0) c = c * (1.0 - 0.5 * uVeil) + vec3(luma(wide)) * uVeil * 0.6;
    vec2 dv = (pOut - 0.5 * uOutSize) / halfDiag();
    float r2 = dot(dv, dv);
    c *= lensVignette(pOut);
    c *= cover;
#ifdef FX_OMNI
    c = omni(c, pOut, 0);
    c = omni(c, pOut, 1);
#endif
    // parity mirror of rust effects::Vignette (roundness 1); an exposure-slot
    // effect, so it also darkens the optical-slot omni
    if (uFxVignette > 0.0) { float f = 1.0 / (1.0 + 2.5 * uFxVignette * r2 * r2); c *= f; }

    vec3 scene = c;
    c *= uExposure;
#ifdef FX_LEAK
    c += leak(vUv);
#endif
#ifdef FILM
#ifdef IR
    {
        // own-pixel mask gated by the regional (wide) one; mirrors infrared::gate
        float vy = 1.0 - vUv.y;
        float veg;
        if (uVegOn > 0.5) {
            // parity mirror of rust infrared::masks + gate_by_seg: seg_gate (smoothstep
            // 0.15..0.5 of p_veg) multiplies BOTH the fine and the regional vegetation
            // mask before infrared::gate combines them
            float s = smoothstep(0.15, 0.5, texture(uVeg, uvS).r);
            veg = vegIndex(scene) * s * smoothstep(0.05, 0.35, vegIndex(wide) * s);
        } else {
            veg = vegIndex(scene) * smoothstep(0.05, 0.35, vegIndex(wide));
        }
        float sky = skyIndex(scene, vy) * smoothstep(0.05, 0.35, skyIndex(wide, vy));
        float base = max(0.75 * c.r + 0.25 * c.g + 0.4 * (c.r - c.b), 0.0);
        // skin reflects far more NIR than red (rust infrared::pseudo_ir)
        base += (max(c.r * uIr.w, base) - base) * skinIndex(c);
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
        float amp = uGrain.x * 0.10 * clamp(sigmaTrue / 0.6, 0.15, 1.0) * 1.6;
        vec2 gp = pOut / s;
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
