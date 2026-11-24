/**
 * Tokens de movimento do PrismView. Fonte única: docs/motion.tokens.json (compartilhada com o
 * Prismlock) → `npm run gen:motion` → motionTokens.gen.ts. Valores em ms (DURATION); `sec` converte
 * para o framer-motion.
 */
import { TOKENS } from "./motionTokens.gen";

export const DURATION = TOKENS.duration;

export type DurationKey = keyof typeof DURATION;

export type Bezier = readonly [number, number, number, number];

export const EASE = TOKENS.easing satisfies Record<string, Bezier>;

export type EaseKey = keyof typeof EASE;

export const SPRING = {
  tactile: { type: "spring" as const, ...TOKENS.spring.tactile },
  carousel: { type: "spring" as const, ...TOKENS.spring.carousel },
  expand: { type: "spring" as const, ...TOKENS.spring.expand },
} as const;

export const BLUR = TOKENS.blur;
export const SCALE = TOKENS.scale;
export const GESTURE = TOKENS.gesture;
export const COLOR = TOKENS.color;

export const sec = (ms: number): number => ms / 1000;

/** `transition` do framer-motion a partir dos tokens. */
export function tween(duration: DurationKey, ease: EaseKey = "emphasized") {
  return { duration: sec(DURATION[duration]), ease: [...EASE[ease]] as [number, number, number, number] };
}

const cssBezier = (b: Bezier): string => `cubic-bezier(${b.join(",")})`;

/** Publica os tokens como CSS custom properties (`--motion-*`). Chamar 1× na entrada do app. */
export function applyMotionCssVars(root: HTMLElement = document.documentElement): void {
  const s = root.style;
  for (const k of Object.keys(DURATION) as DurationKey[]) s.setProperty(`--motion-${k}`, `${DURATION[k]}ms`);
  for (const k of Object.keys(EASE) as EaseKey[]) s.setProperty(`--ease-${k}`, cssBezier(EASE[k]));
  s.setProperty("--blur-enter", `${BLUR.enter}px`);
  s.setProperty("--blur-exit", `${BLUR.exit}px`);
}
