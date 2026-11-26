#version 300 es
// Separable 9-tap Gaussian (direction in uDir, texel units).
//#include common
uniform sampler2D uSrc;
uniform vec2 uDir;
in vec2 vUv;
out vec4 fragColor;
void main() {
    const float w[5] = float[](0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162);
    vec3 c = texture(uSrc, vUv).rgb * w[0];
    for (int i = 1; i < 5; i++) {
        c += texture(uSrc, vUv + uDir * float(i) * 1.5).rgb * w[i];
        c += texture(uSrc, vUv - uDir * float(i) * 1.5).rgb * w[i];
    }
    fragColor = vec4(c, 1.0);
}
