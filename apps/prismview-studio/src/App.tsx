import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { AnimatePresence, motion } from "framer-motion";
import {
  Box,
  ChevronDown,
  ChevronRight,
  Compass,
  Download,
  Film,
  Layers,
  Loader2,
  Monitor,
  Package,
  Pause,
  Play,
  Plus,
  Search,
  Settings,
  Sliders,
  Sparkles,
  Trash2,
  Unlock,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import "./styles/prism.css";
import {
  create3DTemplateProjects,
  createDefaultProjects,
  createPure3DProject,
  createPureVideoProject,
  DEFAULT_TELEMETRY,
  invokeCmd,
  isTauriRuntime,
  LiveTelemetry,
  MonitorInfoDto,
  BrokenProjectDto,
  isImportConflict,
  ImportConflictDto,
  normalizeProjectScene3D,
  PickedFileDto,
  PrismProject,
  ProjectListDto,
  RuntimeState,
  SceneLayer,
  SPRING_TACTILE,
  withNativeAssets,
} from "./types/prism";
import { friendlyError } from "./utils/friendlyError";
import { collectOpenAppReview, OpenAppReviewItem, resolveOpenAppReview } from "./utils/importReview";
import { nativeCompatIssues } from "./utils/nativeCompat";

/** Backoff das novas tentativas de salvar (o último valor se repete até dar certo). */
const SAVE_RETRY_DELAYS_MS = [2000, 5000, 15000];
/** Depois de "Aplicar": tempo para o motor abrir vídeo/modelos antes de ler `errors` do status. */
const ENGINE_STATUS_CHECK_DELAY_MS = 2500;
import { UnifiedCompositor } from "./components/UnifiedCompositor";
import { StudioEditor } from "./components/StudioEditor";
import {
  POWER_PREFERENCE_LABELS,
  POWER_STATE_HINTS,
  type PowerPreference,
  usePowerState,
} from "./engine/powerGovernor";

/** 4.7: Hub thumbnail. Fixed telemetry + memo => cards do not re-render on the 1.5 s telemetry tick. */
const HubThumbnail = React.memo(function HubThumbnail({ project }: { project: PrismProject }) {
  return (
    <UnifiedCompositor
      project={project}
      telemetry={DEFAULT_TELEMETRY}
      paused
      isThumbnail
      interactive={false}
    />
  );
});

function useLiveTelemetry(): LiveTelemetry {
  const [telemetry, setTelemetry] = useState<LiveTelemetry>(DEFAULT_TELEMETRY);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unsub: (() => void) | undefined;

    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        const fn = await listen<LiveTelemetry>("prism://telemetry", (ev) => {
          if (!disposed) setTelemetry(ev.payload);
        });
        if (disposed) {
          fn();
          return;
        }
        unsub = fn;
        void invokeCmd<LiveTelemetry>("get_live_telemetry")
          .then((t) => {
            if (!disposed) setTelemetry(t);
          })
          .catch(() => {});
      })
      .catch(() => {});

    return () => {
      disposed = true;
      unsub?.();
    };
  }, []);

  return telemetry;
}

const ToastOverlay: React.FC<{ message: string | null }> = ({ message }) => (
  <AnimatePresence>
    {message && (
      <motion.div
        initial={{ opacity: 0, y: -12, scale: 0.96 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: -8, scale: 0.96 }}
        transition={SPRING_TACTILE}
        className="fixed top-20 left-1/2 -translate-x-1/2 z-[100] frosted-toast rounded-full px-5 py-2.5 text-xs font-medium text-white pointer-events-none"
      >
        {message}
      </motion.div>
    )}
  </AnimatePresence>
);

const LOCK_CLS = "disabled:opacity-50 disabled:cursor-not-allowed";

const ConfirmDialog: React.FC<{
  open: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  /** Texto do botão secundário (padrão "Cancelar"). */
  cancelLabel?: string;
  /** Texto do botão principal durante `busy` (padrão "Excluindo…"). */
  busyLabel?: string;
  /** `danger` (padrão): botão vermelho com lixeira. `neutral`: botão claro sem ícone. */
  tone?: "danger" | "neutral";
  /** Esc / clique fora. Padrão: `onCancel`. */
  onDismiss?: () => void;
}> = ({
  open,
  title,
  message,
  confirmLabel,
  busy,
  onConfirm,
  onCancel,
  cancelLabel = "Cancelar",
  busyLabel = "Excluindo…",
  tone = "danger",
  onDismiss,
}) => {
  const dismiss = onDismiss ?? onCancel;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) dismiss();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, busy, dismiss]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-[110] flex items-center justify-center bg-black/70 backdrop-blur-sm"
          onClick={() => {
            if (!busy) dismiss();
          }}
        >
          <motion.div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="confirm-title"
            aria-describedby="confirm-message"
            initial={{ opacity: 0, y: 8, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, scale: 0.97 }}
            transition={SPRING_TACTILE}
            onClick={(e) => e.stopPropagation()}
            className="frosted-chrome frosted-chrome-panel w-[min(92vw,420px)] p-6 space-y-4"
          >
            <h3 id="confirm-title" className="text-sm font-semibold text-white">
              {title}
            </h3>
            <p id="confirm-message" className="text-xs text-[#A1A1AA] leading-relaxed">
              {message}
            </p>
            <div className="flex justify-end gap-2 pt-1">
              <button
                type="button"
                autoFocus
                disabled={busy}
                onClick={onCancel}
                className={`prism-pill-idle rounded-full px-4 py-2 text-xs ${LOCK_CLS}`}
              >
                {cancelLabel}
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={onConfirm}
                className={`rounded-full px-4 py-2 text-xs font-medium flex items-center gap-1.5 transition ${
                  tone === "danger"
                    ? "bg-red-500/90 hover:bg-red-500 text-white"
                    : "bg-white/90 hover:bg-white text-black"
                } ${LOCK_CLS}`}
              >
                {busy ? (
                  <Loader2 className="w-3.5 h-3.5 animate-spin" />
                ) : tone === "danger" ? (
                  <Trash2 className="w-3.5 h-3.5" />
                ) : null}
                {busy ? busyLabel : confirmLabel}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
};

/**
 * Pós-import: o pacote quer abrir programas. Cada ação começa desmarcada; só as marcadas
 * recebem o caminho original. Esc / clique fora = nenhuma permitida.
 */
