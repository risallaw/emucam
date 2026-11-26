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
    float nP = uDepthOn > 0.5 ? nearAt(P) : 0.0;
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
    // near field (bokeh.rs `near_out`): blurred content in front spreads over
    // this pixel however sharp it is, so near objects get soft silhouettes
    vec3 nearAcc = vec3(0.0);
    float nearW = 0.0;
    for (int i = 0; i < 64; i++) {
        if (i >= n) break;
        float fi = float(i);
        float rho = sqrt((fi + 0.5) / float(n));
        float th = fi * 2.39996323 + jitter;
        vec2 d = reach * rho * vec2(cos(th), sin(th));
        vec2 S = P + d;
        float r = max(coc(S), 0.5);
        // reject before any further fetch: in sharp areas most taps land here
        if (length(d) > r + 1.0) continue;
        // a blurred background can't spread over a sharper object in front (bokeh.rs occlusion)
        float occ = 1.0;
        float nS = uDepthOn > 0.5 ? nearAt(S) : 0.0;
        if (uDepthOn > 0.5 && nS < nP - 0.03) occ = min(max(c0.a, 0.5) / r, 1.0);
        // footprint limited by the source's own blur so in-focus detail stays sharp
        // (explicit LOD: safe inside non-uniform control flow)
        float lod = log2(max(min(spacing, r) * 0.5, 1.0));
        vec3 s = textureLod(uSrc, uvFromImgPx(S), lod).rgb;
        float wk = kernelW(-d / r, S) / (r * r * uApArea) * aTap;
        float w = wk * occ;
        acc += s * w;
        wsum += w;
        if (uDepthOn > 0.5 && nS > nP + 0.03) {
            // share not already in this pixel's own blur (no double counting)
            float sh = wk * clamp(1.0 - c0.a / r, 0.0, 1.0); // bokeh.rs: 1 - coc_dst / rr
            nearAcc += s * sh;
            nearW += sh;
        }
    }
    vec3 res = acc / max(wsum, 1e-6);
    // coverage doubled: ~half a near disc covers its own silhouette (see bokeh.rs composite)
    float cov = nearW > 1e-4 ? min(1.0, 2.0 * nearW) : 0.0;
    vec3 nearC = cov > 0.0 ? nearAcc / nearW : res;
    // alpha = near-field coverage. final.frag only takes rgb where this pixel is itself
    // blurred, so for a sharp pixel rgb carries the pure near colour and alpha how much
    // of it to lay over the full-res image (bokeh.rs overlays the near field everywhere)
    fragColor = c0.a > 0.5 ? vec4(mix(res, nearC, cov), cov) : vec4(nearC, cov);
}
