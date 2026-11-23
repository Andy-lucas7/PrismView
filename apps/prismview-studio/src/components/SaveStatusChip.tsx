import React from "react";
import { AlertTriangle, Check, Loader2, MonitorX } from "lucide-react";

/** Estado do salvamento do projeto aberto (App → StudioEditor). */
export type SaveState = "idle" | "saving" | "saved" | "error";

export interface SaveStatus {
  state: SaveState;
  /** Motivo da última falha ao salvar (texto amigável). */
  error?: string;
  /** O desktop não recebeu a última versão (sync automático falhou). */
  desktopStale: boolean;
}

/**
 * Chip ao lado do título: "Salvando…", "Salvo", "Falha ao salvar · Tentar de novo" e
 * "Desktop desatualizado". Clicar em falha força o salvamento (mesmo que Ctrl+S).
 */
export const SaveStatusChip: React.FC<{ status: SaveStatus; onSaveNow: () => void }> = ({
  status,
  onSaveNow,
}) => {
  const base = "flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] whitespace-nowrap";
  return (
    <div className="flex items-center gap-1.5" role="status" aria-live="polite">
      {status.state === "saving" && (
        <span className={`${base} text-[#A1A1AA]`}>
          <Loader2 className="w-3 h-3 animate-spin" aria-hidden /> Salvando…
        </span>
      )}
      {status.state === "saved" && (
        <span className={`${base} text-[#A1A1AA]`}>
          <Check className="w-3 h-3" aria-hidden /> Salvo
        </span>
      )}
      {status.state === "error" && (
        <button
          type="button"
          onClick={onSaveNow}
          title={status.error ? `Falha ao salvar: ${status.error}` : "Falha ao salvar"}
          className={`${base} bg-red-500/15 text-red-200 hover:bg-red-500/25`}
        >
          <AlertTriangle className="w-3 h-3" aria-hidden /> Falha ao salvar · Tentar de novo
        </button>
      )}
      {status.desktopStale && (
        <span
          className={`${base} bg-amber-400/10 text-amber-100`}
          title="A última edição não chegou ao desktop. A próxima edição tenta de novo."
        >
          <MonitorX className="w-3 h-3" aria-hidden /> Desktop desatualizado
        </span>
      )}
    </div>
  );
};
