import { useEffect, useSyncExternalStore } from "react";
import { invokeCmd, isTauriRuntime } from "../types/prism";

/**
 * Quando mostrar a intro: no primeiro uso depois de ligar o PC / entrar na sessão do Windows.
 * Reabrir o app na mesma sessão NÃO repete. O marcador vem do Rust (`get_session_marker`: hora de
 * logon da sessão); fora do Tauri (dev/web) vale a sessão da aba.
 */
const SEEN_KEY = "prismview.introSession.v1";

export type IntroDecision = "pending" | "play" | "skip";

async function currentMarker(): Promise<string> {
  if (isTauriRuntime()) {
    try {
      return await invokeCmd<string>("get_session_marker");
    } catch {
      /* cai no marcador da aba */
    }
  }
  try {
    let id = sessionStorage.getItem("prismview.tabSession");
    if (!id) {
      id = String(Date.now());
      sessionStorage.setItem("prismview.tabSession", id);
    }
    return `tab:${id}`;
  } catch {
    return "tab:unknown";
  }
}

export function readSeenMarker(): string | null {
  try {
    return localStorage.getItem(SEEN_KEY);
  } catch {
    return null;
  }
}
export function writeSeenMarker(marker: string): void {
  try {
    localStorage.setItem(SEEN_KEY, marker);
  } catch {
    /* sem storage: aparece de novo na próxima, sem dano */
  }
}

/** Decide uma vez por abertura; espera a janela ficar visível (Studio iniciado minimizado no login não "gasta" a intro). */
export function useIntroDecision(opts: { disabled: boolean }): { decision: IntroDecision; marker: string } {
  const { disabled } = opts;
  const store = useSyncExternalStore(
    (cb) => decisionStore.subscribe(cb),
    () => decisionStore.get(),
    () => decisionStore.get()
  );
  useEffect(() => {
    if (decisionStore.started) return;
    decisionStore.started = true;
    const params = new URLSearchParams(window.location.search);
    const force = params.get("intro");
    void currentMarker().then((marker) => {
      const wants = force === "1" || (force !== "0" && !disabled && readSeenMarker() !== marker);
      if (!wants) {
        if (force !== "0" && disabled) writeSeenMarker(marker); // movimento reduzido/energia: conta como vista
        decisionStore.set({ decision: "skip", marker });
        return;
      }
      const go = () => decisionStore.set({ decision: "play", marker });
      if (document.visibilityState === "visible") go();
      else {
        const onVis = () => {
          if (document.visibilityState === "visible") {
            document.removeEventListener("visibilitychange", onVis);
            go();
          }
        };
        document.addEventListener("visibilitychange", onVis);
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return store;
}

const decisionStore = {
  value: { decision: "pending" as IntroDecision, marker: "" },
  started: false,
  listeners: new Set<() => void>(),
  get() {
    return this.value;
  },
  set(v: { decision: IntroDecision; marker: string }) {
    this.value = v;
    this.listeners.forEach((l) => l());
  },
  subscribe(cb: () => void) {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  },
};

/** Enquanto a intro roda (ou a decisão está pendente) o Hub não monta miniaturas/carrossel: poupa a thread principal. */
const holdStore = {
  value: true,
  listeners: new Set<() => void>(),
  set(v: boolean) {
    if (this.value === v) return;
    this.value = v;
    this.listeners.forEach((l) => l());
  },
};
export const setIntroHold = (v: boolean): void => holdStore.set(v);
export function useIntroHold(): boolean {
  return useSyncExternalStore(
    (cb) => {
      holdStore.listeners.add(cb);
      return () => {
        holdStore.listeners.delete(cb);
      };
    },
    () => holdStore.value,
    () => holdStore.value
  );
}
