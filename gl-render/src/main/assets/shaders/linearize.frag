#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Camera OES → linear light, lateral CA, highlight expansion. Alpha = CoC (full-res px).
//#include common
uniform samplerExternalOES uCamera;
in vec2 vUv;
out vec4 fragColor;

vec3 cam(vec2 uv) { return srgbToLinear(texture(uCamera, (uTexMatrix * vec4(uv, 0.0, 1.0)).xy).rgb); }

void main() {
    vec2 p = imgPx(vUv);
    vec3 c;
    if (abs(uCaLateral) > 1e-5) {
        vec2 ax = axisPx();
        vec2 d = p - ax;
        float rho2 = dot(d, d) / (halfDiag() * halfDiag());
        c.r = cam(uvFromImgPx(ax + d * (1.0 + uCaLateral * rho2))).r;
        c.g = cam(vUv).g;
        c.b = cam(uvFromImgPx(ax + d * (1.0 - uCaLateral * rho2))).b;
    } else {
        c = cam(vUv);
    }
#ifdef LENS_BLUR
    c = expandC(c);
#endif
    fragColor = vec4(c, coc(p));
}
