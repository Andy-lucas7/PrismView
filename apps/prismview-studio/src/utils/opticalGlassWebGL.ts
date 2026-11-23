// Hardware-Accelerated WebGL2 Optical Liquid Glass Engine
// Architecture:
// - 1 single shared offscreen WebGL2 context (zero browser context exhaustion, automatic context-loss recovery)
// - 4-Level Anti-Aliased Downsample Pyramid + Dense 15-Tap Exact Continuous Gaussian Blur (1.0 texel step ALWAYS:
//   ZERO ghosting / ZERO image multiplication at 1px, 5px, 20px, or 80px!)
// - Real analytic Rounded-Rect SDF & 3D Sobel Glyph Normal optical lens refraction (Snell curvature + spherical dome)
// - True 3-channel RGB prismatic dispersion, tactile frosted micro-grain, directional Fresnel specular rim & optical transmission

export interface BackdropSourceInfo {
  /** Unique identity string; changing it triggers static re-upload */
  key: string;
  kind: "video" | "image" | "canvas";
  element: HTMLVideoElement | HTMLImageElement | HTMLCanvasElement;
  intrinsicW: number;
  intrinsicH: number;
  /** Exact viewport CSS coordinates where the backdrop media is drawn */
  drawLeft: number;
  drawTop: number;
  drawW: number;
  drawH: number;
  /** Monotonic frame token (incremented per rAF while video or 3D scene is actively rendering) */
  frameToken: number;
  /** True for animated canvas backdrops (e.g. SCENE_3D) that update via shared frame ticks */
  live?: boolean;
  /** Optional opaque background below a transparent 3D canvas (composited on GPU before blur/refraction) */
  underlay?: BackdropSourceInfo;
}

export interface OpticalGlassRenderParams {
  mode: "plate" | "glyph";
  backdrop: BackdropSourceInfo;
  viewportW: number;
  viewportH: number;
  /** Center of the glass element in viewport CSS pixels */
  centerX: number;
  centerY: number;
  /** Rendered CSS size of the glass element (before scaleY/rotation) */
  width: number;
  height: number;
  rotationDeg: number;
  scaleY: number;
  /** Optical parameters */
  blurPx: number;
  /** Unscaled logical blurPx (0..80) so blurMix ramp matches native engine 1:1 across viewport sizes */
  rawBlurPx?: number;
  refractionScale: number;
  bevelPx: number;
  radiusPx: number;
  chromaticAberration: number;
  frostOpacity: number;
  tintRgb: [number, number, number];
  noiseOpacity: number;
  specularIntensity: number;
  borderOpacity: number;
  brightness: number;
  contrast: number;
  saturation: number;
  opacity: number;
  /** Only for mode === "glyph" */
  glyphCanvas?: HTMLCanvasElement | null;
  /** Blurred heightfield for the GPU Sobel pass that builds the glyph normal map. */
  glyphHeightCanvas?: HTMLCanvasElement | null;
  /** Heightfield blur sigma (px in heightCanvas space) for slope-invariant Sobel gain. */
  glyphHeightSigmaPx?: number;
  /** CSS padding around glyph box so dome curvature matches tight native glyph bounds 1:1 */
  glyphPadPx?: number;
  glyphKey?: string;
  pigmentRgba?: [number, number, number, number];
}

/** GPU Sobel: sharp alpha + blurred height -> packed R=nx, G=ny, B=edgeSlope, A=sharpAlpha (parity with native glass_glyph.wgsl). */
const GLYPH_PACK_FRAG_SRC = `#version 300 es
precision highp float;
uniform sampler2D uSharp;
uniform sampler2D uHeight;
uniform float uBevelSigma;
out vec4 fragColor;

float H(ivec2 p, ivec2 sz) {
  if (p.x < 0 || p.y < 0 || p.x >= sz.x || p.y >= sz.y) return 0.0;
  return texelFetch(uHeight, p, 0).a;
}

void main() {
  ivec2 sz = textureSize(uHeight, 0);
  ivec2 p = ivec2(gl_FragCoord.xy);
  float sharpA = texelFetch(uSharp, p, 0).a;
  if (sharpA * 255.0 < 3.0) {
    fragColor = vec4(128.0 / 255.0, 128.0 / 255.0, 0.0, 0.0);
    return;
  }
  // Textures are uploaded Y-flipped: canvas y-1 (up) == texel y+1.
  float tl = H(p + ivec2(-1,  1), sz);
  float t  = H(p + ivec2( 0,  1), sz);
  float tr = H(p + ivec2( 1,  1), sz);
  float l  = H(p + ivec2(-1,  0), sz);
  float r  = H(p + ivec2( 1,  0), sz);
  float bl = H(p + ivec2(-1, -1), sz);
  float b  = H(p + ivec2( 0, -1), sz);
  float br = H(p + ivec2( 1, -1), sz);
  float gx = (tl + 2.0 * l + bl) - (tr + 2.0 * r + br);
  float gy = (tl + 2.0 * t + tr) - (bl + 2.0 * b + br);
  float slope = length(vec2(gx, gy));
  if (slope > 0.002) {
    float gain = min(1.0, slope * max(2.2, uBevelSigma * 0.9));
    vec2 n = vec2(gx, gy) / slope * gain;
    fragColor = vec4(clamp(128.0 + n * 126.0, 0.0, 255.0) / 255.0, gain, sharpA);
  } else {
    fragColor = vec4(128.0 / 255.0, 128.0 / 255.0, 0.0, sharpA);
  }
}
`;

type GlCanvas = HTMLCanvasElement | OffscreenCanvas;

const VERT_SRC = `#version 300 es
precision highp float;
const vec2 POS[4] = vec2[4](
  vec2(-1.0, -1.0),
  vec2( 1.0, -1.0),
  vec2(-1.0,  1.0),
  vec2( 1.0,  1.0)
);
out vec2 vUv;
void main() {
  vec2 p = POS[gl_VertexID];
  vUv = p * 0.5 + 0.5;
  gl_Position = vec4(p, 0.0, 1.0);
}
`;

// Viewport-aligned backdrop compositor (composites underlay + transparent 3D canvas on GPU, zero CPU copy)
const COMP_FRAG_SRC = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uTex;
uniform vec2 uBgScale;
uniform vec2 uBgOffset;
uniform float uForceOpaque;
out vec4 fragColor;

void main() {
  vec2 cssUv = vec2(vUv.x, 1.0 - vUv.y);
  vec2 srcCss = cssUv * uBgScale + uBgOffset;
  if (srcCss.x < 0.0 || srcCss.x > 1.0 || srcCss.y < 0.0 || srcCss.y > 1.0) {
    fragColor = vec4(0.0);
    return;
  }
  vec2 texUv = clamp(vec2(srcCss.x, 1.0 - srcCss.y), vec2(0.0005), vec2(0.9995));
  vec4 c = texture(uTex, texUv);
  if (uForceOpaque >= 0.5) {
    fragColor = vec4(c.rgb, 1.0);
  } else {
    fragColor = c;
  }
}
`;

// Anti-aliased 4-tap bilinear 2x box/tent downsample (16 source texels averaged cleanly, preserves premultiplied RGBA)
const DOWNSAMPLE_FRAG_SRC = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uTex;
uniform vec2 uHalfTexel;
out vec4 fragColor;

void main() {
  vec4 c = texture(uTex, clamp(vUv + vec2(-uHalfTexel.x, -uHalfTexel.y), 0.001, 0.999));
  c += texture(uTex, clamp(vUv + vec2( uHalfTexel.x, -uHalfTexel.y), 0.001, 0.999));
  c += texture(uTex, clamp(vUv + vec2(-uHalfTexel.x,  uHalfTexel.y), 0.001, 0.999));
  c += texture(uTex, clamp(vUv + vec2( uHalfTexel.x,  uHalfTexel.y), 0.001, 0.999));
  fragColor = c * 0.25;
}
`;

