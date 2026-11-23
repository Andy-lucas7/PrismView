import { createContext } from "react";

export interface HubCardCtx {
  /** Distância ao card central (0 = centro, 1 = laterais...). `undefined` fora do carrossel (grid). */
  rank?: number;
  /** `false` quando o carrossel desligou vidro/blur por peso: nenhuma miniatura usa WebGL de vidro. */
  effects: boolean;
}

export const HubCardContext = createContext<HubCardCtx>({ effects: true });
