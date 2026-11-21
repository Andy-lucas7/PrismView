import { useEffect, useMemo, useRef, useState } from "react";
import type { LiveTelemetry } from "../types/prism";

/**
 * Energy governor (Plano Mestre §3.5).
 *
 * The preference is app-global and lives in localStorage, which every PrismView
 * window (Studio, wallpaper per monitor, widgets) shares because they run on the
 * same origin. Windows pick changes up through the `storage` event and, as a
 * fallback, re-read the key on every telemetry tick (~1.5 s) — no Rust changes.
 */
export type PowerPreference = "AUTO" | "NORMAL" | "ECO" | "ULTRA" | "FROZEN";
export type PowerState = "NORMAL" | "ECO" | "ULTRA" | "FROZEN";

export interface PowerCaps {
  state: PowerState;
  /** Hard FPS ceiling on top of the layer's own slider (null = no extra cap) */
  fpsCap: number | null;
  /** Max devicePixelRatio fed to WebGL (before renderScale) */
  dprCap: number;
  /** Max renderScale (0.5..1) */
  renderScaleCap: number;
  /** Object auto-rotation / procedural sculpture motion */
  allowAutoRotate: boolean;
  /** Audio / keyboard / media / CPU reactive bindings */
  allowSignalBindings: boolean;
  /** Cursor look-at, hover & click raycasts */
  allowCursorBindings: boolean;
  /** Mouse-driven camera orbit parallax */
  allowCursorOrbit: boolean;
  /** Draw a single frame per change and keep the GPU idle otherwise */
  frozen: boolean;
  /** Real-clock day/night refresh interval */
  dayNightIntervalMs: number;
}

const POWER_KEY = "prismview.powerPreference.v1";
const PREFS: PowerPreference[] = ["AUTO", "NORMAL", "ECO", "ULTRA", "FROZEN"];

export const POWER_PREFERENCE_LABELS: Record<PowerPreference, string> = {
  AUTO: "Automático",
  NORMAL: "Normal",
  ECO: "Econômico",
  ULTRA: "Ultra economia",
  FROZEN: "Congelado",
};

export const POWER_STATE_HINTS: Record<PowerState, string> = {
  NORMAL: "Perfil escolhido em cada wallpaper",
  ECO: "Até 30 FPS · resolução 1× · sem órbita extra",
  ULTRA: "Até 15 FPS · sem reatividade nem giro automático",
  FROZEN: "Um quadro só · GPU em repouso",
};

const CAPS: Record<PowerState, PowerCaps> = {
  NORMAL: {
    state: "NORMAL",
    fpsCap: null,
    dprCap: 1.5,
    renderScaleCap: 1,
    allowAutoRotate: true,
    allowSignalBindings: true,
    allowCursorBindings: true,
    allowCursorOrbit: true,
    frozen: false,
    dayNightIntervalMs: 30_000,
  },
  ECO: {
    state: "ECO",
    fpsCap: 30,
    dprCap: 1,
    renderScaleCap: 0.85,
    allowAutoRotate: true,
    allowSignalBindings: true,
    allowCursorBindings: true,
    allowCursorOrbit: true,
    frozen: false,
    dayNightIntervalMs: 60_000,
  },
  ULTRA: {
    state: "ULTRA",
    fpsCap: 15,
    dprCap: 0.75,
    renderScaleCap: 0.7,
    allowAutoRotate: false,
    allowSignalBindings: false,
    allowCursorBindings: false,
    allowCursorOrbit: false,
    frozen: false,
    dayNightIntervalMs: 300_000,
  },
  FROZEN: {
    state: "FROZEN",
    fpsCap: 5,
    dprCap: 1,
    renderScaleCap: 1,
    allowAutoRotate: false,
    allowSignalBindings: false,
    allowCursorBindings: false,
    allowCursorOrbit: false,
    frozen: true,
    dayNightIntervalMs: 600_000,
  },
};

export function powerCapsFor(state: PowerState | undefined | null): PowerCaps {
  return CAPS[state ?? "NORMAL"] ?? CAPS.NORMAL;
}

function readPreference(): PowerPreference {
  try {
    const raw = window.localStorage.getItem(POWER_KEY);
    return PREFS.includes(raw as PowerPreference) ? (raw as PowerPreference) : "AUTO";
  } catch {
    return "AUTO";
  }
}

export function writePowerPreference(pref: PowerPreference): void {
  try {
    window.localStorage.setItem(POWER_KEY, pref);
  } catch {
    /* storage blocked: the preference still applies to this window */
  }
  window.dispatchEvent(new CustomEvent("prismview:power-pref", { detail: pref }));
}

/** Battery thresholds with hysteresis so the state never flickers around a boundary */
const ULTRA_ENTER_PCT = 20;
const ULTRA_EXIT_PCT = 25;

export function resolvePowerState(
  pref: PowerPreference,
  telemetry: Pick<LiveTelemetry, "battery_percent" | "battery_charging"> | null | undefined,
  prev: PowerState
): PowerState {
  if (pref !== "AUTO") return pref;
  if (!telemetry || telemetry.battery_charging) return "NORMAL";
  const pct = Number.isFinite(telemetry.battery_percent) ? telemetry.battery_percent : 100;
  if (pct <= ULTRA_ENTER_PCT) return "ULTRA";
  if (prev === "ULTRA" && pct < ULTRA_EXIT_PCT) return "ULTRA";
  return "ECO";
}

export function usePowerPreference(): [PowerPreference, (p: PowerPreference) => void] {
  const [pref, setPref] = useState<PowerPreference>(() => readPreference());
  useEffect(() => {
    const sync = () => setPref(readPreference());
    const onStorage = (e: StorageEvent) => {
      if (e.key === null || e.key === POWER_KEY) sync();
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener("prismview:power-pref", sync);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("prismview:power-pref", sync);
    };
  }, []);
  return [
    pref,
    (p: PowerPreference) => {
      writePowerPreference(p);
      setPref(p);
    },
  ];
}

/**
 * Resolves the live energy state for one window. Re-reads the preference on every
 * telemetry tick so wallpaper windows follow the Hub even if `storage` events are
 * not delivered across WebView instances.
 */
export function usePowerState(telemetry: LiveTelemetry | null | undefined): {
  preference: PowerPreference;
  setPreference: (p: PowerPreference) => void;
  state: PowerState;
  caps: PowerCaps;
} {
  const [preference, setPreference] = usePowerPreference();
  const [polledPref, setPolledPref] = useState<PowerPreference>(preference);
  const prevStateRef = useRef<PowerState>("NORMAL");

  useEffect(() => {
    setPolledPref(readPreference());
  }, [telemetry]);

  useEffect(() => {
    setPolledPref(preference);
  }, [preference]);

  const effectivePref = polledPref;
  const state = useMemo(
    () => resolvePowerState(effectivePref, telemetry, prevStateRef.current),
    [effectivePref, telemetry?.battery_charging, telemetry?.battery_percent]
  );
  prevStateRef.current = state;
  const caps = useMemo(() => powerCapsFor(state), [state]);
  return { preference: effectivePref, setPreference, state, caps };
}