// Dense 15-tap exact continuous separable Gaussian blur at EXACT 1.0-texel spacing (ZERO skipped texels, ZERO ghosting!)
const GAUSS_FRAG_SRC = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uTex;
uniform vec2 uAxisStep;
uniform float uSigma;
out vec4 fragColor;

void main() {
  float inv2s2 = 0.5 / max(uSigma * uSigma, 0.08);
  vec4 sum = texture(uTex, vUv);
  float wSum = 1.0;

  for (int i = 1; i <= 7; i++) {
    float fi = float(i);
    float w = exp(-fi * fi * inv2s2);
    vec2 off = uAxisStep * fi;
    sum += (texture(uTex, clamp(vUv + off, 0.001, 0.999))
          + texture(uTex, clamp(vUv - off, 0.001, 0.999))) * w;
    wSum += 2.0 * w;
  }

  fragColor = sum / wSum;
}
`;

const GLASS_FRAG_SRC = `#version 300 es
precision highp float;

in vec2 vUv;

uniform int uMode; // 0 = rounded plate, 1 = 3D glyph
uniform sampler2D uSharpTex;
uniform sampler2D uBlurTex;
uniform sampler2D uGlyphTex;

uniform vec2 uViewportSize;
uniform vec2 uLayerCenter;
uniform vec2 uLayerSize;
uniform float uRotationRad;
uniform float uScaleY;

uniform vec2 uBgScale;
uniform vec2 uBgOffset;

uniform float uBlurMix;
uniform float uRefractionPx;
uniform float uBevelPx;
uniform float uRadiusPx;
uniform float uChromaPx;
uniform float uFrostOpacity;
uniform vec3 uTintColor;
uniform float uNoiseOpacity;
uniform float uSpecular;
uniform float uBorderOpacity;
uniform float uBrightness;
uniform float uContrast;
uniform float uSaturation;
uniform float uOpacity;
uniform vec4 uPigmentColor;
uniform float uOpaqueBackdrop;
uniform float uGlyphPadPx;

out vec4 fragColor;