const OpenAppReviewDialog: React.FC<{
  items: OpenAppReviewItem[] | null;
  onConfirm: (allowed: ReadonlySet<string>) => void;
}> = ({ items, onConfirm }) => {
  const [allowed, setAllowed] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setAllowed(new Set());
  }, [items]);
  const open = items !== null;
  const denyAll = useCallback(() => onConfirm(new Set()), [onConfirm]);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") denyAll();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, denyAll]);

  return (
    <AnimatePresence>
      {items && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-[110] flex items-center justify-center bg-black/70 backdrop-blur-sm"
          onClick={denyAll}
        >
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-labelledby="open-app-review-title"
            initial={{ opacity: 0, y: 8, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, scale: 0.97 }}
            transition={SPRING_TACTILE}
            onClick={(e) => e.stopPropagation()}
            className="frosted-chrome frosted-chrome-panel w-[min(92vw,480px)] p-6 space-y-4"
          >
            <h3 id="open-app-review-title" className="text-sm font-semibold text-white">
              Este pacote quer abrir programas
            </h3>
            <p className="text-xs text-[#A1A1AA] leading-relaxed">
              Ao tocar nestas camadas do desktop, o PrismView abriria os programas abaixo. Por
              segurança, elas vieram desativadas. Marque só os que você reconhece.
            </p>
            <ul className="space-y-2 max-h-[40vh] overflow-y-auto">
              {items.map((item) => (
                <li key={item.layerId}>
                  <label className="flex items-start gap-2 text-xs text-white cursor-pointer">
                    <input
                      type="checkbox"
                      className="mt-0.5"
                      checked={allowed.has(item.layerId)}
                      onChange={(e) =>
                        setAllowed((prev) => {
                          const next = new Set(prev);
                          if (e.target.checked) next.add(item.layerId);
                          else next.delete(item.layerId);
                          return next;
                        })
                      }
                    />
                    <span className="min-w-0">
                      <span className="font-medium">{item.layerName}</span>
                      <span className="block font-mono text-[11px] text-[#A1A1AA] break-all">
                        {item.target || "(sem caminho)"}
                      </span>
                    </span>
                  </label>
                </li>
              ))}
            </ul>
            <div className="flex justify-end gap-2 pt-1">
              <button
                type="button"
                autoFocus
                onClick={denyAll}
                className="prism-pill-idle rounded-full px-4 py-2 text-xs"
              >
                Manter desativadas
              </button>
              <button
                type="button"
                disabled={allowed.size === 0}
                onClick={() => onConfirm(allowed)}
                className={`rounded-full px-4 py-2 text-xs font-medium bg-white/90 hover:bg-white text-black transition ${LOCK_CLS}`}
              >
                Permitir selecionadas
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
};

const EmptyState: React.FC<{
  icon: React.ReactNode;
  title: string;
  hint: string;
  actionLabel: string;
  disabled?: boolean;
  onAction: () => void;
}> = ({ icon, title, hint, actionLabel, disabled, onAction }) => (
  <div className="frosted-chrome frosted-chrome-panel flex flex-col items-center gap-3 px-6 py-12 text-center">
    <div className="w-10 h-10 rounded-xl bg-white/10 border border-white/15 flex items-center justify-center">
      {icon}
    </div>
    <h3 className="text-sm font-semibold text-white">{title}</h3>
    <p className="text-xs text-[#A1A1AA] max-w-sm leading-relaxed">{hint}</p>
    <button
      type="button"
      disabled={disabled}
      onClick={onAction}
      className={`prismatic-cta px-5 py-2 text-xs flex items-center gap-1.5 ${LOCK_CLS}`}
    >
      <Plus className="w-3.5 h-3.5" /> {actionLabel}
    </button>
  </div>
);

function isGlobalReferencedInLayers(layers: SceneLayer[], name: string): boolean {
  const escaped = name.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (!escaped) return false;
  const re = new RegExp(`\\$gv\\(\\s*["']?${escaped}["']?\\s*\\)`, "i");
  const walk = (list: SceneLayer[]): boolean => {
    for (const l of list) {
      if (l.text?.content && re.test(l.text.content)) return true;
      if (l.fx) {
        for (const expr of Object.values(l.fx)) {
          if (expr && re.test(expr)) return true;
        }
      }
      if (l.children && walk(l.children)) return true;
    }
    return false;
  };
  return walk(layers);
}

const ExposedGlobalsPanel: React.FC<{
  project: PrismProject;
  onImmediateChange: (next: PrismProject) => void;
  onDebouncedChange: (next: PrismProject) => void;
  onCommitChange: (next: PrismProject) => void;
}> = ({ project, onImmediateChange, onDebouncedChange, onCommitChange }) => {
  const exposed = project.globals
    .map((g, idx) => ({ g, idx }))
    .filter(({ g }) => g.exposed && isGlobalReferencedInLayers(project.layers, g.name));
  if (exposed.length === 0) return null;

  const patchGlobalValue = (idx: number, val: string): PrismProject => ({
    ...project,
    globals: project.globals.map((x, i) => (i === idx ? { ...x, value: val } : x)),
  });

  return (
    <div className="space-y-1.5 pt-1 border-t border-white/10">
      {exposed.map(({ g, idx }) => {
        const numVal = Number(g.value);
        const safeNum = Number.isFinite(numVal) ? numVal : 0;
        const maxRange = safeNum > 1 ? Math.max(100, Math.ceil(safeNum * 2)) : 1;
        const stepRange = maxRange > 1 ? 1 : 0.05;
        return (
          <div key={idx} className="flex items-center justify-between gap-2 text-[11px]">
            <span className="text-[#A1A1AA] truncate">{g.label || g.name}</span>
            {g.kind === "SWITCH" ? (
              <input
                type="checkbox"
                checked={g.value === "1" || g.value.toLowerCase() === "true"}
                onChange={(e) => onImmediateChange(patchGlobalValue(idx, e.target.checked ? "1" : "0"))}
              />
            ) : g.kind === "NUMBER" ? (
              <input
                type="range"
                min={0}
                max={maxRange}
                step={stepRange}
                value={safeNum}
                onChange={(e) => onDebouncedChange(patchGlobalValue(idx, e.target.value))}
                onPointerUp={(e) => onCommitChange(patchGlobalValue(idx, e.currentTarget.value))}
                onKeyUp={(e) => onCommitChange(patchGlobalValue(idx, e.currentTarget.value))}
                className="prism-slider w-24"
              />
            ) : g.kind === "COLOR" ? (
              <input
                type="color"
                value={/^#[0-9a-f]{6}$/i.test(g.value) ? g.value : "#FFFFFF"}
                onChange={(e) => onDebouncedChange(patchGlobalValue(idx, e.target.value))}
                onBlur={(e) => onCommitChange(patchGlobalValue(idx, e.currentTarget.value))}
              />
            ) : (
              <input
                value={g.value}
                onChange={(e) => onDebouncedChange(patchGlobalValue(idx, e.target.value))}
                onBlur={(e) => onCommitChange(patchGlobalValue(idx, e.currentTarget.value))}
                className="w-28 bg-black/70 border border-white/15 rounded-lg px-2 py-0.5 text-[11px] text-white outline-none"
              />
            )}
          </div>
        );
      })}
    </div>
  );
};

/**
 * MODE 3: Studio & Hub Application (Disposable Window — Closing keeps Wallpaper/Widgets in Tray)
 */
type HubTab = "WALLPAPERS" | "WIDGETS" | "TEMPLATES";
type WallpaperKindFilter = "ALL" | "3D" | "VIDEO" | "2D";
type TemplateCategoryFilter = "ALL" | "3D" | "2D" | "WIDGET";

function classifyWallpaperKind(project: PrismProject): "3D" | "VIDEO" | "2D" {
  const hasKind = (layers: SceneLayer[], kind: SceneLayer["kind"]): boolean =>
    layers.some((l) => !l.hidden && (l.kind === kind || (l.children ? hasKind(l.children, kind) : false)));
  if (hasKind(project.layers, "SCENE_3D")) return "3D";
  if (hasKind(project.layers, "VIDEO")) return "VIDEO";
  return "2D";
}

const StudioHubApp: React.FC = () => {
  const telemetry = useLiveTelemetry();
  const power = usePowerState(telemetry);

  // Motor nativo (wallpaper): mesma preferência do Hub. Motor parado não é erro — o backend grava
  // `power_preference.txt` e o motor a lê ao iniciar.
  const pushNativePower = useCallback(async (pref: PowerPreference, silent: boolean) => {
    if (!isTauriRuntime()) return;
    try {
      await invokeCmd("native_engine_set_power", { preference: pref });
    } catch (err) {
      if (!silent) notify(friendlyError(err, "Não foi possível ajustar a energia do motor nativo."));
      else console.error("[PrismView]", err);
    }
  }, []);
  const changePowerPreference = useCallback(
    (pref: PowerPreference) => {
      power.setPreference(pref);
      void pushNativePower(pref, false);
    },
    [power.setPreference, pushNativePower]
  );
  useEffect(() => {
    void pushNativePower(power.preference, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [hubTab, setHubTab] = useState<HubTab>("WALLPAPERS");
  const [wallpaperFilter, setWallpaperFilter] = useState<WallpaperKindFilter>("ALL");
  const [templateFilter, setTemplateFilter] = useState<TemplateCategoryFilter>("ALL");
  const [hubQuery, setHubQuery] = useState("");
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [blenderGuideOpen, setBlenderGuideOpen] = useState(false);
  const [projects, setProjects] = useState<PrismProject[]>([]);
  const [editing, setEditing] = useState<PrismProject | null>(null);
  const [runtime, setRuntime] = useState<RuntimeState>({
    paused: false,
    desktop_visible: true,
    active_wallpaper_id: null,
    pinned_widget_ids: [],
    locked_widget_ids: [],
    monitors: [],
  });
  const [monitors, setMonitors] = useState<MonitorInfoDto[]>([]);
  const [autostart, setAutostart] = useState(false);
  const [engineTrayEnabled, setEngineTrayEnabled] = useState(true);
  const [trayBusy, setTrayBusy] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const optionsRef = useRef<HTMLDivElement | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const pendingSaveRef = useRef<PrismProject | null>(null);
  // Trava de ações pesadas (aplicar/fixar/exportar/importar/excluir); pausa/autostart têm estado próprio.
  const busyRef = useRef<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [pauseBusy, setPauseBusy] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<PrismProject | null>(null);
  const [brokenProjects, setBrokenProjects] = useState<BrokenProjectDto[]>([]);
  const [brokenDetailsOpen, setBrokenDetailsOpen] = useState(false);
  const [importConflict, setImportConflict] = useState<{ path: string; existingTitle: string } | null>(null);
  const [openAppReview, setOpenAppReview] = useState<{
    project: PrismProject;
    items: OpenAppReviewItem[];
  } | null>(null);
  /** `silent`: só reserva a trava (sem re-render), p.ex. sincronizações automáticas de widget. */
  const withBusy = useCallback(async (key: string, fn: () => Promise<void>, silent = false): Promise<boolean> => {
    if (busyRef.current) return false;
    busyRef.current = key;
    if (!silent) setBusy(key);
    try {
      await fn();
    } finally {
      busyRef.current = null;
      if (!silent) setBusy(null);
    }
    return true;
  }, []);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimerRef.current !== null) {
      window.clearTimeout(toastTimerRef.current);
    }
    toastTimerRef.current = window.setTimeout(() => {
      toastTimerRef.current = null;
      setToast(null);
    }, 3200);
  }, []);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current !== null) {
        window.clearTimeout(toastTimerRef.current);
      }
      if (saveRetryRef.current.timer !== null) {
        window.clearTimeout(saveRetryRef.current.timer);
      }
    };
  }, []);

  // ── Salvamento com aviso e nova tentativa ──
  // Falha ao salvar nunca é silenciosa: o projeto fica em `failedSavesRef` (sujo), o usuário vê o
  // aviso uma vez e novas tentativas seguem com backoff até dar certo.
  const failedSavesRef = useRef(new Map<string, PrismProject>());
  const saveRetryRef = useRef<{ attempt: number; timer: number | null }>({ attempt: 0, timer: null });
  const saveProjectOrReportRef = useRef<(project: PrismProject) => Promise<boolean>>(async () => false);

  const saveProjectOrReport = useCallback(
    async (project: PrismProject): Promise<boolean> => {
      const retry = saveRetryRef.current;
      try {
        await invokeCmd("save_project", { project });
        failedSavesRef.current.delete(project.id);
        if (failedSavesRef.current.size === 0) {
          retry.attempt = 0;
          if (retry.timer !== null) {
            window.clearTimeout(retry.timer);
            retry.timer = null;
          }
        }
        return true;
      } catch (err) {
        if (!isTauriRuntime()) return false;
        failedSavesRef.current.set(project.id, project);
        if (retry.attempt === 0) {
          notify(`Não foi possível salvar: ${friendlyError(err, "erro desconhecido")} Tentaremos de novo.`);
        }
        if (retry.timer === null) {
          const delay = SAVE_RETRY_DELAYS_MS[Math.min(retry.attempt, SAVE_RETRY_DELAYS_MS.length - 1)];
          retry.attempt += 1;
          retry.timer = window.setTimeout(() => {
            retry.timer = null;
            for (const pending of [...failedSavesRef.current.values()]) {
              void saveProjectOrReportRef.current(pending);
            }
          }, delay);
        }
        return false;
      }
    },
    [notify]
  );
  saveProjectOrReportRef.current = saveProjectOrReport;

  /** Última chance (fechar a janela): regrava o que falhou antes. */
  const flushFailedSaves = useCallback(async () => {
    await Promise.all([...failedSavesRef.current.values()].map((p) => saveProjectOrReport(p)));
  }, [saveProjectOrReport]);

  const loadAll = useCallback(async () => {
    const [savedRes, rtRes, autoRes, trayRes] = await Promise.allSettled([
      invokeCmd<ProjectListDto>("list_projects"),
      invokeCmd<RuntimeState>("get_runtime_state"),
      invokeCmd<boolean>("get_autostart_enabled"),
      invokeCmd<boolean>("get_engine_tray_enabled"),
    ]);

    if (savedRes.status === "fulfilled") {
      const { projects: saved, broken } = savedRes.value;
      setBrokenProjects(broken);
      if (saved.length > 0) {
        // Border migration: legacy 3D configs (30/60/120 FPS, single modelSrc) → schema 2
        setProjects(saved.map((p) => normalizeProjectScene3D(p)));
      } else if (broken.length > 0) {
        // Projetos ilegíveis (em quarentena): não semear modelos por cima, só avisar.
        setProjects([]);
      } else {
        const defaults = createDefaultProjects();
        setProjects(defaults);
        void Promise.all(defaults.map((p) => saveProjectOrReport(p)));
      }
    } else {
      setProjects(createDefaultProjects());
    }

    if (rtRes.status === "fulfilled") {
      const rt = rtRes.value;
      setRuntime(rt);
      if (rt.monitors?.length) {
        setMonitors(rt.monitors);
      } else {
        void invokeCmd<MonitorInfoDto[]>("list_monitors")
          .then((mons) => {
            if (mons.length) setMonitors(mons);
          })
          .catch(() => {});
      }
    }

    if (autoRes.status === "fulfilled") {
      setAutostart(Boolean(autoRes.value));
    }
    if (trayRes.status === "fulfilled") {
      setEngineTrayEnabled(Boolean(trayRes.value));
    }
  }, [saveProjectOrReport]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  // Migração de wallpaper legado falhou depois do 1º load: busca o estado atualizado (aviso + sem ativo).
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen("prism://legacy-migration-failed", () => {
        void invokeCmd<RuntimeState>("get_runtime_state").then(setRuntime).catch(() => {});
      }).then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const dismissLegacyNotice = useCallback(() => {
    setRuntime((rt) => ({ ...rt, legacy_migration_notice: null }));
    void invokeCmd("dismiss_legacy_migration_notice").catch(() => {});
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unsub: (() => void) | undefined;
    void import("@tauri-apps/api/event").then(async ({ listen }) => {
      const fn = await listen<MonitorInfoDto[]>("prism://monitors-changed", (ev) => {
        if (disposed) return;
        setMonitors(ev.payload);
      });
      if (disposed) {
        fn();
      } else {
        unsub = fn;
      }
    });
    return () => {
      disposed = true;
      unsub?.();
    };
  }, []);

  const wallpapers = useMemo(
    () => projects.filter((p) => p.packageType === "WALLPAPER"),
    [projects]
  );
  const widgets = useMemo(
    () => projects.filter((p) => p.packageType === "WIDGET"),
    [projects]
  );
  const templates3d = useMemo(() => create3DTemplateProjects(), []);
  const defaultTemplates = useMemo(() => {
    const ids3d = new Set(templates3d.map((t) => t.project.id));
    return createDefaultProjects().filter((p) => !ids3d.has(p.id));
  }, [templates3d]);

  const wallpaperCounts = useMemo(() => {
    const counts = { ALL: wallpapers.length, "3D": 0, VIDEO: 0, "2D": 0 };
    for (const wp of wallpapers) {
      counts[classifyWallpaperKind(wp)] += 1;
    }
    return counts;
  }, [wallpapers]);

  const filteredWallpapers = useMemo(() => {
    const q = hubQuery.trim().toLowerCase();
    return wallpapers.filter((wp) => {
      if (wallpaperFilter !== "ALL" && classifyWallpaperKind(wp) !== wallpaperFilter) return false;
      if (q && !wp.title.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [wallpapers, wallpaperFilter, hubQuery]);

  const filteredWidgets = useMemo(() => {
    const q = hubQuery.trim().toLowerCase();
    if (!q) return widgets;
    return widgets.filter((w) => w.title.toLowerCase().includes(q));
  }, [widgets, hubQuery]);

  const unifiedTemplates = useMemo(() => {
    const items: Array<{
      project: PrismProject;
      category: "3D" | "2D" | "WIDGET";
      subtitle: string;
      badges: string[];
    }> = [
      ...templates3d.map((t) => ({
        project: t.project,
        category: "3D" as const,
        subtitle: t.subtitle,
        badges: t.badges,
      })),
      ...defaultTemplates.map((p) => ({
        project: p,
        category: p.packageType === "WIDGET" ? ("WIDGET" as const) : ("2D" as const),
        subtitle:
          p.packageType === "WIDGET"
            ? "Widget independente pronto para fixar sobre qualquer papel de parede."
            : "Composição editorial pronta com tipografia e vidro óptico configurados.",
        badges:
          p.packageType === "WIDGET"
            ? ["Widget", `${p.widget.width}×${p.widget.height}`]
            : ["Vidro Óptico", `${p.layers.length} camadas`],
      })),
    ];
    const q = hubQuery.trim().toLowerCase();
    return items.filter((item) => {
      if (templateFilter !== "ALL" && item.category !== templateFilter) return false;
      if (q && !item.project.title.toLowerCase().includes(q) && !item.subtitle.toLowerCase().includes(q)) {
        return false;
      }
      return true;
    });
  }, [templates3d, defaultTemplates, templateFilter, hubQuery]);

  const runtimeRef = useRef(runtime);
  runtimeRef.current = runtime;
  // Serializa sincronizações automáticas com o desktop (último snapshot vence, sem IPC paralelo).
  const desktopSyncInFlightRef = useRef(false);
  const desktopSyncPendingRef = useRef<PrismProject | null>(null);

  // Sync automático com o desktop: a 1ª falha seguida avisa; o aviso só volta após um sucesso.
  const desktopSyncFailedRef = useRef(false);
  const runDesktopSyncNow = useCallback(
    async (proj: PrismProject) => {
      const rt = runtimeRef.current;
      let request: Promise<RuntimeState> | null = null;
      if (proj.packageType === "WIDGET") {
        if (rt.pinned_widget_ids.includes(proj.id)) {
          request = invokeCmd<RuntimeState>("pin_standalone_widget", { project: proj });
        }
      } else if (rt.active_wallpaper_id === proj.id) {
        // `autosave`: só cena + recarga no motor (o .prism não é re-zipado a cada pausa).
        request = invokeCmd<RuntimeState>("apply_desktop_wallpaper", {
          project: withNativeAssets(proj),
          monitorId: null,
          autosave: true,
        });
      }
      if (!request) return;
      try {
        setRuntime(await request);
        desktopSyncFailedRef.current = false;
      } catch (err) {
        if (!desktopSyncFailedRef.current) {
          desktopSyncFailedRef.current = true;
          notify(
            `O desktop não pôde ser atualizado: ${friendlyError(err, "erro desconhecido")} A próxima edição tenta de novo.`
          );
        } else {
          console.error("[PrismView] sync desktop:", err);
        }
      }
    },
    [notify]
  );

  const syncIfActiveOnDesktop = useCallback(
    async (proj: PrismProject) => {
      if (!isTauriRuntime()) return;
      desktopSyncPendingRef.current = proj;
      if (desktopSyncInFlightRef.current) return;
      desktopSyncInFlightRef.current = true;
      try {
        while (desktopSyncPendingRef.current) {
          const next = desktopSyncPendingRef.current;
          desktopSyncPendingRef.current = null;
          await runDesktopSyncNow(next);
        }
      } finally {
        desktopSyncInFlightRef.current = false;
      }
    },
    [runDesktopSyncNow]
  );

  const txActiveRef = useRef(false);
  const txReleaseCleanupRef = useRef<(() => void) | null>(null);

  const flushPendingSave = useCallback(
    async (skipDesktopSync = false) => {
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      const pending = pendingSaveRef.current;
      if (!pending) return;
      pendingSaveRef.current = null;
      // Save border: normalize once on commit/save instead of on every slider frame
      const normalized = normalizeProjectScene3D(pending);
      setEditing((cur) => (cur?.id === normalized.id ? normalized : cur));
      setProjects((prev) =>
        prev.some((p) => p.id === normalized.id)
          ? prev.map((p) => (p.id === normalized.id ? normalized : p))
          : [...prev, normalized]
      );
      await saveProjectOrReport(normalized);
      if (!skipDesktopSync) {
        await syncIfActiveOnDesktop(normalized);
      }
    },
    [syncIfActiveOnDesktop, saveProjectOrReport]
  );

  const setEditingTxActive = useCallback(
    (active: boolean) => {
      txActiveRef.current = active;
      if (txReleaseCleanupRef.current) {
        txReleaseCleanupRef.current();
        txReleaseCleanupRef.current = null;
      }
      if (active) {
        // Never fire autosave/desktop sync mid-drag.
        if (saveTimerRef.current !== null) {
          window.clearTimeout(saveTimerRef.current);
          saveTimerRef.current = null;
        }
        // Safety net: if pointerup happens outside the control, unlock tx cleanly.
        const release = () => {
          if (!txActiveRef.current) return;
          txActiveRef.current = false;
          if (txReleaseCleanupRef.current) {
            txReleaseCleanupRef.current();
            txReleaseCleanupRef.current = null;
          }
          if (pendingSaveRef.current && saveTimerRef.current === null) {
            saveTimerRef.current = window.setTimeout(() => {
              saveTimerRef.current = null;
              void flushPendingSave();
            }, 220);
          }
        };
        window.addEventListener("pointerup", release, { once: true });
        window.addEventListener("pointercancel", release, { once: true });
        txReleaseCleanupRef.current = () => {
          window.removeEventListener("pointerup", release);
          window.removeEventListener("pointercancel", release);
        };
      }
    },
    [flushPendingSave]
  );

  useEffect(() => {
    // Fallback (navegador): `beforeunload` não espera promessas.
    const onUnload = () => {
      void flushPendingSave();
    };
    window.addEventListener("beforeunload", onUnload);
    // Tauri: a janela só é destruída depois que o handler termina (sem preventDefault o
    // `onCloseRequested` chama `destroy()` ao fim), então o save pendente chega ao Rust.
    let unlistenClose: (() => void) | null = null;
    let disposed = false;
    if (isTauriRuntime()) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) =>
          getCurrentWindow().onCloseRequested(async () => {
            await flushPendingSave();
            await flushFailedSaves();
          })
        )
        .then((unlisten) => {
          if (disposed) unlisten();
          else unlistenClose = unlisten;
        })
        .catch((err) => console.error("[PrismView] onCloseRequested:", err));
    }
    return () => {
      disposed = true;
      unlistenClose?.();
      window.removeEventListener("beforeunload", onUnload);
      if (txReleaseCleanupRef.current) {
        txReleaseCleanupRef.current();
        txReleaseCleanupRef.current = null;
      }
      void flushPendingSave();
    };
  }, [flushPendingSave, flushFailedSaves]);

  const persistProject = useCallback(
    (incoming: PrismProject, transient = false) => {
      // Live path: update ONLY the active editor project (never re-render Hub `projects[]`
      // or run deep 3D normalization on every slider tick).
      const next = transient ? incoming : normalizeProjectScene3D(incoming);
      setEditing((cur) => (cur?.id === next.id ? next : cur));
      if (pendingSaveRef.current && pendingSaveRef.current.id !== next.id) {
        const previous = normalizeProjectScene3D(pendingSaveRef.current);
        pendingSaveRef.current = null;
        setProjects((prev) =>
          prev.some((p) => p.id === previous.id)
            ? prev.map((p) => (p.id === previous.id ? previous : p))
            : [...prev, previous]
        );
        void saveProjectOrReport(previous);
      }
      pendingSaveRef.current = next;
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      // Pause debounced disk save & desktop sync while a drag/slider transaction is in progress.
      if (txActiveRef.current) {
        return;
      }
      saveTimerRef.current = window.setTimeout(() => {
        saveTimerRef.current = null;
        void flushPendingSave();
      }, 450);
    },
    [flushPendingSave]
  );

  const persistProjectLive = useCallback(
    (incoming: PrismProject) => persistProject(incoming, true),
    [persistProject]
  );

  const persistProjectImmediate = useCallback(async (incoming: PrismProject) => {
    const next = normalizeProjectScene3D(incoming);
    if (pendingSaveRef.current?.id === next.id) {
      pendingSaveRef.current = null;
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    }
    setEditing((cur) => (cur?.id === next.id ? next : cur));
    setProjects((prev) =>
      prev.some((p) => p.id === next.id)
        ? prev.map((p) => (p.id === next.id ? next : p))
        : [...prev, next]
    );
    await saveProjectOrReport(next);
  }, [saveProjectOrReport]);

  const applyWallpaper = async (proj: PrismProject, silent = false) => {
    // Clique explícito sempre vence qualquer autosave enfileirado.
    desktopSyncPendingRef.current = null;
    await withBusy(
      `apply:${proj.id}`,
      async () => {
        try {
          const rt = await invokeCmd<RuntimeState>("apply_desktop_wallpaper", {
            project: withNativeAssets(proj),
            monitorId: null,
          });
          setRuntime(rt);
          desktopSyncFailedRef.current = false;
          if (rt.monitors?.length) setMonitors(rt.monitors);
          // Avisos do conversor (arquivo ausente…) + recursos que o desktop não desenha.
          const problems = [
            ...(rt.warnings ?? []),
            ...nativeCompatIssues(proj).map((i) => i.message),
          ];
          if (problems.length > 0) {
            console.warn("[PrismView] avisos ao aplicar:", problems);
            notify(
              `"${proj.title}" aplicado com ${problems.length} aviso(s): ${problems[0]}${
                problems.length > 1 ? ` (+${problems.length - 1})` : ""
              }`
            );
          } else if (!silent) {
            // Motor nativo desenha todos os monitores com a mesma cena.
            const targetLabel = monitors.length > 1 ? ` em ${monitors.length} monitores` : "";
            notify(`Wallpaper "${proj.title}" aplicado${targetLabel}`);
          }
          // Vídeo/modelo/imagem carregam depois no motor: confere os erros dele um pouco depois.
          window.setTimeout(() => {
            void invokeCmd<{ errors?: { layer_id?: string | null; message: string }[] }>(
              "native_engine_status"
            )
              .then((status) => {
                const errors = status.errors ?? [];
                if (errors.length === 0) return;
                console.warn("[PrismView] erros do motor:", errors);
                notify(
                  `Desktop: ${errors[0].message}${errors.length > 1 ? ` (+${errors.length - 1})` : ""}`
                );
              })
              .catch((err) => console.error("[PrismView] status do motor:", err));
          }, ENGINE_STATUS_CHECK_DELAY_MS);
        } catch (err) {
          if (!silent) {
            notify(
              isTauriRuntime()
                ? friendlyError(err, "Não foi possível aplicar o wallpaper.")
                : "Disponível no app desktop"
            );
          } else {
            console.error("[PrismView]", err);
          }
        }
      },
      silent
    );
  };

  const clearWallpaper = async () => {
    await withBusy("clear", async () => {
      try {
        const rt = await invokeCmd<RuntimeState>("clear_desktop_wallpaper", { monitorId: null });
        setRuntime(rt);
        notify("Wallpaper removido do desktop");
      } catch (err) {
        notify(friendlyError(err, "Não foi possível remover o wallpaper."));
      }
    });
  };

  const pinWidget = async (proj: PrismProject, silent = false) => {
    await withBusy(
      `pin:${proj.id}`,
      async () => {
        try {
          const rt = await invokeCmd<RuntimeState>("pin_standalone_widget", { project: proj });
          setRuntime(rt);
          if (!silent) notify(`Widget "${proj.title}" fixado na tela`);
        } catch (err) {
          if (!silent) {
            notify(
              isTauriRuntime()
                ? friendlyError(err, "Não foi possível fixar o widget.")
                : "Disponível no app desktop"
            );
          } else {
            console.error("[PrismView]", err);
          }
        }
      },
      silent
    );
  };

  const unpinWidget = async (widgetId: string) => {
    await withBusy(`unpin:${widgetId}`, async () => {
      try {
        const rt = await invokeCmd<RuntimeState>("unpin_standalone_widget", { widgetId });
        setRuntime(rt);
        notify("Widget removido da área de trabalho");
      } catch (err) {
        notify(friendlyError(err, "Não foi possível remover o widget."));
      }
    });
  };

  const exportProject = async (proj: PrismProject) => {
    const isWidget = proj.packageType === "WIDGET";
    const safeTitle = proj.title.trim().replace(/[^a-zA-Z0-9_-]+/g, "_") || "PrismView";
    const defaultFileName = `${safeTitle}.${isWidget ? "widget.prism" : "prism"}`;

    await withBusy(`export:${proj.id}`, async () => {
      if (isTauriRuntime()) {
        try {
          const chosenPath = await invokeCmd<string | null>("pick_save_archive_path", {
            defaultFileName,
            isWidget,
          });
          if (!chosenPath) return;
          const res = await invokeCmd<{ archive_path: string; file_name: string }>(
            "export_prism_archive",
            { project: proj, destPath: chosenPath }
          );
          notify(`Exportado: ${res.file_name}`);
        } catch (err) {
          notify(friendlyError(err, "Não foi possível exportar o projeto."));
        }
        return;
      }

      const blob = new Blob([JSON.stringify(proj, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = defaultFileName;
      a.click();
      URL.revokeObjectURL(url);
      notify(`Exportado localmente: ${defaultFileName}`);
    });
  };

  const importPrismFromDisk = async () => {
    await withBusy("import", async () => {
      try {
        const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "prism" });
        if (!picked) return;
        await runImport(picked.path);
      } catch (err) {
        notify(friendlyError(err, "Não foi possível importar o arquivo .prism."));
      }
    });
  };

  /**
   * Importa um `.prism`. Sem `replace`, um id já existente volta como conflito e o usuário
   * escolhe no diálogo ("Manter os dois" = cópia com id novo; "Substituir" = sobrescreve).
   */
  const runImport = async (archivePath: string, replace?: boolean) => {
    const result = await invokeCmd<PrismProject | ImportConflictDto>("import_prism_archive", {
      archivePath,
      replace,
    });
    if (isImportConflict(result)) {
      setImportConflict({ path: archivePath, existingTitle: result.existingTitle });
      return;
    }
    const imported = normalizeProjectScene3D(result);
    // Substituiu o projeto aberto no editor: o autosave não pode regravar a versão antiga.
    if (pendingSaveRef.current?.id === imported.id) {
      pendingSaveRef.current = null;
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    }
    failedSavesRef.current.delete(imported.id);
    setEditing((cur) => (cur?.id === imported.id ? imported : cur));
    setProjects((prev) =>
      prev.some((p) => p.id === imported.id)
        ? prev.map((p) => (p.id === imported.id ? imported : p))
        : [...prev, imported]
    );
    setHubTab(imported.packageType === "WIDGET" ? "WIDGETS" : "WALLPAPERS");
    notify(`Importado: ${imported.title}`);
    const review = collectOpenAppReview(imported.layers);
    if (review.length > 0) setOpenAppReview({ project: imported, items: review });
  };

  const resolveImportConflict = async (replace: boolean) => {
    const conflict = importConflict;
    if (!conflict) return;
    await withBusy("import", async () => {
      try {
        await runImport(conflict.path, replace);
        setImportConflict(null);
      } catch (err) {
        setImportConflict(null);
        notify(friendlyError(err, "Não foi possível importar o arquivo .prism."));
      }
    });
  };

  const confirmOpenAppReview = async (allowed: ReadonlySet<string>) => {
    const review = openAppReview;
    setOpenAppReview(null);
    if (!review) return;
    const updated: PrismProject = {
      ...review.project,
      layers: resolveOpenAppReview(review.project.layers, allowed),
    };
    await persistProjectImmediate(updated);
    notify(
      allowed.size > 0
        ? `${allowed.size} ação(ões) "Abrir app" permitida(s).`
        : 'Ações "Abrir app" do pacote ficaram desativadas.'
    );
  };

  const importLocalModel3D = async (openInStudio: boolean) => {
    const out: { proj: PrismProject | null } = { proj: null };
    await withBusy("import", async () => {
      try {
        const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "model3d" });
        if (!picked) return;
        const title = picked.file_name.replace(/\.[^.]+$/, "");
        const created = createPure3DProject(title, picked.path);
        await persistProjectImmediate(created);
        out.proj = created;
      } catch (err) {
        notify(
          isTauriRuntime()
            ? friendlyError(err, "Não foi possível importar o modelo 3D.")
            : "Disponível no app nativo Tauri"
        );
      }
    });
    if (!out.proj) return;
    if (openInStudio) setEditing(out.proj);
    else await applyWallpaper(out.proj);
  };

  const importLocalVideoOrImage = async (openInStudio: boolean) => {
    const out: { proj: PrismProject | null } = { proj: null };
    await withBusy("import", async () => {
      try {
        const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "media" });
        if (!picked) return;
        const title = picked.file_name.replace(/\.[^.]+$/, "");
        let proj: PrismProject;
        if (picked.is_model_3d) {
          proj = createPure3DProject(title, picked.path);
        } else {
          proj = createPureVideoProject(title, picked.path);
          if (!picked.is_video) {
            proj.layers[0] = {
              ...proj.layers[0],
              kind: "IMAGE",
              video: undefined,
              image: {
                src: picked.path,
                objectFit: "cover",
                borderRadius: 0,
              },
            };
          }
        }
        await persistProjectImmediate(proj);
        out.proj = proj;
      } catch (err) {
        notify(
          isTauriRuntime()
            ? friendlyError(err, "Não foi possível importar a mídia.")
            : "Disponível no app nativo Tauri"
        );
      }
    });
    if (!out.proj) return;
    if (openInStudio) setEditing(out.proj);
    else await applyWallpaper(out.proj);
  };

  const createNew3DWallpaper = async () => {
    const created = createPure3DProject(`Cena 3D ${wallpapers.length + 1}`, "");
    await persistProjectImmediate(created);
    setEditing(created);
  };

  const createNew = async (kind: "WALLPAPER" | "WIDGET") => {
    const defaults = createDefaultProjects();
    const base = defaults.find((p) => p.packageType === kind) ?? defaults[0];
    const created: PrismProject = {
      ...JSON.parse(JSON.stringify(base)),
      id: `proj-${Date.now().toString(36)}`,
      title: kind === "WIDGET" ? `Novo Widget ${widgets.length + 1}` : `Novo Wallpaper ${wallpapers.length + 1}`,
      packageType: kind,
    };
    await persistProjectImmediate(created);
    setEditing(created);
  };

  const addMissingDefaultPresets = async (kind: "WALLPAPER" | "WIDGET") => {
    const defaults = createDefaultProjects().filter((p) => p.packageType === kind);
    let added = 0;
    for (const preset of defaults) {
      if (!projects.some((p) => p.id === preset.id)) {
        await persistProjectImmediate(preset);
        added += 1;
      }
    }
    if (added > 0) {
      notify(`${added} ${added === 1 ? "modelo base adicionado" : "modelos base adicionados"}`);
    } else {
      notify("Todos os modelos base já estão na biblioteca");
    }
  };

  const logIgnored = (e: unknown) => {
    console.error("[PrismView]", e);
    return null;
  };

  const deleteProj = async (proj: PrismProject): Promise<boolean> =>
    withBusy(`delete:${proj.id}`, async () => {
      // Cancel any debounced save for this project so it can never resurrect on disk
      if (pendingSaveRef.current?.id === proj.id) {
        pendingSaveRef.current = null;
        if (saveTimerRef.current !== null) {
          window.clearTimeout(saveTimerRef.current);
          saveTimerRef.current = null;
        }
      }
      // Tirar do desktop é melhor-esforço; falha aqui não impede a exclusão.
      if (proj.packageType === "WIDGET") {
        if (runtime.pinned_widget_ids.includes(proj.id)) {
          const rt = await invokeCmd<RuntimeState>("unpin_standalone_widget", {
            widgetId: proj.id,
          }).catch(logIgnored);
          if (rt) setRuntime(rt);
        }
      } else {
        if (runtime.active_wallpaper_id === proj.id) {
          const rt = await invokeCmd<RuntimeState>("clear_desktop_wallpaper", {
            monitorId: null,
          }).catch(logIgnored);
          if (rt) setRuntime(rt);
        }
      }
      try {
        await invokeCmd("delete_project", { projectId: proj.id });
      } catch (err) {
        // Não some da lista se o arquivo não foi apagado de fato.
        notify(friendlyError(err, `Não foi possível excluir "${proj.title}".`));
        return;
      }
      setProjects((prev) => prev.filter((p) => p.id !== proj.id));
      notify(`Excluído: ${proj.title}`);
    });

  const confirmDelete = async () => {
    const target = pendingDelete;
    if (!target) return;
    if (await deleteProj(target)) setPendingDelete(null);
  };

  const deleteDialogMessage = (() => {
    if (!pendingDelete) return "";
    const isWidget = pendingDelete.packageType === "WIDGET";
    const active = isWidget
      ? runtime.pinned_widget_ids.includes(pendingDelete.id)
      : runtime.active_wallpaper_id === pendingDelete.id;
    const extra = active
      ? isWidget
        ? " Ele está fixado na tela e será removido de lá."
        : " Ele está ativo no desktop e será removido de lá."
      : "";
    return `"${pendingDelete.title}" será excluído permanentemente. Esta ação não pode ser desfeita.${extra}`;
  })();

  const togglePause = async () => {
    if (pauseBusy) return;
    const nextPaused = !runtimeRef.current.paused;
    setPauseBusy(true);
    // Feedback imediato no botão Ativo/Pausado sem esperar outras ações do Hub.
    setRuntime((prev) => ({ ...prev, paused: nextPaused }));
    try {
      const rt = await invokeCmd<RuntimeState>("set_engine_paused", { paused: nextPaused });
      setRuntime(rt);
    } catch (err) {
      setRuntime((prev) => ({ ...prev, paused: !nextPaused }));
      notify(friendlyError(err, "Não foi possível pausar/retomar o motor."));
    } finally {
      setPauseBusy(false);
    }
  };

  useEffect(() => {
    if (!optionsOpen) return;
    const onPointerDown = (ev: MouseEvent) => {
      if (optionsRef.current && !optionsRef.current.contains(ev.target as Node)) {
        setOptionsOpen(false);
      }
    };
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setOptionsOpen(false);
    };
    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [optionsOpen]);

  useEffect(() => {
    if (!createModalOpen) return;
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setCreateModalOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [createModalOpen]);

  const toggleEngineTray = async () => {
    if (trayBusy) return;
    const next = !engineTrayEnabled;
    setTrayBusy(true);
    setEngineTrayEnabled(next);
    try {
      const saved = await invokeCmd<boolean>("set_engine_tray_visible", { visible: next });
      setEngineTrayEnabled(Boolean(saved));
      notify(
        saved
          ? "Engine visível na bandeja do sistema"
          : "Engine oculta da bandeja do sistema"
      );
    } catch (err) {
      setEngineTrayEnabled(!next);
      notify(
        isTauriRuntime()
          ? friendlyError(err, "Não foi possível alterar a bandeja da engine.")
          : "Disponível no app desktop"
      );
    } finally {
      setTrayBusy(false);
    }
  };

  const stopBackgroundEngine = async () => {
    setOptionsOpen(false);
    await withBusy("stop-engine", async () => {
      try {
        await invokeCmd("native_engine_stop");
        notify("Engine em segundo plano encerrada");
      } catch (err) {
        notify(
          isTauriRuntime()
            ? friendlyError(err, "Não foi possível encerrar a engine.")
            : "Disponível no app desktop"
        );
      }
    });
  };

  const toggleAutostart = async () => {
    if (autostartBusy) return;
    setAutostartBusy(true);
    try {
      const next = await invokeCmd<boolean>("set_autostart_enabled", { enabled: !autostart });
      setAutostart(Boolean(next));
      notify(next ? "Iniciar com o Windows ativado" : "Iniciar com o Windows desativado");
    } catch (err) {
      notify(
        isTauriRuntime()
          ? friendlyError(err, "Não foi possível alterar o início automático.")
          : "Disponível no app desktop"
      );
    } finally {
      setAutostartBusy(false);
    }
  };

  const targetMonitorAspect = useMemo(() => {
    const mon = monitors.find((m) => m.is_primary) ?? monitors[0];
    if (mon && mon.width > 0 && mon.height > 0) {
      return mon.width / mon.height;
    }
    return 16 / 9;
  }, [monitors]);

  if (editing) {
    return (
      <div className="w-screen h-screen bg-black text-white overflow-hidden">
        <StudioEditor
          project={editing}
          availableWidgets={widgets}
          telemetry={telemetry}
          monitorAspect={targetMonitorAspect}
          onChangeProject={persistProject}
          onChangeProjectLive={persistProjectLive}
          onTxStateChange={setEditingTxActive}
          onFlushSave={() => void flushPendingSave()}
          onBack={() => {
            setEditingTxActive(false);
            void flushPendingSave();
            setEditing(null);
          }}
          applying={busy !== null && (busy.startsWith("apply:") || busy.startsWith("pin:"))}
          exporting={busy !== null && busy.startsWith("export:")}
          onApply={() => {
            setEditingTxActive(false);
            const ready = normalizeProjectScene3D(editing);
            void flushPendingSave(true);
            if (ready.packageType === "WIDGET") {
              void pinWidget(ready);
            } else {
              void applyWallpaper(ready);
            }
          }}
          onExport={() => {
            setEditingTxActive(false);
            const ready = normalizeProjectScene3D(editing);
            void flushPendingSave();
            void exportProject(ready);
          }}
        />
        <ToastOverlay message={toast} />
      </div>
    );
  }

  const totalTemplatesCount = templates3d.length + defaultTemplates.length;

  return (
    <div className="relative w-screen h-screen bg-[#060608] text-white flex overflow-hidden">
      {/* Subtle Monochrome Architectural Light */}
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(ellipse 70% 42% at 55% 0%, rgba(255,255,255,0.045), transparent 72%)",
        }}
      />

      {/* Persistent Left Sidebar (Creative Hub Navigation) */}
      <aside className="relative z-20 w-[236px] shrink-0 border-r border-white/[0.08] bg-black/45 backdrop-blur-xl flex flex-col justify-between p-4">
        <div className="space-y-5">
          {/* Brand */}
          <div className="px-2 pt-1 flex items-center justify-between">
            <div>
              <span className="font-display text-lg font-semibold tracking-tight text-white block leading-none">
                PrismView
              </span>
              <span className="text-[10px] font-mono-fx uppercase tracking-widest text-[#71717A]">
                Desktop Studio
              </span>
            </div>
            <span
              title={runtime.paused ? "Motor pausado" : "Motor ativo"}
              className={`w-2 h-2 rounded-full ${
                !runtime.paused ? "bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.7)]" : "bg-[#52525B]"
              }`}
            />
          </div>

          {/* Primary Creation CTA */}
          <button
            type="button"
            onClick={() => setCreateModalOpen(true)}
            disabled={busy !== null}
            className={`${LOCK_CLS} prismatic-cta w-full py-2.5 px-4 text-xs flex items-center justify-center gap-2`}
          >
            <Plus className="w-4 h-4" />
            Novo Projeto
          </button>

          {/* Section 1: Biblioteca */}
          <nav className="space-y-1" aria-label="Biblioteca">
            <div className="px-2.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717A]">
              Biblioteca
            </div>
            {(
              [
                { id: "WALLPAPERS" as HubTab, label: "Wallpapers", icon: Monitor, count: wallpapers.length },
                { id: "WIDGETS" as HubTab, label: "Widgets", icon: Layers, count: widgets.length },
              ]
            ).map((item) => {
              const Icon = item.icon;
              const active = hubTab === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => {
                    setHubTab(item.id);
                    setHubQuery("");
                  }}
                  className={`w-full px-3 py-2 rounded-xl text-xs flex items-center justify-between transition ${
                    active
                      ? "bg-white text-black font-semibold shadow-sm"
                      : "text-[#D4D4D8] hover:text-white hover:bg-white/[0.06]"
                  }`}
                >
                  <span className="flex items-center gap-2.5">
                    <Icon className="w-3.5 h-3.5" />
                    {item.label}
                  </span>
                  <span
                    className={`text-[10px] font-mono-fx px-1.5 py-0.5 rounded-full ${
                      active ? "bg-black/15 text-black" : "bg-white/[0.06] text-[#A1A1AA]"
                    }`}
                  >
                    {item.count}
                  </span>
                </button>
              );
            })}
          </nav>

          {/* Section 2: Descobrir */}
          <nav className="space-y-1" aria-label="Descobrir">
            <div className="px-2.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717A]">
              Descobrir
            </div>
            <button
              type="button"
              onClick={() => {
                setHubTab("TEMPLATES");
                setHubQuery("");
              }}
              className={`w-full px-3 py-2 rounded-xl text-xs flex items-center justify-between transition ${
                hubTab === "TEMPLATES"
                  ? "bg-white text-black font-semibold shadow-sm"
                  : "text-[#D4D4D8] hover:text-white hover:bg-white/[0.06]"
              }`}
            >
              <span className="flex items-center gap-2.5">
                <Compass className="w-3.5 h-3.5" />
                Modelos Prontos
              </span>
              <span
                className={`text-[10px] font-mono-fx px-1.5 py-0.5 rounded-full ${
                  hubTab === "TEMPLATES" ? "bg-black/15 text-black" : "bg-white/[0.06] text-[#A1A1AA]"
                }`}
              >
                {totalTemplatesCount}
              </span>
            </button>
          </nav>

          {/* Section 3: Importação Rápida (Direct Actions, Not a Cluttered Tab) */}
          <div className="space-y-1 pt-2 border-t border-white/[0.08]">
            <div className="px-2.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717A]">
              Importar do PC
            </div>
            <button
              type="button"
              onClick={() => void importLocalModel3D(true)}
              disabled={busy !== null}
              className={`${LOCK_CLS} w-full px-3 py-2 rounded-xl text-xs text-[#A1A1AA] hover:text-white hover:bg-white/[0.06] transition flex items-center gap-2.5 text-left`}
            >
              <Box className="w-3.5 h-3.5 shrink-0" />
              <span className="truncate">Modelo 3D (.glb / .gltf)</span>
            </button>
            <button
              type="button"
              onClick={() => void importLocalVideoOrImage(true)}
              disabled={busy !== null}
              className={`${LOCK_CLS} w-full px-3 py-2 rounded-xl text-xs text-[#A1A1AA] hover:text-white hover:bg-white/[0.06] transition flex items-center gap-2.5 text-left`}
            >
              <Film className="w-3.5 h-3.5 shrink-0" />
              <span className="truncate">Vídeo ou Imagem</span>
            </button>
            <button
              type="button"
              onClick={() => void importPrismFromDisk()}
              disabled={busy !== null}
              className={`${LOCK_CLS} w-full px-3 py-2 rounded-xl text-xs text-[#A1A1AA] hover:text-white hover:bg-white/[0.06] transition flex items-center gap-2.5 text-left`}
            >
              {busy === "import" ? (
                <Loader2 className="w-3.5 h-3.5 shrink-0 animate-spin" />
              ) : (
                <Package className="w-3.5 h-3.5 shrink-0" />
              )}
              <span className="truncate">Pacote .prism</span>
            </button>
          </div>
        </div>

        {/* Sidebar Footer: Telemetry & Engine Status */}
        <div className="pt-3 border-t border-white/[0.08] space-y-2.5">
          <div className="px-2 flex items-center justify-between text-[11px] font-mono-fx text-[#A1A1AA]">
            <span>CPU {Math.round(telemetry.cpu_percent)}%</span>
            <span className="text-white/20">·</span>
            <span>RAM {Math.round(telemetry.ram_percent)}%</span>
          </div>
          {power.state !== "NORMAL" && (
            <div
              title={`Energia: ${POWER_PREFERENCE_LABELS[power.state]} — ${POWER_STATE_HINTS[power.state]}`}
              className="px-2.5 py-1 rounded-lg bg-amber-400/10 border border-amber-300/25 text-[10px] text-amber-100 text-center"
            >
              Energia: {POWER_PREFERENCE_LABELS[power.state]}
            </div>
          )}
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={togglePause}
              disabled={pauseBusy}
              title={
                runtime.paused
                  ? "Retomar animações do wallpaper e widgets no desktop"
                  : "Pausar animações do wallpaper no desktop sem removê-lo"
              }
              className={`${LOCK_CLS} flex-1 rounded-xl px-3 py-2 text-xs flex items-center justify-center gap-1.5 transition ${
                runtime.paused
                  ? "prism-pill-active"
                  : "frosted-chrome text-white hover:border-white/30"
              }`}
            >
              {runtime.paused ? <Play className="w-3 h-3" /> : <Pause className="w-3 h-3" />}
              {runtime.paused ? "Retomar" : "Pausar"}
            </button>
            <div ref={optionsRef} className="relative">
              <button
                type="button"
                onClick={() => setOptionsOpen((open) => !open)}
                title="Opções do PrismView e da Engine"
                aria-expanded={optionsOpen}
                className={`rounded-xl p-2 text-xs flex items-center justify-center transition ${
                  optionsOpen
                    ? "prism-pill-active"
                    : "frosted-chrome text-[#A1A1AA] hover:text-white hover:border-white/30"
                }`}
              >
                <Settings className="w-4 h-4" />
              </button>

              {optionsOpen && (
                <div className="absolute left-0 bottom-12 z-50 w-80 frosted-chrome frosted-chrome-panel rounded-2xl p-4 shadow-2xl border border-white/15 space-y-3.5">
                  <div className="flex items-center justify-between border-b border-white/10 pb-2">
                    <span className="text-xs font-semibold text-white">Opções do Aplicativo</span>
                    <button
                      type="button"
                      onClick={() => setOptionsOpen(false)}
                      aria-label="Fechar opções"
                      className="text-[#A1A1AA] hover:text-white p-0.5"
                    >
                      <X className="w-3.5 h-3.5" />
                    </button>
                  </div>

                  {/* Tray Visibility Toggle */}
                  <label className="flex items-start justify-between gap-3 cursor-pointer select-none">
                    <div className="space-y-0.5">
                      <div className="text-xs font-medium text-white">
                        Mostrar engine na bandeja
                      </div>
                      <p className="text-[11px] leading-snug text-[#A1A1AA]">
                        Mantém o ícone na gaveta do Windows enquanto o wallpaper roda em segundo plano.
                      </p>
                    </div>
                    <button
                      type="button"
                      role="switch"
                      aria-checked={engineTrayEnabled}
                      disabled={trayBusy}
                      onClick={() => void toggleEngineTray()}
                      className={`mt-0.5 shrink-0 w-9 h-5 rounded-full transition-colors relative ${
                        engineTrayEnabled ? "bg-white" : "bg-white/15"
                      } ${trayBusy ? "opacity-60" : ""}`}
                    >
                      <span
                        className={`block w-3.5 h-3.5 rounded-full transition-transform absolute top-[3px] ${
                          engineTrayEnabled
                            ? "translate-x-[19px] bg-black"
                            : "translate-x-[3px] bg-white/80"
                        }`}
                      />
                    </button>
                  </label>

                  {/* Autostart Toggle */}
                  <label className="flex items-start justify-between gap-3 cursor-pointer select-none pt-2 border-t border-white/10">
                    <div className="space-y-0.5">
                      <div className="text-xs font-medium text-white">
                        Iniciar com o Windows
                      </div>
                      <p className="text-[11px] leading-snug text-[#A1A1AA]">
                        Reaplica automaticamente o wallpaper ativo ao ligar o computador.
                      </p>
                    </div>
                    <button
                      type="button"
                      role="switch"
                      aria-checked={autostart}
                      disabled={autostartBusy}
                      onClick={() => void toggleAutostart()}
                      className={`mt-0.5 shrink-0 w-9 h-5 rounded-full transition-colors relative ${
                        autostart ? "bg-white" : "bg-white/15"
                      } ${autostartBusy ? "opacity-60" : ""}`}
                    >
                      <span
                        className={`block w-3.5 h-3.5 rounded-full transition-transform absolute top-[3px] ${
                          autostart
                            ? "translate-x-[19px] bg-black"
                            : "translate-x-[3px] bg-white/80"
                        }`}
                      />
                    </button>
                  </label>

                  {/* Power Preference */}
                  <div className="pt-2 border-t border-white/10 space-y-1.5">
                    <div className="flex items-center justify-between text-xs">
                      <span className="font-medium text-white">Modo de Energia</span>
                      <span className="text-[10px] font-mono-fx text-[#A1A1AA]">
                        {POWER_PREFERENCE_LABELS[power.state]}
                      </span>
                    </div>
                    <select
                      value={power.preference}
                      onChange={(e) => changePowerPreference(e.target.value as PowerPreference)}
                      className="w-full bg-black/70 border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                    >
                      {(["AUTO", "NORMAL", "ECO", "ULTRA", "FROZEN"] as PowerPreference[]).map((p) => (
                        <option key={p} value={p}>
                          {p === "AUTO"
                            ? `Automático (${POWER_PREFERENCE_LABELS[power.state]})`
                            : POWER_PREFERENCE_LABELS[p]}
                        </option>
                      ))}
                    </select>
                    <p className="text-[10px] text-[#71717A] leading-snug">
                      {POWER_STATE_HINTS[power.state]}
                    </p>
                  </div>

                  {/* Background Engine Shutdown */}
                  <div className="pt-2 border-t border-white/10 flex items-center justify-between gap-2">
                    <span className="text-[11px] text-[#A1A1AA]">Engine em segundo plano</span>
                    <button
                      type="button"
                      onClick={() => void stopBackgroundEngine()}
                      disabled={busy !== null}
                      className={`${LOCK_CLS} rounded-full px-3 py-1 text-[11px] border border-red-400/30 bg-red-500/10 text-red-200 hover:bg-red-500/20 hover:text-white transition`}
                    >
                      Desligar Engine
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        </div>
      </aside>

      {/* Main Content Area */}
      <div className="relative z-10 flex-1 flex flex-col min-w-0 overflow-hidden">
        {brokenProjects.length > 0 && (
          <div
            role="alert"
            className="mx-7 mt-4 rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-3 text-xs text-amber-100 shrink-0"
          >
            <div className="flex items-start gap-3">
              <span className="flex-1 leading-relaxed">
                {brokenProjects.length === 1
                  ? "1 projeto não pôde ser aberto"
                  : `${brokenProjects.length} projetos não puderam ser abertos`}
                {brokenProjects.some((b) => b.restored) ? " (alguns foram restaurados do backup)" : ""}.
              </span>
              <button
                type="button"
                onClick={() => setBrokenDetailsOpen((v) => !v)}
                aria-expanded={brokenDetailsOpen}
                className="shrink-0 underline text-amber-100/90 hover:text-white"
              >
                {brokenDetailsOpen ? "Ocultar detalhes" : "Ver detalhes"}
              </button>
              <button
                type="button"
                onClick={() => setBrokenProjects([])}
                aria-label="Dispensar aviso"
                className="shrink-0 text-amber-100/70 hover:text-white"
              >
                <X size={14} />
              </button>
            </div>
            {brokenDetailsOpen && (
              <ul className="mt-2 space-y-1 text-[11px] text-amber-100/80">
                {brokenProjects.map((b) => (
                  <li key={b.file} className="break-all">
                    <span className="font-mono">{b.file}</span> —{" "}
                    {b.restored ? "restaurado do backup" : "movido para projects/_corrompidos"}: {b.error}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {runtime.legacy_migration_notice && (
          <div
            role="alert"
            className="mx-7 mt-4 flex items-start gap-3 rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-3 text-xs text-amber-100 shrink-0"
          >
            <span className="flex-1 leading-relaxed">{runtime.legacy_migration_notice}</span>
            <button
              type="button"
              onClick={dismissLegacyNotice}
              aria-label="Dispensar aviso"
              className="shrink-0 text-amber-100/70 hover:text-white"
            >
              <X size={14} />
            </button>
          </div>
        )}

        {/* Contextual Top Bar */}
        <header className="px-7 pt-5 pb-3 border-b border-white/[0.06] flex flex-wrap items-center justify-between gap-3 shrink-0">
          <div>
            <h1 className="font-display text-lg font-semibold text-white">
              {hubTab === "WALLPAPERS"
                ? "Meus Wallpapers"
                : hubTab === "WIDGETS"
                  ? "Meus Widgets"
                  : "Modelos Prontos"}
            </h1>
            <p className="text-xs text-[#A1A1AA]">
              {hubTab === "WALLPAPERS"
                ? "Sua biblioteca de wallpapers 2D, vídeo e 3D com pausa automática em tela cheia."
                : hubTab === "WIDGETS"
                  ? "Fixe sobre qualquer papel de parede ou importe dentro de um wallpaper no Estúdio."
                  : "Comece rápido com composições 2D, vidro óptico, widgets ou cenas 3D interativas."}
            </p>
          </div>

          <div className="flex items-center gap-2.5">
            {/* Search Input */}
            <div className="relative">
              <Search className="w-3.5 h-3.5 text-[#71717A] absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none" />
              <input
                type="text"
                value={hubQuery}
                onChange={(e) => setHubQuery(e.target.value)}
                placeholder={
                  hubTab === "WALLPAPERS"
                    ? "Buscar wallpapers…"
                    : hubTab === "WIDGETS"
                      ? "Buscar widgets…"
                      : "Buscar modelos…"
                }
                className="w-52 frosted-chrome rounded-full pl-8 pr-7 py-1.5 text-xs text-white placeholder:text-[#71717A] outline-none focus:border-white/30"
              />
              {hubQuery && (
                <button
                  type="button"
                  onClick={() => setHubQuery("")}
                  aria-label="Limpar busca"
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-[#71717A] hover:text-white"
                >
                  <X className="w-3 h-3" />
                </button>
              )}
            </div>

            {hubTab === "WIDGETS" && (
              <button
                type="button"
                onClick={() => void createNew("WIDGET")}
                className="prismatic-cta px-4 py-1.5 text-xs flex items-center gap-1.5"
              >
                <Plus className="w-3.5 h-3.5" /> Novo Widget
              </button>
            )}
            {hubTab === "TEMPLATES" && (
              <button
                type="button"
                onClick={() => void createNew3DWallpaper()}
                className="prism-pill-idle rounded-full px-3.5 py-1.5 text-xs flex items-center gap-1.5"
              >
                <Box className="w-3.5 h-3.5" /> Cena 3D em Branco
              </button>
            )}
          </div>
        </header>

        {/* Scrollable Workspace Body */}
        <main className="flex-1 overflow-y-auto px-7 py-5">
          {hubTab === "WALLPAPERS" && (
            <div className="max-w-7xl mx-auto space-y-5">
              {/* Filter Chips + Multi-Monitor Selector */}
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="flex flex-wrap items-center gap-1.5">
                  {(
                    [
                      { id: "ALL" as WallpaperKindFilter, label: "Todos", count: wallpaperCounts.ALL },
                      { id: "3D" as WallpaperKindFilter, label: "3D", count: wallpaperCounts["3D"] },
                      { id: "VIDEO" as WallpaperKindFilter, label: "Vídeo", count: wallpaperCounts.VIDEO },
                      { id: "2D" as WallpaperKindFilter, label: "2D & Vidro", count: wallpaperCounts["2D"] },
                    ]
                  ).map((f) => (
                    <button
                      key={f.id}
                      type="button"
                      onClick={() => setWallpaperFilter(f.id)}
                      className={`px-3 py-1.5 rounded-full text-[11px] flex items-center gap-1.5 transition ${
                        wallpaperFilter === f.id ? "prism-pill-active" : "prism-pill-idle"
                      }`}
                    >
                      <span>{f.label}</span>
                      <span className="font-mono-fx opacity-70">{f.count}</span>
                    </button>
                  ))}
                </div>

              </div>

              {filteredWallpapers.length === 0 && (
                <EmptyState
                  icon={<Monitor className="w-4 h-4 text-white" />}
                  title={
                    wallpapers.length === 0
                      ? "Nenhum wallpaper na sua biblioteca"
                      : "Nenhum wallpaper corresponde ao filtro"
                  }
                  hint={
                    wallpapers.length === 0
                      ? "Crie um novo projeto, explore os modelos prontos ou importe um arquivo do PC."
                      : "Limpe a busca ou troque o filtro acima para ver seus outros wallpapers."
                  }
                  actionLabel={wallpapers.length === 0 ? "Novo Projeto" : "Mostrar Todos"}
                  disabled={busy !== null}
                  onAction={() => {
                    if (wallpapers.length === 0) setCreateModalOpen(true);
                    else {
                      setWallpaperFilter("ALL");
                      setHubQuery("");
                    }
                  }}
                />
              )}

              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
                {filteredWallpapers.map((wp) => {
                  const isActive = runtime.active_wallpaper_id === wp.id;
                  const wpKind = classifyWallpaperKind(wp);
                  return (
                    <article key={wp.id} className="prism-hub-card flex flex-col">
                      <div
                        onClick={() => setEditing(wp)}
                        className="relative aspect-video bg-black cursor-pointer overflow-hidden"
                      >
                        <HubThumbnail project={wp} />
                        <div className="absolute top-2.5 left-2.5 right-2.5 flex items-center justify-between gap-2 pointer-events-none">
                          {isActive ? (
                            <span className="px-2.5 py-1 rounded-full prism-pill-active text-[10px]">
                              Ativo no Desktop
                            </span>
                          ) : (
                            <span />
                          )}
                          <span className="px-2 py-0.5 rounded-full bg-black/75 border border-white/15 text-[10px] font-mono-fx text-white/85">
                            {wpKind}
                          </span>
                        </div>
                      </div>
                      <div className="p-4 flex flex-col gap-3">
                        <div className="flex items-center justify-between gap-2">
                          <div className="min-w-0">
                            <h3 className="text-sm font-semibold text-white truncate">{wp.title}</h3>
                            <span className="text-[11px] text-[#A1A1AA]">
                              {wp.layers.length} {wp.layers.length === 1 ? "camada" : "camadas"}
                            </span>
                          </div>
                          <div className="flex items-center gap-1">
                            <button
                              type="button"
                              onClick={() => exportProject(wp)}
                              disabled={busy !== null}
                              title="Exportar .prism"
                              className={`${LOCK_CLS} p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10`}
                            >
                              {busy === `export:${wp.id}` ? (
                                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                              ) : (
                                <Download className="w-3.5 h-3.5" />
                              )}
                            </button>
                            {wallpapers.length > 1 && (
                              <button
                                type="button"
                                onClick={() => setPendingDelete(wp)}
                                disabled={busy !== null}
                                title="Excluir"
                                className={`${LOCK_CLS} p-1.5 rounded-full text-[#A1A1AA] hover:text-red-300 hover:bg-white/10`}
                              >
                                <Trash2 className="w-3.5 h-3.5" />
                              </button>
                            )}
                          </div>
                        </div>
                        <div className="grid grid-cols-2 gap-2">
                          {isActive ? (
                            <button
                              type="button"
                              onClick={() => void clearWallpaper()}
                              disabled={busy !== null}
                              className={`${LOCK_CLS} prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5`}
                            >
                              {busy === "clear" ? (
                                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                              ) : (
                                <X className="w-3.5 h-3.5" />
                              )}{" "}
                              {busy === "clear" ? "Removendo…" : "Remover"}
                            </button>
                          ) : (
                            <button
                              type="button"
                              onClick={() => applyWallpaper(wp)}
                              disabled={busy !== null}
                              aria-busy={busy === `apply:${wp.id}`}
                              className={`${LOCK_CLS} prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5`}
                            >
                              {busy === `apply:${wp.id}` ? (
                                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                              ) : (
                                <Sparkles className="w-3.5 h-3.5" />
                              )}{" "}
                              {busy === `apply:${wp.id}` ? "Aplicando…" : "Aplicar"}
                            </button>
                          )}
                          <button
                            type="button"
                            onClick={() => setEditing(wp)}
                            className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                          >
                            <Sliders className="w-3.5 h-3.5" /> Estúdio
                          </button>
                        </div>
                      </div>
                    </article>
                  );
                })}
              </div>
            </div>
          )}

          {hubTab === "WIDGETS" && (
            <div className="max-w-7xl mx-auto space-y-5">
              {runtime.pinned_widget_ids.length >= 3 && (
                <div className="rounded-xl border border-amber-400/30 bg-amber-400/[0.06] px-3.5 py-2.5 text-[11px] text-amber-200/90">
                  Cada widget fixado roda em uma janela WebView própria (dezenas de MB de RAM cada).{" "}
                  Você tem {runtime.pinned_widget_ids.length} fixados — considere agrupar widgets dentro de um Wallpaper.
                </div>
              )}

              {filteredWidgets.length === 0 && (
                <EmptyState
                  icon={<Layers className="w-4 h-4 text-white" />}
                  title={widgets.length === 0 ? "Nenhum widget na biblioteca" : "Nenhum widget encontrado"}
                  hint={
                    widgets.length === 0
                      ? "Crie um widget do zero, explore os modelos prontos ou importe um .widget.prism."
                      : "Limpe a busca para visualizar seus widgets."
                  }
                  actionLabel={widgets.length === 0 ? "Novo Widget" : "Limpar Busca"}
                  disabled={busy !== null}
                  onAction={() => {
                    if (widgets.length === 0) void createNew("WIDGET");
                    else setHubQuery("");
                  }}
                />
              )}

              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
                {filteredWidgets.map((w) => {
                  const pinned = runtime.pinned_widget_ids.includes(w.id);
                  const isLocked = runtime.locked_widget_ids.includes(w.id);
                  return (
                    <article key={w.id} className="prism-hub-card flex flex-col">
                      <div
                        onClick={() => setEditing(w)}
                        className="relative aspect-[16/10] flex items-center justify-center p-4 cursor-pointer overflow-hidden bg-[#0D0D0F]"
                        style={{
                          backgroundImage:
                            "radial-gradient(rgba(255,255,255,0.08) 1px, transparent 1px)",
                          backgroundSize: "16px 16px",
                        }}
                      >
                        <div
                          className="w-full max-w-[340px]"
                          style={{ aspectRatio: `${w.widget.width} / ${w.widget.height}` }}
                        >
                          <HubThumbnail project={w} />
                        </div>
                        {pinned && (
                          <span className="absolute top-2.5 left-2.5 px-2.5 py-1 rounded-full prism-pill-active text-[10px]">
                            {isLocked ? "Fixado · Travado" : "Fixado na Tela"}
                          </span>
                        )}
                      </div>
                      <div className="p-4 flex flex-col gap-3">
                        <div className="flex items-center justify-between gap-2">
                          <div className="min-w-0">
                            <h3 className="text-sm font-semibold text-white truncate">{w.title}</h3>
                            <span className="text-[11px] font-mono-fx text-[#A1A1AA]">
                              {w.widget.width}×{w.widget.height}px · {w.layers.length} camadas
                            </span>
                          </div>
                          <div className="flex items-center gap-1">
                            {pinned && isLocked && (
                              <button
                                type="button"
                                onClick={async () => {
                                  const rt = await invokeCmd<RuntimeState>("set_widget_locked", {
                                    widgetId: w.id,
                                    locked: false,
                                  }).catch(() => null);
                                  if (rt) setRuntime(rt);
                                  notify(`Widget "${w.title}" destravado`);
                                }}
                                disabled={busy !== null}
                                title="Destravar posição"
                                className={`${LOCK_CLS} px-2.5 py-1 rounded-full prism-pill-idle text-[10px] flex items-center gap-1`}
                              >
                                <Unlock className="w-3 h-3" /> Destravar
                              </button>
                            )}
                            <button
                              type="button"
                              onClick={() => exportProject(w)}
                              disabled={busy !== null}
                              title="Exportar .widget.prism"
                              className={`${LOCK_CLS} p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10`}
                            >
                              {busy === `export:${w.id}` ? (
                                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                              ) : (
                                <Download className="w-3.5 h-3.5" />
                              )}
                            </button>
                            {widgets.length > 1 && (
                              <button
                                type="button"
                                onClick={() => setPendingDelete(w)}
                                disabled={busy !== null}
                                title="Excluir widget"
                                className={`${LOCK_CLS} p-1.5 rounded-full text-[#A1A1AA] hover:text-red-300 hover:bg-white/10`}
                              >
                                <Trash2 className="w-3.5 h-3.5" />
                              </button>
                            )}
                          </div>
                        </div>
                        <ExposedGlobalsPanel
                          project={w}
                          onImmediateChange={(nextW) => {
                            void persistProjectImmediate(nextW);
                            if (pinned) void pinWidget(nextW, true);
                          }}
                          onDebouncedChange={persistProject}
                          onCommitChange={(nextW) => {
                            void flushPendingSave(pinned);
                            if (pinned) void pinWidget(nextW, true);
                          }}
                        />
                        <div className="grid grid-cols-2 gap-2">
                          {pinned ? (
                            <button
                              type="button"
                              onClick={() => unpinWidget(w.id)}
                              disabled={busy !== null}
                              className={`${LOCK_CLS} prism-pill-idle rounded-full py-2 text-xs`}
                            >
                              {busy === `unpin:${w.id}` ? "Removendo…" : "Remover da Tela"}
                            </button>
                          ) : (
                            <button
                              type="button"
                              onClick={() => pinWidget(w)}
                              disabled={busy !== null}
                              aria-busy={busy === `pin:${w.id}`}
                              className={`${LOCK_CLS} prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5`}
                            >
                              {busy === `pin:${w.id}` ? (
                                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                              ) : (
                                <Sparkles className="w-3.5 h-3.5" />
                              )}{" "}
                              {busy === `pin:${w.id}` ? "Fixando…" : "Fixar na Tela"}
                            </button>
                          )}
                          <button
                            type="button"
                            onClick={() => setEditing(w)}
                            className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                          >
                            <Sliders className="w-3.5 h-3.5" /> Estúdio
                          </button>
                        </div>
                      </div>
                    </article>
                  );
                })}
              </div>
            </div>
          )}

          {hubTab === "TEMPLATES" && (
            <div className="max-w-7xl mx-auto space-y-6 pb-6">
              {/* Category Filter Bar + Restore Defaults */}
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="flex flex-wrap items-center gap-1.5">
                  {(
                    [
                      { id: "ALL" as TemplateCategoryFilter, label: "Todos" },
                      { id: "3D" as TemplateCategoryFilter, label: "3D Interativo" },
                      { id: "2D" as TemplateCategoryFilter, label: "2D & Vidro" },
                      { id: "WIDGET" as TemplateCategoryFilter, label: "Widgets" },
                    ]
                  ).map((cat) => (
                    <button
                      key={cat.id}
                      type="button"
                      onClick={() => setTemplateFilter(cat.id)}
                      className={`px-3.5 py-1.5 rounded-full text-[11px] transition ${
                        templateFilter === cat.id ? "prism-pill-active" : "prism-pill-idle"
                      }`}
                    >
                      {cat.label}
                    </button>
                  ))}
                </div>
                <button
                  type="button"
                  onClick={() => {
                    void addMissingDefaultPresets("WALLPAPER");
                    void addMissingDefaultPresets("WIDGET");
                  }}
                  className="prism-pill-idle rounded-full px-3.5 py-1.5 text-[11px] flex items-center gap-1.5"
                >
                  <Sparkles className="w-3 h-3" /> Restaurar Padrões na Biblioteca
                </button>
              </div>

              {/* Unified Templates Grid */}
              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
                {unifiedTemplates.map(({ project: tpl, category, subtitle, badges }) => {
                  const isWidgetTpl = tpl.packageType === "WIDGET";
                  const openOrCloneTemplate = async (applyDirectly: boolean) => {
                    const existing = projects.find((p) => p.id === tpl.id);
                    const target: PrismProject = existing
                      ? existing
                      : (JSON.parse(JSON.stringify(tpl)) as PrismProject);
                    if (!existing) {
                      await persistProjectImmediate(target);
                    }
                    if (applyDirectly) {
                      if (isWidgetTpl) await pinWidget(target);
                      else await applyWallpaper(target);
                    } else {
                      setEditing(target);
                    }
                  };
                  return (
                    <article
                      key={tpl.id}
                      className="prism-hub-card overflow-hidden flex flex-col justify-between"
                    >
                      <div>
                        <div
                          onClick={() => void openOrCloneTemplate(false)}
                          className="relative aspect-video bg-[#08080A] overflow-hidden border-b border-white/10 cursor-pointer flex items-center justify-center"
                        >
                          {isWidgetTpl ? (
                            <div
                              className="w-full max-w-[290px] p-3"
                              style={{ aspectRatio: `${tpl.widget.width} / ${tpl.widget.height}` }}
                            >
                              <HubThumbnail project={tpl} />
                            </div>
                          ) : (
                            <HubThumbnail project={tpl} />
                          )}
                          <span className="absolute top-2.5 left-2.5 px-2.5 py-0.5 rounded-full bg-black/80 border border-white/15 text-[10px] font-mono-fx uppercase tracking-wider text-white">
                            {category === "3D"
                              ? "3D Interativo"
                              : category === "WIDGET"
                                ? "Widget"
                                : "2D & Vidro"}
                          </span>
                        </div>
                        <div className="p-4 space-y-2">
                          <h3 className="text-sm font-semibold text-white">{tpl.title}</h3>
                          <p className="text-xs text-[#A1A1AA] leading-relaxed">{subtitle}</p>
                          <div className="flex flex-wrap gap-1.5 pt-1">
                            {badges.map((b) => (
                              <span
                                key={b}
                                className="px-2 py-0.5 rounded-full bg-white/5 border border-white/10 text-[10px] text-white/80"
                              >
                                {b}
                              </span>
                            ))}
                          </div>
                        </div>
                      </div>
                      <div className="px-4 pb-4 grid grid-cols-2 gap-2">
                        <button
                          type="button"
                          onClick={() => void openOrCloneTemplate(true)}
                          disabled={busy !== null}
                          className={`${LOCK_CLS} prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5`}
                        >
                          {busy === `apply:${tpl.id}` || busy === `pin:${tpl.id}` ? (
                            <Loader2 className="w-3.5 h-3.5 animate-spin" />
                          ) : (
                            <Sparkles className="w-3.5 h-3.5" />
                          )}{" "}
                          {isWidgetTpl ? "Fixar" : "Aplicar"}
                        </button>
                        <button
                          type="button"
                          onClick={() => void openOrCloneTemplate(false)}
                          className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                        >
                          <Sliders className="w-3.5 h-3.5" /> Personalizar
                        </button>
                      </div>
                    </article>
                  );
                })}
              </div>

              {/* Collapsible Blender Optimization Guide (Out of the way unless needed) */}
              <div className="frosted-chrome frosted-chrome-panel rounded-2xl overflow-hidden">
                <button
                  type="button"
                  onClick={() => setBlenderGuideOpen((v) => !v)}
                  className="w-full px-5 py-3.5 flex items-center justify-between text-left hover:bg-white/[0.03] transition"
                >
                  <div className="flex items-center gap-2.5">
                    {blenderGuideOpen ? (
                      <ChevronDown className="w-4 h-4 text-[#A1A1AA]" />
                    ) : (
                      <ChevronRight className="w-4 h-4 text-[#A1A1AA]" />
                    )}
                    <span className="text-xs font-semibold text-white">
                      Guia Rápido: Exportando Modelos do Blender para o PrismView
                    </span>
                  </div>
                  <span className="text-[10px] font-mono-fx text-[#A1A1AA]">
                    Meta: ≤ 1% CPU · GPU Ultra-Leve
                  </span>
                </button>
                {blenderGuideOpen && (
                  <div className="px-5 pb-5 pt-1 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3 text-xs border-t border-white/10">
                    <div className="rounded-xl bg-white/[0.03] border border-white/10 p-3 space-y-1">
                      <span className="font-mono-fx text-[10px] text-white/60">01 · FORMATO</span>
                      <p className="font-medium text-white">Exporte como glTF Binary (.glb)</p>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Prefira <strong>.glb padrão</strong> (ou Meshopt) para compatibilidade máxima com o motor nativo.
                      </p>
                    </div>
                    <div className="rounded-xl bg-white/[0.03] border border-white/10 p-3 space-y-1">
                      <span className="font-mono-fx text-[10px] text-white/60">02 · GEOMETRIA</span>
                      <p className="font-medium text-white">Mantenha ≤ 60k triângulos</p>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Use o modificador <strong>Decimate</strong> e Normal Maps no lugar de subdivisão pesada.
                      </p>
                    </div>
                    <div className="rounded-xl bg-white/[0.03] border border-white/10 p-3 space-y-1">
                      <span className="font-mono-fx text-[10px] text-white/60">03 · TEXTURAS</span>
                      <p className="font-medium text-white">PNG / WebP / KTX2 até 2048px</p>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Evite texturas 4K/8K em wallpaper; o auditor interno avisa automaticamente se a VRAM subir.
                      </p>
                    </div>
                    <div className="rounded-xl bg-white/[0.03] border border-white/10 p-3 space-y-1">
                      <span className="font-mono-fx text-[10px] text-white/60">04 · INTERAÇÃO</span>
                      <p className="font-medium text-white">Nomeie Ossos e Materiais</p>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Ex.: <code className="font-mono-fx text-white">Head</code> para Look-At,{" "}
                        <code className="font-mono-fx text-white">Screen</code> para capa de música ao vivo.
                      </p>
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}
        </main>
      </div>

      {/* Unified Create / Import Modal */}
      <AnimatePresence>
        {createModalOpen && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[105] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
            onClick={() => setCreateModalOpen(false)}
          >
            <motion.div
              role="dialog"
              aria-modal="true"
              aria-labelledby="create-modal-title"
              initial={{ opacity: 0, y: 10, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 6, scale: 0.97 }}
              transition={SPRING_TACTILE}
              onClick={(e) => e.stopPropagation()}
              className="w-full max-w-2xl frosted-chrome frosted-chrome-panel rounded-3xl p-6 border border-white/15 shadow-2xl space-y-5"
            >
              <div className="flex items-center justify-between">
                <div>
                  <h2 id="create-modal-title" className="font-display text-base font-semibold text-white">
                    Novo Projeto ou Importação
                  </h2>
                  <p className="text-xs text-[#A1A1AA]">
                    Escolha começar do zero ou trazer um arquivo 3D, vídeo, imagem ou pacote do seu PC.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => setCreateModalOpen(false)}
                  aria-label="Fechar"
                  className="p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>

              {/* Section: Criar do Zero */}
              <div className="space-y-2">
                <span className="text-[10px] font-semibold uppercase tracking-wider text-[#71717A]">
                  Criar do Zero
                </span>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                  <button
                    type="button"
                    onClick={() => {
                      setCreateModalOpen(false);
                      void createNew("WALLPAPER");
                    }}
                    className="rounded-2xl border border-white/10 bg-white/[0.03] hover:bg-white/[0.08] hover:border-white/25 p-4 text-left transition space-y-2"
                  >
                    <Monitor className="w-5 h-5 text-white" />
                    <div>
                      <div className="text-xs font-semibold text-white">Wallpaper 2D / Vidro</div>
                      <p className="text-[11px] text-[#A1A1AA] mt-0.5">
                        Camadas de vidro óptico, relógio e telemetria.
                      </p>
                    </div>
                  </button>

                  <button
                    type="button"
                    onClick={() => {
                      setCreateModalOpen(false);
                      void createNew3DWallpaper();
                    }}
                    className="rounded-2xl border border-white/10 bg-white/[0.03] hover:bg-white/[0.08] hover:border-white/25 p-4 text-left transition space-y-2"
                  >
                    <Box className="w-5 h-5 text-white" />
                    <div>
                      <div className="text-xs font-semibold text-white">Cena 3D Interativa</div>
                      <p className="text-[11px] text-[#A1A1AA] mt-0.5">
                        Sala 3D pronta para esculturas e modelos .glb.
                      </p>
                    </div>
                  </button>

                  <button
                    type="button"
                    onClick={() => {
                      setCreateModalOpen(false);
                      void createNew("WIDGET");
                    }}
                    className="rounded-2xl border border-white/10 bg-white/[0.03] hover:bg-white/[0.08] hover:border-white/25 p-4 text-left transition space-y-2"
                  >
                    <Layers className="w-5 h-5 text-white" />
                    <div>
                      <div className="text-xs font-semibold text-white">Widget Independente</div>
                      <p className="text-[11px] text-[#A1A1AA] mt-0.5">
                        Fixe sobre qualquer papel de parede do Windows.
                      </p>
                    </div>
                  </button>
                </div>
              </div>

              {/* Section: Importar do Computador */}
              <div className="space-y-2">
                <span className="text-[10px] font-semibold uppercase tracking-wider text-[#71717A]">
                  Importar do Computador
                </span>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                  <div className="rounded-2xl border border-white/10 bg-white/[0.03] p-4 flex flex-col justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2 text-xs font-semibold text-white">
                        <Box className="w-4 h-4" /> Modelo 3D (.glb)
                      </div>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Modelos e animações do Blender.
                      </p>
                    </div>
                    <div className="grid grid-cols-2 gap-1.5">
                      <button
                        type="button"
                        onClick={() => {
                          setCreateModalOpen(false);
                          void importLocalModel3D(true);
                        }}
                        className="prism-pill-active rounded-full py-1.5 text-[11px] flex items-center justify-center gap-1"
                      >
                        <Wand2 className="w-3 h-3" /> Estúdio
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setCreateModalOpen(false);
                          void importLocalModel3D(false);
                        }}
                        className="prism-pill-idle rounded-full py-1.5 text-[11px]"
                      >
                        Aplicar
                      </button>
                    </div>
                  </div>

                  <div className="rounded-2xl border border-white/10 bg-white/[0.03] p-4 flex flex-col justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2 text-xs font-semibold text-white">
                        <Film className="w-4 h-4" /> Vídeo / Imagem
                      </div>
                      <p className="text-[11px] text-[#A1A1AA]">
                        MP4, WebM, MKV, PNG ou JPG local.
                      </p>
                    </div>
                    <div className="grid grid-cols-2 gap-1.5">
                      <button
                        type="button"
                        onClick={() => {
                          setCreateModalOpen(false);
                          void importLocalVideoOrImage(true);
                        }}
                        className="prism-pill-active rounded-full py-1.5 text-[11px] flex items-center justify-center gap-1"
                      >
                        <Wand2 className="w-3 h-3" /> Estúdio
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setCreateModalOpen(false);
                          void importLocalVideoOrImage(false);
                        }}
                        className="prism-pill-idle rounded-full py-1.5 text-[11px]"
                      >
                        Aplicar
                      </button>
                    </div>
                  </div>

                  <div className="rounded-2xl border border-white/10 bg-white/[0.03] p-4 flex flex-col justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2 text-xs font-semibold text-white">
                        <Package className="w-4 h-4" /> Pacote .prism
                      </div>
                      <p className="text-[11px] text-[#A1A1AA]">
                        Tema ou widget completo exportado.
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => {
                        setCreateModalOpen(false);
                        void importPrismFromDisk();
                      }}
                      className="prism-pill-idle rounded-full py-1.5 text-[11px] w-full flex items-center justify-center gap-1"
                    >
                      <Upload className="w-3 h-3" /> Escolher Arquivo
                    </button>
                  </div>
                </div>
              </div>

              <div className="pt-2 border-t border-white/10 flex items-center justify-between text-xs">
                <span className="text-[#A1A1AA]">Quer começar a partir de um visual pronto?</span>
                <button
                  type="button"
                  onClick={() => {
                    setCreateModalOpen(false);
                    setHubTab("TEMPLATES");
                  }}
                  className="text-white font-medium hover:underline flex items-center gap-1"
                >
                  Explorar Modelos Prontos <ChevronRight className="w-3.5 h-3.5" />
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>

      <ConfirmDialog
        open={pendingDelete !== null}
        title={pendingDelete?.packageType === "WIDGET" ? "Excluir widget?" : "Excluir wallpaper?"}
        message={deleteDialogMessage}
        confirmLabel="Excluir"
        busy={pendingDelete !== null && busy === `delete:${pendingDelete.id}`}
        onConfirm={() => void confirmDelete()}
        onCancel={() => setPendingDelete(null)}
      />
      <ConfirmDialog
        open={importConflict !== null}
        title="Este projeto já existe"
        message={`"${importConflict?.existingTitle || "Projeto"}" já está na sua biblioteca. Substituir apaga as edições locais dele; "Manter os dois" importa uma cópia.`}
        confirmLabel="Substituir"
        cancelLabel="Manter os dois"
        busyLabel="Importando…"
        busy={busy === "import"}
        onConfirm={() => void resolveImportConflict(true)}
        onCancel={() => void resolveImportConflict(false)}
        onDismiss={() => setImportConflict(null)}
      />
      <OpenAppReviewDialog
        items={openAppReview?.items ?? null}
        onConfirm={(allowed) => void confirmOpenAppReview(allowed)}
      />
      <ToastOverlay message={toast} />
    </div>
  );
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <StudioHubApp />
  </React.StrictMode>
);
