/**
 * Shader da intro (GLSL ES 3.00). Base: AGSL `HERO_BEAM_AGSL` do Prismlock (quatro holofotes
 * convergindo + holofotes orbitais atrás do título), traduzido e refinado:
 *  - dither (sem faixas nos degradês escuros), resolução independente do monitor;
 *  - fase de DERRETIMENTO (`uMelt`): em vez do "furo" radial do Prismlock, o hero se dissolve em
 *    blur a partir do botão: o campo (distância + blobs suaves) abre transparência com borda difusa,
 *    o conteúdo desce pouco, o título é desfocado por viés de mip e a luz atravessa até o Hub.
 *    Custo: 1 chamada de hero(), fbm de 2 oitavas, sem dispersão cromática.
 * Saída em alfa pré-multiplicado (canvas `premultipliedAlpha`).
 */
export const HERO_VERT = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

export const HERO_FRAG = `#version 300 es
precision highp float;
uniform vec2 uRes;
uniform vec2 uConv;
uniform vec2 uTitleCenter;
uniform float uTime;
uniform float uAmb;
uniform float uMelt;
uniform vec2 uMeltOrigin;
uniform sampler2D uTitle;
uniform vec4 uTitleRect;
uniform float uTitleOn;
out vec4 outColor;

float hash21(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}
float vnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  float a = hash21(i);
  float b = hash21(i + vec2(1.0, 0.0));
  float c = hash21(i + vec2(0.0, 1.0));
  float d = hash21(i + vec2(1.0, 1.0));
  return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.5;
  for (int i = 0; i < 2; i++) {
    v += a * vnoise(p);
    p = p * 2.03 + vec2(17.1, 9.7);
    a *= 0.5;
  }
  return v;
}

float spotlightCone(vec2 uv, vec2 origin, vec2 aim, float startSigma, float endSigma) {
  vec2 seg = aim - origin;
  float lenSq = max(dot(seg, seg), 0.0001);
  float len = sqrt(lenSq);
  vec2 dir = seg / len;
  float along = dot(uv - origin, dir);
  float h = clamp(along / len, 0.0, 1.18);
  vec2 proj = origin + dir * (h * len);
  float dPerp = length(uv - proj);
  float sigma = mix(startSigma, endSigma, clamp(h, 0.0, 1.0));
  float radialCone = exp(-(dPerp * dPerp) / max(2.0 * sigma * sigma, 0.0005));
  float axialFade = smoothstep(-0.08, 0.22, h) * (0.28 + 0.72 * smoothstep(0.0, 1.0, h));
  float focusPool = exp(-dot(uv - aim, uv - aim) / max(2.2 * endSigma * endSigma, 0.002));
  return radialCone * axialFade * 0.75 + focusPool * 0.55;
}

float revolvingSpotlight(vec2 uvTitle, float angle, vec2 axisScale, float sigmaLong, float sigmaCross) {
  float c = cos(angle);
  float s = sin(angle);
  vec2 orbPos = vec2(c * axisScale.x, s * axisScale.y);
  vec2 d = uvTitle - orbPos;
  vec2 beamDir = normalize(-orbPos + vec2(s * 0.14, -c * 0.14));
  float projAlong = dot(d, beamDir);
  float projCross = d.x * (-beamDir.y) + d.y * beamDir.x;
  float longFalloff = exp(-(projAlong * projAlong) / max(2.0 * sigmaLong * sigmaLong, 0.002));
  float crossFalloff = exp(-(projCross * projCross) / max(2.0 * sigmaCross * sigmaCross, 0.002));
  float halo = exp(-dot(d, d) / max(2.0 * sigmaCross * sigmaCross * 1.8, 0.002));
  return longFalloff * crossFalloff * 0.85 + halo * 0.40;
}

vec3 hero(vec2 fragCoord) {
  float minDim = max(min(uRes.x, uRes.y), 1.0);
  vec2 uv = (fragCoord - uConv) / minDim;

  float conv = clamp(uTime / 0.22, 0.0, 1.0);
  float easeConv = conv * conv * (3.0 - 2.0 * conv);
  float uniteGlow = smoothstep(0.15, 0.24, uTime);
  float introFadeOut = 1.0 - smoothstep(0.20, 0.36, uTime);

  vec2 c1 = (vec2(0.0, 0.0) - uConv) / minDim * 1.05;
  vec2 c2 = (vec2(uRes.x, 0.0) - uConv) / minDim * 1.05;
  vec2 c3 = (vec2(0.0, uRes.y) - uConv) / minDim * 1.05;
  vec2 c4 = (uRes - uConv) / minDim * 1.05;

  vec2 aim1 = mix(c1 * 0.42, vec2(-0.02, -0.02), easeConv);
  vec2 aim2 = mix(c2 * 0.42, vec2( 0.03, -0.02), easeConv);
  vec2 aim3 = mix(c3 * 0.42, vec2(-0.02,  0.03), easeConv);
  vec2 aim4 = mix(c4 * 0.42, vec2( 0.03,  0.02), easeConv);

  float startSig = mix(0.22, 0.16, easeConv);
  float endSig = mix(0.18, 0.105, easeConv);

  float s1 = spotlightCone(uv, c1, aim1, startSig, endSig);
  float s2 = spotlightCone(uv, c2, aim2, startSig, endSig);
  float s3 = spotlightCone(uv, c3, aim3, startSig, endSig);
  float s4 = spotlightCone(uv, c4, aim4, startSig, endSig);

  vec3 introCol = vec3(0.0);
  introCol += vec3(0.00, 0.88, 1.00) * s1 * 0.56;
  introCol += vec3(0.55, 0.36, 0.96) * s2 * 0.56;
  introCol += vec3(0.23, 0.51, 0.96) * s3 * 0.56;
  introCol += vec3(0.95, 0.25, 0.48) * s4 * 0.56;

  float pool = exp(-dot(uv, uv) / 0.045);
  introCol += vec3(0.65, 0.82, 1.00) * pool * uniteGlow * 0.40;
  introCol *= introFadeOut;

  float ambientFade = smoothstep(0.30, 0.65, uTime);
  vec2 uvTitle = (fragCoord - uTitleCenter) / minDim;

  float r1 = revolvingSpotlight(uvTitle,  uAmb,               vec2(0.32, 0.13), 0.36, 0.16);
  float r2 = revolvingSpotlight(uvTitle, -uAmb * 0.85 + 2.1,  vec2(0.36, 0.15), 0.40, 0.18);
  float r3 = revolvingSpotlight(uvTitle,  uAmb * 1.10 + 4.2,  vec2(0.28, 0.11), 0.34, 0.15);
  float r4 = revolvingSpotlight(uvTitle, -uAmb * 0.70 + 5.2,  vec2(0.38, 0.16), 0.42, 0.19);

  vec3 ambientCol = vec3(0.0);
  ambientCol += vec3(0.00, 0.84, 1.00) * r1 * 0.30;
  ambientCol += vec3(0.56, 0.34, 0.96) * r2 * 0.30;
  ambientCol += vec3(0.92, 0.24, 0.50) * r3 * 0.26;
  ambientCol += vec3(0.22, 0.48, 0.98) * r4 * 0.26;
  ambientCol *= ambientFade;

  return clamp(introCol + ambientCol, 0.0, 1.0);
}

vec4 titleAt(vec2 fc, float blurBias) {
  vec2 tc = (fc - uTitleRect.xy) / uTitleRect.zw;
  if (uTitleOn < 0.5 || tc.x < 0.0 || tc.y < 0.0 || tc.x > 1.0 || tc.y > 1.0) return vec4(0.0);
  return texture(uTitle, tc, blurBias); // viés de mip = desfoque barato do título durante o derretimento
}

void main() {
  vec2 fc = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y);
  float minDim = max(min(uRes.x, uRes.y), 1.0);

  vec2 f2 = fc;
  float alpha = 1.0;
  float glow = 0.0;
  float sag = 0.0;
  if (uMelt > 0.0005) {
    // Derretimento suave: campo largo (distância + 2 oitavas de blob, sem listras), borda difusa
    // em vez de linha, deslocamento pequeno; o desfoque domina (título por viés de mip, luz já é macia).
    vec2 pn = fc / minDim;
    float maxR = length(max(uMeltOrigin, uRes - uMeltOrigin));
    float dist = length(fc - uMeltOrigin) / maxR;
    float blobs = fbm(pn * 2.6 + 5.0);
    float field = dist * 0.66 + blobs * 0.34;
    float th = uMelt * 1.18 - 0.14;
    float edge = field - th;
    float k = edge < 0.0 ? 1.0 : 1.0 - smoothstep(0.0, 0.5, edge);
    sag = k * k;
    float ease = smoothstep(0.0, 0.3, uMelt);
    f2 = fc - vec2(0.0, sag * minDim * 0.045 * ease);
    alpha = smoothstep(0.0, 0.2, edge);
    glow = 4.0 * alpha * (1.0 - alpha);
  }

  vec3 base = hero(f2);
  vec4 tt = titleAt(f2, sag * 4.5);
  base = base * (1.0 - tt.a) + tt.rgb;

  vec3 rgb = base * alpha + vec3(0.86, 0.93, 1.0) * glow * 0.14;
  float a = clamp(alpha + glow * 0.14, 0.0, 1.0);
  float dither = (hash21(gl_FragCoord.xy + fract(uTime * 13.0)) - 0.5) / 255.0;
  rgb += dither * a;
  outColor = vec4(clamp(rgb, 0.0, 1.0), a);
}`;