float sdRoundedBox(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + vec2(r);
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

float hash12(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

vec4 sampleBackdrop(vec2 vpPx) {
  vec2 cssUv = vpPx / max(uViewportSize, vec2(1.0));
  vec2 srcCss = cssUv * uBgScale + uBgOffset;
  vec2 texUv = clamp(vec2(srcCss.x, 1.0 - srcCss.y), vec2(0.0015), vec2(0.9985));
  bool opaqueBg = uOpaqueBackdrop >= 0.5;
  vec4 sharp = texture(uSharpTex, texUv);
  if (opaqueBg) sharp.a = 1.0;
  if (uBlurMix <= 0.001) return sharp;
  vec4 blurred = texture(uBlurTex, texUv);
  if (opaqueBg) blurred.a = 1.0;
  return mix(sharp, blurred, uBlurMix);
}

void main() {
  // CSS top-down local pixel coordinates relative to layer center
  vec2 localPx = vec2(
    (vUv.x - 0.5) * uLayerSize.x,
    (0.5 - vUv.y) * uLayerSize.y
  );

  float alpha = 1.0;
  vec2 normal = vec2(0.0);
  float edgeStrength = 0.0;
  float borderMask = 0.0;
  float lensMag = 0.0;

  if (uMode == 0) {
    vec2 halfSize = uLayerSize * 0.5;
    float r = clamp(uRadiusPx, 0.0, min(halfSize.x, halfSize.y) - 0.5);
    float d = sdRoundedBox(localPx, halfSize, r);
    alpha = 1.0 - smoothstep(-0.75, 0.75, d);
    if (alpha <= 0.002) discard;

    float depth = max(0.0, -d);
    float bevel = clamp(uBevelPx, 3.0, min(halfSize.x, halfSize.y) * 0.92);

    // Analytic gradient of SDF (outward normal in CSS top-down space)
    float eps = 0.85;
    float gx = sdRoundedBox(localPx + vec2(eps, 0.0), halfSize, r) - sdRoundedBox(localPx - vec2(eps, 0.0), halfSize, r);
    float gy = sdRoundedBox(localPx + vec2(0.0, eps), halfSize, r) - sdRoundedBox(localPx - vec2(0.0, eps), halfSize, r);
    vec2 grad = vec2(gx, gy);
    float glen = length(grad);
    vec2 outN = glen > 0.0001 ? grad / glen : vec2(0.0);

    // Curved convex bevel refraction + gentle spherical dome across center
    vec2 domeN = localPx / max(halfSize, vec2(1.0));
    if (depth < bevel) {
      float t = 1.0 - depth / bevel; // 1 at rim, 0 at inner bevel edge
      float bevelCurve = pow(t, 1.25) * sin(t * 1.5707963);
      normal = outN * bevelCurve + domeN * 0.14;
      lensMag = bevelCurve;
    } else {
      normal = domeN * 0.14;
      lensMag = 0.0;
    }

    edgeStrength = exp(-depth / max(2.0, bevel * 0.34));
    borderMask = 1.0 - smoothstep(0.0, 1.5, depth);
  } else {
    vec4 gSample = texture(uGlyphTex, vUv);
    alpha = gSample.a;
    if (alpha <= 0.004) discard;
    // Exact decode of 128/255 -> 0.0 + convex dome curvature matched to tight glyph bounds (1:1 native)
    vec2 sobelN = clamp((gSample.rg * 255.0 - 128.0) / 126.0, vec2(-1.0), vec2(1.0));
    vec2 innerHalf = max(uLayerSize * 0.5 - vec2(uGlyphPadPx), vec2(1.0));
    vec2 domeN = clamp(localPx / innerHalf, vec2(-1.0), vec2(1.0));
    normal = clamp(sobelN + domeN * 0.28, vec2(-1.0), vec2(1.0));
    edgeStrength = gSample.b;
    lensMag = edgeStrength;
    borderMask = smoothstep(0.35, 0.95, edgeStrength);
  }

  // Transform localPx -> rotated/stretched viewport CSS pixel coordinate
  vec2 scaledPx = vec2(localPx.x, localPx.y * uScaleY);
  float c = cos(uRotationRad);
  float s = sin(uRotationRad);
  vec2 rotPx = vec2(
    scaledPx.x * c - scaledPx.y * s,
    scaledPx.x * s + scaledPx.y * c
  );
  vec2 vpPx = uLayerCenter + rotPx;

  // Real optical lens displacement (in viewport pixels, rotated with layer)
  vec2 dispLocal = -normal * (uRefractionPx * 1.35);
  vec2 dispVp = vec2(
    dispLocal.x * c - dispLocal.y * s,
    dispLocal.x * s + dispLocal.y * c
  );

  // Sample refracted backdrop with optional 3-channel RGB prismatic dispersion
  vec4 bg;
  if (uChromaPx > 0.05) {
    float ca = clamp(uChromaPx * 0.075, 0.0, 0.45);
    vec2 dir = dispVp;
    if (length(dir) < 0.5) {
      vec2 fallbackLocal = -normal * (uChromaPx * 2.2);
      dir = vec2(
        fallbackLocal.x * c - fallbackLocal.y * s,
        fallbackLocal.x * s + fallbackLocal.y * c
      );
    }
    vec4 rS = sampleBackdrop(vpPx + dir * (1.0 + ca));
    vec4 gS = sampleBackdrop(vpPx + dir);
    vec4 bS = sampleBackdrop(vpPx + dir * (1.0 - ca));
    bg = vec4(rS.r, gS.g, bS.b, clamp((rS.a + gS.a + bS.a) * (1.0 / 3.0), 0.0, 1.0));
  } else {
    bg = sampleBackdrop(vpPx + dispVp);
  }

  // Optical transmission preserving premultiplied alpha (transparent 3D background stays transparent, never black)
  vec4 res = vec4(0.0);
  if (bg.a > 0.001) {
    vec3 rgb = clamp(bg.rgb / bg.a, 0.0, 1.0);
    rgb *= uBrightness;
    rgb = (rgb - 0.5) * uContrast + 0.5;
    float luma = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = clamp(mix(vec3(luma), rgb, uSaturation), 0.0, 1.0);
    res = vec4(rgb * bg.a, bg.a);
  }

  // Glass Tint Veil (Porter-Duff src-over: 0% = colorless clear crystal, 80% = dense milky/smoked glass)
  float diag = clamp(vUv.x * 0.5 + (1.0 - vUv.y) * 0.5, 0.0, 1.0);
  if (uFrostOpacity > 0.002) {
    float tintAmt = clamp(mix(uFrostOpacity * 1.18, uFrostOpacity * 0.82, diag), 0.0, 0.94);
    res = vec4(uTintColor * tintAmt, tintAmt) + res * (1.0 - tintAmt);
  }

  // Tactile frosted micro-grain scaled by actual glass coverage
  if (uNoiseOpacity > 0.002 && res.a > 0.002) {
    float n1 = hash12(gl_FragCoord.xy + vec2(17.3, 41.7)) - 0.5;
    float n2 = hash12(floor(gl_FragCoord.xy * 0.5) + vec2(91.1, 13.7)) - 0.5;
    float grain = n1 * 0.65 + n2 * 0.35;
    res.rgb = clamp(res.rgb + vec3(grain * uNoiseOpacity * 0.88) * res.a, 0.0, res.a);
  }

  // Foreground pigment src-over for TEXT GLYPH mode (real Color Alpha 0..100%: transparent -> color)
  if (uMode == 1 && uPigmentColor.a > 0.001) {
    float pa = clamp(uPigmentColor.a, 0.0, 1.0);
    res = vec4(uPigmentColor.rgb * pa, pa) + res * (1.0 - pa);
  }

  // Directional top-left crystal specular highlight + bottom-right internal bounce
  vec2 lightDir = normalize(vec2(-0.65, -0.76));
  float nLen = length(normal);
  vec2 unitN = nLen > 0.001 ? normal / nLen : vec2(0.0);
  float ndotl = dot(unitN, lightDir);

  if (uSpecular > 0.005) {
    float topSpec = pow(max(0.0, ndotl), 1.45) * edgeStrength * uSpecular * 0.92;
    float bottomBounce = pow(max(0.0, -ndotl), 1.7) * edgeStrength * uSpecular * 0.30;
    float sheen = smoothstep(0.62, 0.0, diag) * uSpecular * (uMode == 0 ? 0.14 : 0.08);
    float bevelGlow = lensMag * uSpecular * 0.12;
    vec3 specRgb = vec3(1.0) * (topSpec + sheen + bevelGlow) + vec3(0.86, 0.90, 1.0) * bottomBounce;
    float specA = clamp(max(specRgb.r, max(specRgb.g, specRgb.b)), 0.0, 1.0);
    res = vec4(res.rgb + specRgb, clamp(res.a + specA * (1.0 - res.a), 0.0, 1.0));
  }

  // Crisp subtle tinted border along outer bevel (src-over)
  if (uBorderOpacity > 0.005) {
    float rimFactor = uMode == 0 ? borderMask : borderMask * 0.65;
    float ba = clamp(rimFactor * uBorderOpacity, 0.0, 0.85);
    res = vec4(uTintColor * ba, ba) + res * (1.0 - ba);
  }

  float mask = clamp(alpha * uOpacity, 0.0, 1.0);
  fragColor = vec4(clamp(res.rgb, 0.0, 1.0) * mask, clamp(res.a, 0.0, 1.0) * mask);
}
`;

// --- CSS Gradient / Solid Background Rasterizer (for non-video/non-image scenes) ---

const bgCanvasCache = new Map<string, HTMLCanvasElement>();

function parseColorStops(inside: string): Array<{ color: string; pos: number | null }> {
  const parts: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of inside) {
    if (ch === "(") depth++;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    if (ch === "," && depth === 0) {
      parts.push(cur.trim());
      cur = "";
    } else {
      cur += ch;
    }
  }
  if (cur.trim()) parts.push(cur.trim());

  const stops: Array<{ color: string; pos: number | null }> = [];
  for (const p of parts) {
    const m = p.match(/^(.*?)\s+([\d.]+)%$/);
    if (m) {
      stops.push({ color: m[1].trim(), pos: Math.max(0, Math.min(1, Number(m[2]) / 100)) });
    } else if (/^(#|rgb|hsl|transparent|[a-z]+$)/i.test(p)) {
      stops.push({ color: p, pos: null });
    }
  }
  return stops;
}

function applyStops(grad: CanvasGradient, stops: Array<{ color: string; pos: number | null }>) {
  if (stops.length === 0) return;
  const n = stops.length;
  stops.forEach((s, i) => {
    const pos = s.pos !== null ? s.pos : n === 1 ? 0 : i / (n - 1);
    try {
      grad.addColorStop(pos, s.color);
    } catch {
      /* ignore invalid stop */
    }
  });
}

export function getCssBackgroundCanvas(cssBg: string): HTMLCanvasElement | null {
  if (typeof document === "undefined") return null;
  const key = cssBg || "#060608";
  const hit = bgCanvasCache.get(key);
  if (hit) return hit;

  const w = 512;
  const h = 288;
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;

  const trimmed = key.trim();
  if (trimmed.startsWith("radial-gradient(")) {
    const inner = trimmed.slice("radial-gradient(".length, trimmed.lastIndexOf(")"));
    const firstComma = inner.indexOf(",");
    const header = firstComma > 0 ? inner.slice(0, firstComma) : "";
    const stopsStr = firstComma > 0 && /at\s+/i.test(header) ? inner.slice(firstComma + 1) : inner;
    const atMatch = header.match(/at\s+([\d.]+)%\s+([\d.]+)%/i);
    const cx = atMatch ? (Number(atMatch[1]) / 100) * w : w * 0.5;
    const cy = atMatch ? (Number(atMatch[2]) / 100) * h : h * 0.5;
    const grad = ctx.createRadialGradient(cx, cy, 0, cx, cy, Math.hypot(w, h) * 0.62);
    applyStops(grad, parseColorStops(stopsStr));
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, w, h);
  } else if (trimmed.startsWith("linear-gradient(")) {
    const inner = trimmed.slice("linear-gradient(".length, trimmed.lastIndexOf(")"));
    const firstComma = inner.indexOf(",");
    const header = firstComma > 0 ? inner.slice(0, firstComma).trim() : "";
    const degMatch = header.match(/^(-?[\d.]+)deg$/i);
    const stopsStr = degMatch ? inner.slice(firstComma + 1) : inner;
    const deg = degMatch ? Number(degMatch[1]) : 180;
    const rad = ((deg - 90) * Math.PI) / 180;
    const dx = Math.cos(rad) * w * 0.5;
    const dy = Math.sin(rad) * h * 0.5;
    const grad = ctx.createLinearGradient(w * 0.5 - dx, h * 0.5 - dy, w * 0.5 + dx, h * 0.5 + dy);
    applyStops(grad, parseColorStops(stopsStr));
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, w, h);
  } else {
    ctx.fillStyle = trimmed === "transparent" ? "#08090c" : trimmed;
    ctx.fillRect(0, 0, w, h);
  }

  if (bgCanvasCache.size > 16) {
    const oldest = bgCanvasCache.keys().next().value;
    if (oldest) bgCanvasCache.delete(oldest);
  }
  bgCanvasCache.set(key, canvas);
  return canvas;
}

// --- Shared Singleton WebGL2 Engine ---

interface MipPair {
  w: number;
  h: number;
  texA: WebGLTexture;
  fboA: WebGLFramebuffer;
  texB: WebGLTexture;
  fboB: WebGLFramebuffer;
}

const GLYPH_TEX_CACHE_LIMIT = 12;
const SHARP_TEX_CACHE_LIMIT = 4;
const BLUR_TEX_CACHE_LIMIT = 4;

interface SharpEntry {
  tex: WebGLTexture;
  /** Effective frame token of the last upload (0 for static backdrops). */
  token: number;
}

interface BlurEntry {
  tex: WebGLTexture;
  fbo: WebGLFramebuffer;
  w: number;
  h: number;
}

/** Static backdrops (images / non-live canvas) ignore frameToken; video and live canvases follow it. */
function effectiveFrameToken(bg: BackdropSourceInfo): number {
  return bg.kind === "video" || bg.live ? bg.frameToken : 0;
}

class OpticalGlassWebGLEngine {
  private canvas: GlCanvas | null = null;
  private gl: WebGL2RenderingContext | null = null;
  private available = true;
  private contextLost = false;
  /**
   * 4.2: when OffscreenCanvas + bitmaprenderer exist, the shared GL canvas is an OffscreenCanvas and each
   * glass receives the frame via transferToImageBitmap (zero copy, no per-glass 2D canvas/drawImage).
   */
  private bitmapMode =
    typeof OffscreenCanvas !== "undefined" &&
    typeof ImageBitmapRenderingContext !== "undefined" &&
    typeof OffscreenCanvas.prototype.transferToImageBitmap === "function";

  private packProgram: WebGLProgram | null = null;
  private packLocs: {
    uSharp: WebGLUniformLocation | null;
    uHeight: WebGLUniformLocation | null;
    uBevelSigma: WebGLUniformLocation | null;
  } | null = null;
  private compProgram: WebGLProgram | null = null;
  private compLocs: {
    uTex: WebGLUniformLocation | null;
    uBgScale: WebGLUniformLocation | null;
    uBgOffset: WebGLUniformLocation | null;
    uForceOpaque: WebGLUniformLocation | null;
  } | null = null;
  private compTex: WebGLTexture | null = null;
  private compFbo: WebGLFramebuffer | null = null;
  private compW = 0;
  private compH = 0;
  private compKey = "";
  private downProgram: WebGLProgram | null = null;
  private gaussProgram: WebGLProgram | null = null;
  private glassProgram: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;

  private sharpTex: WebGLTexture | null = null;
  private glyphTexCache = new Map<string, WebGLTexture>();
  private mips: MipPair[] = [];
  private mipBaseW = 0;
  private mipBaseH = 0;
  private activeBlurTex: WebGLTexture | null = null;

  /** Per-backdrop-key sharp textures (small LRU) so alternating backdrops don't re-upload every glass. */
  private sharpCache = new Map<string, SharpEntry>();
  /** Small LRU of blurred results keyed by `backdropKey|frameToken|blurQuant|baseWxbaseH`. */
  private blurCache = new Map<string, BlurEntry>();

  // Cached uniform locations
  private downLocs: { uTex: WebGLUniformLocation | null; uHalfTexel: WebGLUniformLocation | null } | null = null;
  private gaussLocs: {
    uTex: WebGLUniformLocation | null;
    uAxisStep: WebGLUniformLocation | null;
    uSigma: WebGLUniformLocation | null;
  } | null = null;
  private glassLocs: Record<string, WebGLUniformLocation | null> = {};

  public isAvailable(): boolean {
    if (!this.available || this.contextLost || typeof document === "undefined") return false;
    if (!this.gl) this.init();
    return Boolean(this.gl && !this.gl.isContextLost());
  }

  private compileShader(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
    const sh = gl.createShader(type);
    if (!sh) return null;
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      console.error("[PrismView WebGL2] Shader compile error:", gl.getShaderInfoLog(sh));
      gl.deleteShader(sh);
      return null;
    }
    return sh;
  }

  private linkProgram(gl: WebGL2RenderingContext, vsSrc: string, fsSrc: string): WebGLProgram | null {
    const vs = this.compileShader(gl, gl.VERTEX_SHADER, vsSrc);
    const fs = this.compileShader(gl, gl.FRAGMENT_SHADER, fsSrc);
    if (!vs || !fs) {
      if (vs) gl.deleteShader(vs);
      if (fs) gl.deleteShader(fs);
      return null;
    }
    const prog = gl.createProgram();
    if (!prog) {
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      return null;
    }
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      console.error("[PrismView WebGL2] Program link error:", gl.getProgramInfoLog(prog));
      gl.deleteProgram(prog);
      return null;
    }
    return prog;
  }

  private createLinearTexture(gl: WebGL2RenderingContext): WebGLTexture | null {
    const tex = gl.createTexture();
    if (!tex) return null;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    return tex;
  }

  private setupGlResources(gl: WebGL2RenderingContext): boolean {
    const downProgram = this.linkProgram(gl, VERT_SRC, DOWNSAMPLE_FRAG_SRC);
    const gaussProgram = this.linkProgram(gl, VERT_SRC, GAUSS_FRAG_SRC);
    const glassProgram = this.linkProgram(gl, VERT_SRC, GLASS_FRAG_SRC);
    const packProgram = this.linkProgram(gl, VERT_SRC, GLYPH_PACK_FRAG_SRC);
    const compProgram = this.linkProgram(gl, VERT_SRC, COMP_FRAG_SRC);
    if (!downProgram || !gaussProgram || !glassProgram || !packProgram || !compProgram) {
      if (downProgram) gl.deleteProgram(downProgram);
      if (gaussProgram) gl.deleteProgram(gaussProgram);
      if (glassProgram) gl.deleteProgram(glassProgram);
      if (packProgram) gl.deleteProgram(packProgram);
      if (compProgram) gl.deleteProgram(compProgram);
      return false;
    }

    this.gl = gl;
    this.packProgram = packProgram;
    this.packLocs = {
      uSharp: gl.getUniformLocation(packProgram, "uSharp"),
      uHeight: gl.getUniformLocation(packProgram, "uHeight"),
      uBevelSigma: gl.getUniformLocation(packProgram, "uBevelSigma"),
    };
    this.compProgram = compProgram;
    this.compLocs = {
      uTex: gl.getUniformLocation(compProgram, "uTex"),
      uBgScale: gl.getUniformLocation(compProgram, "uBgScale"),
      uBgOffset: gl.getUniformLocation(compProgram, "uBgOffset"),
      uForceOpaque: gl.getUniformLocation(compProgram, "uForceOpaque"),
    };
    this.downProgram = downProgram;
    this.gaussProgram = gaussProgram;
    this.glassProgram = glassProgram;
    this.vao = gl.createVertexArray();
    // Fallback texture; real backdrops live in sharpCache and are pointed to by sharpTex per draw.
    this.sharpTex = this.createLinearTexture(gl);
    this.glyphTexCache.clear();
    this.mips = [];
    this.mipBaseW = 0;
    this.mipBaseH = 0;
    this.activeBlurTex = null;
    this.sharpCache.clear();
    this.blurCache.clear();

    this.downLocs = {
      uTex: gl.getUniformLocation(downProgram, "uTex"),
      uHalfTexel: gl.getUniformLocation(downProgram, "uHalfTexel"),
    };
    this.gaussLocs = {
      uTex: gl.getUniformLocation(gaussProgram, "uTex"),
      uAxisStep: gl.getUniformLocation(gaussProgram, "uAxisStep"),
      uSigma: gl.getUniformLocation(gaussProgram, "uSigma"),
    };

    const names = [
      "uMode",
      "uSharpTex",
      "uBlurTex",
      "uGlyphTex",
      "uViewportSize",
      "uLayerCenter",
      "uLayerSize",
      "uRotationRad",
      "uScaleY",
      "uBgScale",
      "uBgOffset",
      "uBlurMix",
      "uRefractionPx",
      "uBevelPx",
      "uRadiusPx",
      "uChromaPx",
      "uFrostOpacity",
      "uTintColor",
      "uNoiseOpacity",
      "uSpecular",
      "uBorderOpacity",
      "uBrightness",
      "uContrast",
      "uSaturation",
      "uOpacity",
      "uPigmentColor",
      "uOpaqueBackdrop",
      "uGlyphPadPx",
    ];
    this.glassLocs = {};
    for (const n of names) {
      this.glassLocs[n] = gl.getUniformLocation(glassProgram, n);
    }
    return true;
  }

  private init() {
    if (typeof document === "undefined") {
      this.available = false;
      return;
    }
    if (!this.canvas) {
      const raw: GlCanvas = this.bitmapMode ? new OffscreenCanvas(2, 2) : document.createElement("canvas");
      raw.width = 2;
      raw.height = 2;
      // Both canvas kinds share getContext/addEventListener; the union only trips TS overload resolution.
      const canvas = raw as HTMLCanvasElement;

      canvas.addEventListener("webglcontextlost", (e) => {
        e.preventDefault();
        this.contextLost = true;
        this.gl = null;
        this.mips = [];
        this.mipBaseW = 0;
        this.mipBaseH = 0;
        this.activeBlurTex = null;
        this.glyphTexCache.clear();
        this.sharpCache.clear();
        this.blurCache.clear();
        this.sharpTex = null;
        this.compTex = null;
        this.compFbo = null;
        this.compW = 0;
        this.compH = 0;
        this.compKey = "";
      });
      canvas.addEventListener("webglcontextrestored", () => {
        this.contextLost = false;
        const restored = canvas.getContext("webgl2", {
          alpha: true,
          antialias: false,
          depth: false,
          stencil: false,
          premultipliedAlpha: true,
          preserveDrawingBuffer: false,
          powerPreference: "high-performance",
        });
        if (!restored || !this.setupGlResources(restored)) {
          this.available = false;
        }
      });
      this.canvas = raw;
    }

    const gl = (this.canvas as HTMLCanvasElement).getContext("webgl2", {
      alpha: true,
      antialias: false,
      depth: false,
      stencil: false,
      premultipliedAlpha: true,
      preserveDrawingBuffer: false,
      powerPreference: "high-performance",
    });

    if (!gl || !this.setupGlResources(gl)) {
      this.available = false;
    }
  }

  private getOrUploadGlyphTexture(
    gl: WebGL2RenderingContext,
    glyphKey: string,
    glyphCanvas: HTMLCanvasElement,
    heightCanvas: HTMLCanvasElement,
    bevelSigmaPx: number
  ): WebGLTexture | null {
    const existing = this.glyphTexCache.get(glyphKey);
    if (existing) {
      // Refresh LRU order
      this.glyphTexCache.delete(glyphKey);
      this.glyphTexCache.set(glyphKey, existing);
      return existing;
    }
    const w = glyphCanvas.width;
    const h = glyphCanvas.height;
    const tex = this.createLinearTexture(gl);
    const sharpSrc = this.createLinearTexture(gl);
    const heightSrc = this.createLinearTexture(gl);
    const fbo = gl.createFramebuffer();
    if (!tex || !sharpSrc || !heightSrc || !fbo || !this.packProgram || !this.packLocs) {
      if (tex) gl.deleteTexture(tex);
      if (sharpSrc) gl.deleteTexture(sharpSrc);
      if (heightSrc) gl.deleteTexture(heightSrc);
      if (fbo) gl.deleteFramebuffer(fbo);
      return null;
    }

    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, 1);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, sharpSrc);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, glyphCanvas);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, heightSrc);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, heightCanvas);

    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);

    gl.viewport(0, 0, w, h);
    gl.disable(gl.SCISSOR_TEST);
    gl.disable(gl.BLEND);
    gl.useProgram(this.packProgram);
    gl.bindVertexArray(this.vao);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, sharpSrc);
    gl.uniform1i(this.packLocs.uSharp, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, heightSrc);
    gl.uniform1i(this.packLocs.uHeight, 1);
    gl.uniform1f(this.packLocs.uBevelSigma, Math.max(1.5, bevelSigmaPx));
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.deleteFramebuffer(fbo);
    gl.deleteTexture(sharpSrc);
    gl.deleteTexture(heightSrc);
    this.glyphTexCache.set(glyphKey, tex);
    if (this.glyphTexCache.size > GLYPH_TEX_CACHE_LIMIT) {
      const oldestKey = this.glyphTexCache.keys().next().value;
      if (oldestKey !== undefined) {
        const oldTex = this.glyphTexCache.get(oldestKey);
        if (oldTex) gl.deleteTexture(oldTex);
        this.glyphTexCache.delete(oldestKey);
      }
    }
    return tex;
  }

  private ensureMipPyramid(gl: WebGL2RenderingContext, srcW: number, srcH: number) {
    // Cap mip-0 width at 1080px (FHD+ budget) so 4K wallpapers stay ultra-fast & crisp.
    // 5 levels (1x, 1/2, 1/4, 1/8, 1/16) guarantee smooth monotonic blur all the way to 80px.
    const scale = srcW > 1080 ? 1080 / srcW : 1;
    const baseW = Math.max(64, Math.round(srcW * scale));
    const baseH = Math.max(64, Math.round(srcH * scale));
    if (baseW === this.mipBaseW && baseH === this.mipBaseH && this.mips.length === 5) return;

    // Clean up previous pyramid textures/FBOs
    for (const m of this.mips) {
      gl.deleteTexture(m.texA);
      gl.deleteTexture(m.texB);
      gl.deleteFramebuffer(m.fboA);
      gl.deleteFramebuffer(m.fboB);
    }
    this.mips = [];
    this.mipBaseW = baseW;
    this.mipBaseH = baseH;

    let w = baseW;
    let h = baseH;
    for (let lvl = 0; lvl < 5; lvl++) {
      const texA = this.createLinearTexture(gl)!;
      gl.bindTexture(gl.TEXTURE_2D, texA);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      const fboA = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.FRAMEBUFFER, fboA);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texA, 0);

      const texB = this.createLinearTexture(gl)!;
      gl.bindTexture(gl.TEXTURE_2D, texB);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      const fboB = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.FRAMEBUFFER, fboB);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texB, 0);

      this.mips.push({ w, h, texA, fboA, texB, fboB });
      w = Math.max(16, Math.round(w * 0.5));
      h = Math.max(16, Math.round(h * 0.5));
    }

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.activeBlurTex = null;
  }

  private disposeBlurEntry(gl: WebGL2RenderingContext, e: BlurEntry) {
    gl.deleteFramebuffer(e.fbo);
    gl.deleteTexture(e.tex);
  }

  private uploadSingleBackdrop(gl: WebGL2RenderingContext, bg: BackdropSourceInfo): WebGLTexture | null {
    const token = effectiveFrameToken(bg);
    const existing = this.sharpCache.get(bg.key);

    if (existing) {
      this.sharpCache.delete(bg.key);
      this.sharpCache.set(bg.key, existing);
      if (existing.token === token) return existing.tex;
    }

    if (bg.kind === "video") {
      const v = bg.element as HTMLVideoElement;
      if (v.readyState < 2 || v.videoWidth <= 0 || v.videoHeight <= 0) return null;
    } else if (bg.kind === "image") {
      const img = bg.element as HTMLImageElement;
      if (!img.complete || img.naturalWidth <= 0 || img.naturalHeight <= 0) return null;
    }

    let entry = existing;
    let created = false;
    if (!entry) {
      const tex = this.createLinearTexture(gl);
      if (!tex) return null;
      entry = { tex, token: -1 };
      created = true;
    }

    try {
      gl.bindTexture(gl.TEXTURE_2D, entry.tex);
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, 1);
      const premul = bg.kind === "canvas";
      if (premul) gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, 1);
      try {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, bg.element);
      } finally {
        if (premul) gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, 0);
      }
    } catch {
      if (created) gl.deleteTexture(entry.tex);
      return null;
    }

    entry.token = token;
    if (created) {
      this.sharpCache.set(bg.key, entry);
      if (this.sharpCache.size > SHARP_TEX_CACHE_LIMIT) {
        const oldestKey = this.sharpCache.keys().next().value;
        if (oldestKey !== undefined) {
          const old = this.sharpCache.get(oldestKey);
          if (old) gl.deleteTexture(old.tex);
          this.sharpCache.delete(oldestKey);
        }
      }
    }
    return entry.tex;
  }

  private ensureCompositeTarget(gl: WebGL2RenderingContext, w: number, h: number): boolean {
    if (this.compTex && this.compFbo && this.compW === w && this.compH === h) return true;
    if (this.compFbo) gl.deleteFramebuffer(this.compFbo);
    if (this.compTex) gl.deleteTexture(this.compTex);
    const tex = this.createLinearTexture(gl);
    const fbo = gl.createFramebuffer();
    if (!tex || !fbo) {
      if (tex) gl.deleteTexture(tex);
      if (fbo) gl.deleteFramebuffer(fbo);
      this.compTex = null;
      this.compFbo = null;
      this.compW = 0;
      this.compH = 0;
      return false;
    }
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.compTex = tex;
    this.compFbo = fbo;
    this.compW = w;
    this.compH = h;
    this.compKey = "";
    return true;
  }

  /**
   * Prepares the active sharp backdrop texture. When a transparent 3D canvas has an `underlay`
   * (gradient, image, or video below it), composites both on the GPU into an opaque viewport-aligned
   * texture so blur & refraction interact cleanly with the full background without sharp bleed-through.
   */
  private prepareBackdrop(
    gl: WebGL2RenderingContext,
    bg: BackdropSourceInfo,
    viewportW: number,
    viewportH: number
  ): BackdropSourceInfo | null {
    if (!bg.underlay || !this.compProgram || !this.compLocs) {
      const tex = this.uploadSingleBackdrop(gl, bg);
      if (!tex) return null;
      this.sharpTex = tex;
      return bg;
    }

    const underlay = bg.underlay;
    const underlayTex = this.uploadSingleBackdrop(gl, underlay);
    const fgTex = this.uploadSingleBackdrop(gl, bg);
    if (!fgTex && !underlayTex) return null;
    if (!fgTex && underlayTex) {
      this.sharpTex = underlayTex;
      return underlay;
    }
    if (fgTex && !underlayTex) {
      this.sharpTex = fgTex;
      return bg;
    }

    const scale = viewportW > 1080 ? 1080 / viewportW : 1;
    const cw = Math.max(64, Math.round(viewportW * scale));
    const ch = Math.max(64, Math.round(viewportH * scale));
    if (!this.ensureCompositeTarget(gl, cw, ch)) {
      this.sharpTex = fgTex;
      return bg;
    }

    const fgTok = effectiveFrameToken(bg);
    const ulTok = effectiveFrameToken(underlay);
    const compKey = `${underlay.key}|${ulTok}|${Math.round(underlay.drawW)}x${Math.round(underlay.drawH)}|${bg.key}|${fgTok}|${Math.round(bg.drawW)}x${Math.round(bg.drawH)}|${cw}x${ch}`;
    if (this.compKey !== compKey) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.compFbo);
      gl.viewport(0, 0, cw, ch);
      gl.disable(gl.SCISSOR_TEST);
      gl.useProgram(this.compProgram);
      gl.bindVertexArray(this.vao);

      // Pass 1: opaque underlay mapped into viewport space
      gl.disable(gl.BLEND);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, underlayTex);
      gl.uniform1i(this.compLocs.uTex, 0);
      gl.uniform2f(
        this.compLocs.uBgScale,
        viewportW / Math.max(underlay.drawW, 1),
        viewportH / Math.max(underlay.drawH, 1)
      );
      gl.uniform2f(
        this.compLocs.uBgOffset,
        -underlay.drawLeft / Math.max(underlay.drawW, 1),
        -underlay.drawTop / Math.max(underlay.drawH, 1)
      );
      gl.uniform1f(this.compLocs.uForceOpaque, 1.0);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

      // Pass 2: premultiplied transparent 3D canvas composited src-over the underlay
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      gl.bindTexture(gl.TEXTURE_2D, fgTex);
      gl.uniform2f(
        this.compLocs.uBgScale,
        viewportW / Math.max(bg.drawW, 1),
        viewportH / Math.max(bg.drawH, 1)
      );
      gl.uniform2f(
        this.compLocs.uBgOffset,
        -bg.drawLeft / Math.max(bg.drawW, 1),
        -bg.drawTop / Math.max(bg.drawH, 1)
      );
      gl.uniform1f(this.compLocs.uForceOpaque, 0.0);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

      gl.disable(gl.BLEND);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      this.compKey = compKey;
    }

    this.sharpTex = this.compTex;
    return {
      key: `comp|${underlay.key}|${bg.key}`,
      kind: "image",
      live: Boolean(bg.live || underlay.live),
      element: bg.element,
      intrinsicW: cw,
      intrinsicH: ch,
      drawLeft: 0,
      drawTop: 0,
      drawW: Math.max(1, viewportW),
      drawH: Math.max(1, viewportH),
      frameToken: fgTok * 31 + ulTok,
    };
  }

  private updateBlurPyramid(gl: WebGL2RenderingContext, bg: BackdropSourceInfo, blurPx: number) {
    // Do NOT touch activeBlurTex / blurCache when blurPx <= 0.1 so cached blurred layers stay valid
    if (blurPx <= 0.1) return;
    this.ensureMipPyramid(gl, bg.intrinsicW, bg.intrinsicH);

    // Quantize to 0.25px so small Studio viewports stay smooth and match native 1:1
    const blurQuant = Math.round(Math.max(0.25, Math.min(80, blurPx)) * 4) / 4;
    const cacheKey = `${bg.key}|${effectiveFrameToken(bg)}|${blurQuant}|${this.mipBaseW}x${this.mipBaseH}`;
    const cached = this.blurCache.get(cacheKey);
    if (cached) {
      // Refresh LRU order
      this.blurCache.delete(cacheKey);
      this.blurCache.set(cacheKey, cached);
      this.activeBlurTex = cached.tex;
      return;
    }

    // Convert CSS blurPx -> Gaussian sigma in mip-0 texels
    const mip0 = this.mips[0];
    const texelsPerCssPx = mip0.w / Math.max(bg.drawW, 1);
    const sigma0 = Math.max(0.4, blurQuant * 0.52 * texelsPerCssPx);

    // Pick lowest mip level where sigma stays <= 3.3 texels (15-tap kernel covers >98.5% of bell curve with 1.0 texel step!)
    let targetLevel = 0;
    let sigmaInMip = sigma0;
    while (targetLevel < this.mips.length - 1 && sigmaInMip > 3.2) {
      targetLevel += 1;
      sigmaInMip *= 0.5;
    }
    sigmaInMip = Math.max(0.45, Math.min(4.2, sigmaInMip));

    gl.bindVertexArray(this.vao);
    gl.disable(gl.BLEND);

    // Step 1: Anti-aliased progressive <=2x downsample chain from sharpTex -> mips[targetLevel].fboB
    let curTex: WebGLTexture = this.sharpTex!;
    let curW = bg.intrinsicW;
    let curH = bg.intrinsicH;

    const needsMip0Down = bg.intrinsicW > mip0.w * 1.15 || bg.intrinsicH > mip0.h * 1.15;
    if (needsMip0Down || targetLevel > 0) {
      gl.useProgram(this.downProgram);
      const startLvl = needsMip0Down ? 0 : 1;
      for (let lvl = startLvl; lvl <= targetLevel; lvl++) {
        const dst = this.mips[lvl];
        gl.bindFramebuffer(gl.FRAMEBUFFER, dst.fboB);
        gl.viewport(0, 0, dst.w, dst.h);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, curTex);
        gl.uniform1i(this.downLocs!.uTex, 0);
        gl.uniform2f(this.downLocs!.uHalfTexel, 0.5 / Math.max(curW, 1), 0.5 / Math.max(curH, 1));
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        curTex = dst.texB;
        curW = dst.w;
        curH = dst.h;
      }
    }

    // Step 2: Exact dense 15-tap separable Gaussian H + V at EXACT 1.0-texel spacing (ZERO ghosting!)
    const mip = this.mips[targetLevel];
    gl.useProgram(this.gaussProgram);
    gl.viewport(0, 0, mip.w, mip.h);
    gl.uniform1f(this.gaussLocs!.uSigma, sigmaInMip);

    // Horizontal pass -> mip.fboA
    gl.bindFramebuffer(gl.FRAMEBUFFER, mip.fboA);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, curTex);
    gl.uniform1i(this.gaussLocs!.uTex, 0);
    gl.uniform2f(this.gaussLocs!.uAxisStep, 1.0 / mip.w, 0.0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    // Vertical pass -> dedicated cache entry (survives other glasses using a different blur / backdrop)
    const entry = this.acquireBlurEntry(gl, mip.w, mip.h);
    if (!entry) {
      // Out of GPU resources: fall back to the shared pyramid texture (uncached for this frame)
      gl.bindFramebuffer(gl.FRAMEBUFFER, mip.fboB);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, mip.texA);
      gl.uniform1i(this.gaussLocs!.uTex, 0);
      gl.uniform2f(this.gaussLocs!.uAxisStep, 0.0, 1.0 / mip.h);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      this.activeBlurTex = mip.texB;
      return;
    }

    gl.bindFramebuffer(gl.FRAMEBUFFER, entry.fbo);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, mip.texA);
    gl.uniform1i(this.gaussLocs!.uTex, 0);
    gl.uniform2f(this.gaussLocs!.uAxisStep, 0.0, 1.0 / mip.h);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.blurCache.set(cacheKey, entry);
    this.activeBlurTex = entry.tex;
  }

  /** Returns a free blur target of the given size, recycling the LRU entry when the cache is full. */
  private acquireBlurEntry(gl: WebGL2RenderingContext, w: number, h: number): BlurEntry | null {
    if (this.blurCache.size >= BLUR_TEX_CACHE_LIMIT) {
      const oldestKey = this.blurCache.keys().next().value;
      if (oldestKey !== undefined) {
        const old = this.blurCache.get(oldestKey)!;
        this.blurCache.delete(oldestKey);
        if (old.w === w && old.h === h) return old;
        this.disposeBlurEntry(gl, old);
      }
    }
    const tex = this.createLinearTexture(gl);
    if (!tex) return null;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    const fbo = gl.createFramebuffer();
    if (!fbo) {
      gl.deleteTexture(tex);
      return null;
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    return { tex, fbo, w, h };
  }

  public renderToCanvas(targetCanvas: HTMLCanvasElement, params: OpticalGlassRenderParams): boolean {
    if (!this.isAvailable()) return false;
    const gl = this.gl!;
    const srcCanvas = this.canvas!;

    const cssW = Math.max(2, Math.round(params.width));
    const cssH = Math.max(2, Math.round(params.height));
    const dpr = Math.min(1.5, window.devicePixelRatio || 1);
    const maxDim = 1080;
    const scaleDown = Math.max(cssW * dpr, cssH * dpr) > maxDim
      ? maxDim / Math.max(cssW * dpr, cssH * dpr)
      : 1;
    const rw = Math.max(2, Math.round(cssW * dpr * scaleDown));
    const rh = Math.max(2, Math.round(cssH * dpr * scaleDown));

    const activeBg = this.prepareBackdrop(gl, params.backdrop, params.viewportW, params.viewportH);
    if (!activeBg) return false;
    this.updateBlurPyramid(gl, activeBg, params.blurPx);

    let activeGlyphTex: WebGLTexture | null = this.sharpTex;
    if (params.mode === "glyph") {
      if (!params.glyphCanvas || !params.glyphHeightCanvas || !params.glyphKey) return false;
      activeGlyphTex = this.getOrUploadGlyphTexture(
        gl,
        params.glyphKey,
        params.glyphCanvas,
        params.glyphHeightCanvas,
        params.glyphHeightSigmaPx ?? 6
      );
      if (!activeGlyphTex) return false;
    }

    // Grow shared backing WebGL canvas monotonically (high-water mark <= 1080px) to avoid per-draw reallocation
    // (bitmap mode: exact size, because transferToImageBitmap hands over the whole canvas)
    const nextCanvasW = this.bitmapMode ? rw : Math.min(maxDim, Math.max(srcCanvas.width, rw));
    const nextCanvasH = this.bitmapMode ? rh : Math.min(maxDim, Math.max(srcCanvas.height, rh));
    if (srcCanvas.width !== nextCanvasW || srcCanvas.height !== nextCanvasH) {
      srcCanvas.width = nextCanvasW;
      srcCanvas.height = nextCanvasH;
    }

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, rw, rh);
    gl.enable(gl.SCISSOR_TEST);
    gl.scissor(0, 0, rw, rh);
    gl.disable(gl.BLEND);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);

    gl.useProgram(this.glassProgram);
    gl.bindVertexArray(this.vao);

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.sharpTex);
    gl.uniform1i(this.glassLocs.uSharpTex, 0);

    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, params.blurPx > 0.1 && this.activeBlurTex ? this.activeBlurTex : this.sharpTex);
    gl.uniform1i(this.glassLocs.uBlurTex, 1);

    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, activeGlyphTex);
    gl.uniform1i(this.glassLocs.uGlyphTex, 2);

    const bg = activeBg;
    const bgScaleX = params.viewportW / Math.max(bg.drawW, 1);
    const bgScaleY = params.viewportH / Math.max(bg.drawH, 1);
    const bgOffsetX = -bg.drawLeft / Math.max(bg.drawW, 1);
    const bgOffsetY = -bg.drawTop / Math.max(bg.drawH, 1);

    // Smooth micro-blur fade from 0px -> 1.5px in logical px (1:1 parity with native blur_mix)
    const mixBasis = params.rawBlurPx ?? params.blurPx;
    const blurMix = mixBasis <= 0.1 ? 0 : Math.min(1, mixBasis / 1.5);

    gl.uniform1i(this.glassLocs.uMode, params.mode === "glyph" ? 1 : 0);
    gl.uniform2f(this.glassLocs.uViewportSize, params.viewportW, params.viewportH);
    gl.uniform2f(this.glassLocs.uLayerCenter, params.centerX, params.centerY);
    gl.uniform2f(this.glassLocs.uLayerSize, params.width, params.height);
    gl.uniform1f(this.glassLocs.uRotationRad, (params.rotationDeg * Math.PI) / 180);
    gl.uniform1f(this.glassLocs.uScaleY, params.scaleY);

    gl.uniform2f(this.glassLocs.uBgScale, bgScaleX, bgScaleY);
    gl.uniform2f(this.glassLocs.uBgOffset, bgOffsetX, bgOffsetY);

    gl.uniform1f(this.glassLocs.uBlurMix, blurMix);
    gl.uniform1f(this.glassLocs.uRefractionPx, params.refractionScale);
    gl.uniform1f(this.glassLocs.uBevelPx, params.bevelPx);
    gl.uniform1f(this.glassLocs.uRadiusPx, params.radiusPx);
    gl.uniform1f(this.glassLocs.uChromaPx, params.chromaticAberration);
    gl.uniform1f(this.glassLocs.uFrostOpacity, params.frostOpacity);
    gl.uniform3f(
      this.glassLocs.uTintColor,
      params.tintRgb[0] / 255,
      params.tintRgb[1] / 255,
      params.tintRgb[2] / 255
    );
    gl.uniform1f(this.glassLocs.uNoiseOpacity, params.noiseOpacity);
    gl.uniform1f(this.glassLocs.uSpecular, params.specularIntensity);
    gl.uniform1f(this.glassLocs.uBorderOpacity, params.borderOpacity);
    gl.uniform1f(this.glassLocs.uBrightness, params.brightness);
    gl.uniform1f(this.glassLocs.uContrast, params.contrast);
    gl.uniform1f(this.glassLocs.uSaturation, params.saturation);
    gl.uniform1f(this.glassLocs.uOpacity, params.opacity);

    const pig = params.pigmentRgba ?? [1, 1, 1, 0];
    gl.uniform4f(this.glassLocs.uPigmentColor, pig[0] / 255, pig[1] / 255, pig[2] / 255, pig[3]);
    gl.uniform1f(this.glassLocs.uOpaqueBackdrop, activeBg.kind === "canvas" ? 0.0 : 1.0);
    gl.uniform1f(this.glassLocs.uGlyphPadPx, params.glyphPadPx ?? 0);

    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.disable(gl.SCISSOR_TEST);

    if (this.bitmapMode) {
      // Zero-copy hand-off: the target canvas adopts the GPU frame (and its size) directly.
      const bctx = targetCanvas.getContext("bitmaprenderer");
      if (!bctx) return false;
      bctx.transferFromImageBitmap((srcCanvas as OffscreenCanvas).transferToImageBitmap());
      return true;
    }

    if (targetCanvas.width !== rw || targetCanvas.height !== rh) {
      targetCanvas.width = rw;
      targetCanvas.height = rh;
    }
    const ctx2d = targetCanvas.getContext("2d");
    if (!ctx2d) return false;
    ctx2d.clearRect(0, 0, rw, rh);
    // WebGL viewport (0, 0, rw, rh) occupies bottom-left of srcCanvas -> top-down sy = srcCanvas.height - rh
    ctx2d.drawImage(srcCanvas as HTMLCanvasElement, 0, srcCanvas.height - rh, rw, rh, 0, 0, rw, rh);
    return true;
  }
}

export const opticalGlassEngine = new OpticalGlassWebGLEngine();
