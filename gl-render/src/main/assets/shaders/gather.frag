#version 300 es
// Scatter-as-gather bokeh at reduced resolution (mirror of bokeh.rs).
//#include common
uniform sampler2D uSrc;   // linearize output: rgb (highlight-expanded linear)
uniform float uTaps;
in vec2 vUv;
out vec4 fragColor;

void main() {
    vec2 P = imgPx(vUv);
    vec4 c0 = vec4(texture(uSrc, vUv).rgb, coc(P));
    float reach = uRmax * uOutSize.x + 1.0;
    // center sample represents one low-res texel (≈ 4 full px²)
    float aC = 4.0;
    float wc = aC / max(max(c0.a, 0.5) * max(c0.a, 0.5) * uApArea, aC);
    vec3 acc = c0.rgb * wc;
    float wsum = wc;
    int n = int(uTaps);
    float aTap = PI * reach * reach / float(n);
    // per-pixel spiral rotation: turns structured sampling streaks into fine noise (hidden by grain)
    float jitter = hash3(P, 7.0).x * 6.2831853;
    // Taps are ~reach*sqrt(pi/n) apart; read a mip level whose footprint covers
    // that gap so each tap averages its neighbourhood instead of point-sampling.
    // (source texel = 2 output px)
    float spacing = reach * sqrt(PI / float(n));
    for (int i = 0; i < 64; i++) {
        if (i >= n) break;
        float fi = float(i);
        float rho = sqrt((fi + 0.5) / float(n));
        float th = fi * 2.39996323 + jitter;
        vec2 d = reach * rho * vec2(cos(th), sin(th));
        vec2 S = P + d;
        float r = max(coc(S), 0.5);
        // footprint limited by the source's own blur so in-focus detail stays sharp
        float lod = log2(max(min(spacing, r) * 0.5, 1.0));
        vec3 s = textureLod(uSrc, uvFromImgPx(S), lod).rgb;
        if (length(d) > r + 1.0) continue;
        float w = kernelW(-d / r, S) / (r * r * uApArea) * aTap;
        acc += s * w;
        wsum += w;
    }
    fragColor = vec4(acc / max(wsum, 1e-6), c0.a);
}
