/**
 * Tokens de movimento do PrismView (fonte única de durações/easings/springs).
 * Sem imports: pode ser usado por qualquer módulo. Valores em ms (DURATION) e
 * convertidos para segundos só no ponto de uso do framer-motion (`sec`).
 */
export const DURATION = {
  instant: 90,
  fast: 160,
  base: 280,
  slow: 420,
  hero: 650,
  intro: 5200,
} as const;

export type DurationKey = keyof typeof DURATION;

export type Bezier = readonly [number, number, number, number];

export const EASE = {
  emphasized: [0.2, 0, 0, 1],
  decelerate: [0.05, 0.7, 0.1, 1],
  accelerate: [0.3, 0, 0.8, 0.15],
  /** Curva histórica dos botões (`cubic-bezier(0.22,1,0.36,1)`). */
  press: [0.22, 1, 0.36, 1],
} as const satisfies Record<string, Bezier>;

export type EaseKey = keyof typeof EASE;

export const SPRING = {
  tactile: { type: "spring" as const, stiffness: 400, damping: 32, mass: 0.9 },
  carousel: { type: "spring" as const, stiffness: 260, damping: 30, mass: 1 },
  expand: { type: "spring" as const, stiffness: 190, damping: 26, mass: 1 },
} as const;

export const BLUR = { enter: 12, exit: 8 } as const;
export const SCALE = { enter: 0.96, exit: 0.98, carouselSide: 0.84 } as const;

export const GESTURE = {
  /** Fração da altura da tela para confirmar um gesto vertical. */
  commitDistance: 0.22,
  /** px/s para confirmar por velocidade. */
  commitVelocity: 900,
  /** Constante `c` do rubber-band (fração da altura). */
  rubberBand: 0.35,
} as const;

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
