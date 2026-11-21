import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { AnimatePresence, motion } from "framer-motion";
import {
  Box,
  Download,
  Film,
  Layers,
  Lock,
  Monitor,
  Move,
  Package,
  Pause,
  Play,
  Plus,
  Power,
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
  normalizeProjectScene3D,
  PickedFileDto,
  PrismProject,
  RuntimeState,
  SceneLayer,
  SPRING_TACTILE,
} from "./types/prism";
import { UnifiedCompositor } from "./components/UnifiedCompositor";
import { StudioEditor } from "./components/StudioEditor";
import { useDesktopWindowBridge } from "./hooks/useDesktopWindowBridge";
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
type HubTab = "WALLPAPERS" | "WIDGETS" | "MEDIA_3D";

const StudioHubApp: React.FC = () => {
  const telemetry = useLiveTelemetry();
  const power = usePowerState(telemetry);
  const [hubTab, setHubTab] = useState<HubTab>("WALLPAPERS");
  const [projects, setProjects] = useState<PrismProject[]>([]);
  const [editing, setEditing] = useState<PrismProject | null>(null);
  const [runtime, setRuntime] = useState<RuntimeState>({
    paused: false,
    desktop_visible: true,
    active_wallpaper_id: null,
    pinned_widget_ids: [],
    locked_widget_ids: [],
    monitors: [],
    monitor_wallpapers: {},
  });
  const [monitors, setMonitors] = useState<MonitorInfoDto[]>([]);
  const [targetMonitorId, setTargetMonitorId] = useState<string | null>(null);
  const [autostart, setAutostart] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const pendingSaveRef = useRef<PrismProject | null>(null);
  const applyingRef = useRef(false);

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
    };
  }, []);

  const loadAll = useCallback(async () => {
    try {
      const saved = await invokeCmd<PrismProject[]>("list_projects");
      if (saved.length > 0) {
        // Border migration: legacy 3D configs (30/60/120 FPS, single modelSrc) → schema 2
        setProjects(saved.map((p) => normalizeProjectScene3D(p)));
      } else {
        const defaults = createDefaultProjects();
        setProjects(defaults);
        for (const p of defaults) {
          await invokeCmd("save_project", { project: p }).catch(() => {});
        }
      }
    } catch {
      setProjects(createDefaultProjects());
    }

    try {
      const rt = await invokeCmd<RuntimeState>("get_runtime_state");
      setRuntime(rt);
      if (rt.monitors?.length) {
        setMonitors(rt.monitors);
      } else {
        const mons = await invokeCmd<MonitorInfoDto[]>("list_monitors").catch(() => []);
        if (mons.length) setMonitors(mons);
      }
    } catch {
      /* browser mode */
    }

    try {
      const auto = await invokeCmd<boolean>("get_autostart_enabled");
      setAutostart(Boolean(auto));
    } catch {
      /* browser mode */
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unsub: (() => void) | undefined;
    void import("@tauri-apps/api/event").then(async ({ listen }) => {
      const fn = await listen<MonitorInfoDto[]>("prism://monitors-changed", (ev) => {
        if (disposed) return;
        setMonitors(ev.payload);
        setTargetMonitorId((cur) =>
          cur && !ev.payload.some((m) => m.id === cur) ? null : cur
        );
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

  const runtimeRef = useRef(runtime);
  runtimeRef.current = runtime;

  const syncIfActiveOnDesktop = useCallback(async (proj: PrismProject) => {
    if (!isTauriRuntime()) return;
    const rt = runtimeRef.current;
    if (proj.packageType === "WIDGET") {
      if (rt.pinned_widget_ids.includes(proj.id)) {
        const nextRt = await invokeCmd<RuntimeState>("pin_standalone_widget", {
          project: proj,
        }).catch(() => null);
        if (nextRt) setRuntime(nextRt);
      }
      return;
    }
    const matchingMonitors = Object.entries(rt.monitor_wallpapers ?? {})
      .filter(([, pid]) => pid === proj.id)
      .map(([mid]) => mid);
    if (matchingMonitors.length > 0) {
      for (const mid of matchingMonitors) {
        const nextRt = await invokeCmd<RuntimeState>("apply_desktop_wallpaper", {
          project: proj,
          monitorId: mid,
        }).catch(() => null);
        if (nextRt) setRuntime(nextRt);
      }
    } else if (rt.active_wallpaper_id === proj.id) {
      const nextRt = await invokeCmd<RuntimeState>("apply_desktop_wallpaper", {
        project: proj,
        monitorId: null,
      }).catch(() => null);
      if (nextRt) setRuntime(nextRt);
    }
  }, []);

  const flushPendingSave = useCallback(
    async (skipDesktopSync = false) => {
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      const pending = pendingSaveRef.current;
      if (!pending) return;
      pendingSaveRef.current = null;
      await invokeCmd("save_project", { project: pending }).catch(() => {});
      if (!skipDesktopSync) {
        await syncIfActiveOnDesktop(pending);
      }
    },
    [syncIfActiveOnDesktop]
  );

  useEffect(() => {
    const onUnload = () => {
      void flushPendingSave();
    };
    window.addEventListener("beforeunload", onUnload);
    return () => {
      window.removeEventListener("beforeunload", onUnload);
      void flushPendingSave();
    };
  }, [flushPendingSave]);

  const persistProject = useCallback(
    (next: PrismProject) => {
      setEditing((cur) => (cur?.id === next.id ? next : cur));
      setProjects((prev) =>
        prev.some((p) => p.id === next.id)
          ? prev.map((p) => (p.id === next.id ? next : p))
          : [...prev, next]
      );
      if (pendingSaveRef.current && pendingSaveRef.current.id !== next.id) {
        const previous = pendingSaveRef.current;
        pendingSaveRef.current = null;
        void invokeCmd("save_project", { project: previous }).catch(() => {});
      }
      pendingSaveRef.current = next;
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
      }
      saveTimerRef.current = window.setTimeout(() => {
        saveTimerRef.current = null;
        void flushPendingSave();
      }, 400);
    },
    [flushPendingSave]
  );

  const persistProjectImmediate = useCallback(async (next: PrismProject) => {
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
    await invokeCmd("save_project", { project: next }).catch(() => {});
  }, []);

  const applyWallpaper = async (proj: PrismProject, silent = false) => {
    if (applyingRef.current) return;
    applyingRef.current = true;
    try {
      const rt = await invokeCmd<RuntimeState>("apply_desktop_wallpaper", {
        project: proj,
        monitorId: targetMonitorId,
      });
      setRuntime(rt);
      if (rt.monitors?.length) setMonitors(rt.monitors);
      if (!silent) {
        const targetLabel = targetMonitorId
          ? ` no monitor ${targetMonitorId}`
          : monitors.length > 1
          ? ` em ${monitors.length} monitores`
          : "";
        notify(`Wallpaper "${proj.title}" aplicado${targetLabel}`);
      }
    } catch {
      if (!silent) notify("Disponível no app desktop");
    } finally {
      applyingRef.current = false;
    }
  };

  const clearWallpaper = async (monitorId: string | null = null) => {
    try {
      const rt = await invokeCmd<RuntimeState>("clear_desktop_wallpaper", { monitorId });
      setRuntime(rt);
      notify("Wallpaper removido do desktop");
    } catch {
      /* ignore */
    }
  };

  const pinWidget = async (proj: PrismProject, silent = false) => {
    if (applyingRef.current) return;
    applyingRef.current = true;
    try {
      const rt = await invokeCmd<RuntimeState>("pin_standalone_widget", { project: proj });
      setRuntime(rt);
      if (!silent) notify(`Widget "${proj.title}" fixado na tela`);
    } catch {
      if (!silent) notify("Disponível no app desktop");
    } finally {
      applyingRef.current = false;
    }
  };

  const unpinWidget = async (widgetId: string) => {
    try {
      const rt = await invokeCmd<RuntimeState>("unpin_standalone_widget", { widgetId });
      setRuntime(rt);
      notify("Widget removido da área de trabalho");
    } catch {
      /* ignore */
    }
  };

  const exportProject = async (proj: PrismProject) => {
    const isWidget = proj.packageType === "WIDGET";
    const safeTitle = proj.title.trim().replace(/[^a-zA-Z0-9_-]+/g, "_") || "PrismView";
    const defaultFileName = `${safeTitle}.${isWidget ? "widget.prism" : "prism"}`;

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
        notify(`Salvo em: ${res.archive_path}`);
      } catch (err) {
        notify(`Falha ao exportar: ${String(err)}`);
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
  };

  const importPrismFromDisk = async () => {
    try {
      const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "prism" });
      if (!picked) return;
      const imported = normalizeProjectScene3D(
        await invokeCmd<PrismProject>("import_prism_archive", {
          archivePath: picked.path,
        })
      );
      setProjects((prev) =>
        prev.some((p) => p.id === imported.id)
          ? prev.map((p) => (p.id === imported.id ? imported : p))
          : [...prev, imported]
      );
      setHubTab(imported.packageType === "WIDGET" ? "WIDGETS" : "WALLPAPERS");
      notify(`Importado: ${imported.title}`);
    } catch {
      notify("Cancelado ou arquivo .prism inválido");
    }
  };

  const importLocalModel3D = async (openInStudio: boolean) => {
    try {
      const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "model3d" });
      if (!picked) return;
      const title = picked.file_name.replace(/\.[^.]+$/, "");
      const proj = createPure3DProject(title, picked.path);
      await persistProjectImmediate(proj);
      if (openInStudio) {
        setEditing(proj);
      } else {
        await applyWallpaper(proj);
      }
    } catch {
      notify("Disponível no app nativo Tauri");
    }
  };

  const importLocalVideoOrImage = async (openInStudio: boolean) => {
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
      if (openInStudio) {
        setEditing(proj);
      } else {
        await applyWallpaper(proj);
      }
    } catch {
      notify("Disponível no app nativo Tauri");
    }
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

  const deleteProj = async (proj: PrismProject) => {
    // Cancel any debounced save for this project so it can never resurrect on disk
    if (pendingSaveRef.current?.id === proj.id) {
      pendingSaveRef.current = null;
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    }
    if (proj.packageType === "WIDGET") {
      if (runtime.pinned_widget_ids.includes(proj.id)) {
        const rt = await invokeCmd<RuntimeState>("unpin_standalone_widget", {
          widgetId: proj.id,
        }).catch(() => null);
        if (rt) setRuntime(rt);
      }
    } else {
      const activeMonitors = Object.entries(runtime.monitor_wallpapers ?? {})
        .filter(([, pid]) => pid === proj.id)
        .map(([mid]) => mid);
      if (activeMonitors.length > 0) {
        for (const mid of activeMonitors) {
          const rt = await invokeCmd<RuntimeState>("clear_desktop_wallpaper", {
            monitorId: mid,
          }).catch(() => null);
          if (rt) setRuntime(rt);
        }
      } else if (runtime.active_wallpaper_id === proj.id) {
        const rt = await invokeCmd<RuntimeState>("clear_desktop_wallpaper", {
          monitorId: null,
        }).catch(() => null);
        if (rt) setRuntime(rt);
      }
    }
    await invokeCmd("delete_project", { projectId: proj.id }).catch(() => {});
    setProjects((prev) => prev.filter((p) => p.id !== proj.id));
    notify(`Excluído: ${proj.title}`);
  };

  const togglePause = async () => {
    try {
      const rt = await invokeCmd<RuntimeState>("set_engine_paused", { paused: !runtime.paused });
      setRuntime(rt);
    } catch {
      /* ignore */
    }
  };

  const toggleAutostart = async () => {
    try {
      const next = await invokeCmd<boolean>("set_autostart_enabled", { enabled: !autostart });
      setAutostart(Boolean(next));
      notify(next ? "Iniciar com o Windows ativado" : "Iniciar com o Windows desativado");
    } catch {
      notify("Disponível no app desktop");
    }
  };

  if (editing) {
    return (
      <div className="w-screen h-screen bg-black text-white overflow-hidden">
        <StudioEditor
          project={editing}
          availableWidgets={widgets}
          telemetry={telemetry}
          onChangeProject={persistProject}
          onFlushSave={() => void flushPendingSave()}
          onBack={() => {
            void flushPendingSave();
            setEditing(null);
          }}
          onApply={() => {
            void flushPendingSave(true);
            if (editing.packageType === "WIDGET") {
              void pinWidget(editing);
            } else {
              void applyWallpaper(editing);
            }
          }}
          onExport={() => {
            void flushPendingSave();
            void exportProject(editing);
          }}
        />
        <ToastOverlay message={toast} />
      </div>
    );
  }

  return (
    <div className="relative w-screen h-screen bg-[#060608] text-white flex flex-col overflow-hidden">
      {/* Subtle Monochrome Architectural Light */}
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(ellipse 75% 45% at 50% 0%, rgba(255,255,255,0.045), transparent 72%)",
        }}
      />

      {/* Top Navigation Bar */}
      <header className="relative z-20 flex items-center justify-between px-7 pt-5 pb-3 gap-4">
        <div className="flex items-center gap-3 shrink-0">
          <span className="font-display text-xl font-semibold tracking-tight text-white">
            PrismView
          </span>
          <span className="h-3.5 w-px bg-white/15" />
          <span className="text-[11px] font-mono-fx text-[#A1A1AA]">
            CPU {Math.round(telemetry.cpu_percent)}% · RAM {Math.round(telemetry.ram_percent)}%
          </span>
        </div>

        {/* 3 Clear Hub Tabs */}
        <div className="frosted-chrome rounded-full p-1 flex items-center gap-1">
          {(
            [
              { id: "WALLPAPERS" as HubTab, label: "Wallpapers", icon: Monitor },
              { id: "WIDGETS" as HubTab, label: "Widgets Independentes", icon: Layers },
              { id: "MEDIA_3D" as HubTab, label: "3D & Mídia Local", icon: Box },
            ]
          ).map((t) => {
            const Icon = t.icon;
            const active = hubTab === t.id;
            return (
              <button
                key={t.id}
                onClick={() => setHubTab(t.id)}
                className={`px-4 py-2 rounded-full text-xs flex items-center gap-2 ${
                  active ? "prism-pill-active" : "prism-pill-idle border-transparent bg-transparent"
                }`}
              >
                <Icon className="w-3.5 h-3.5" />
                {t.label}
              </button>
            );
          })}
        </div>

        <div className="flex items-center gap-2 shrink-0">
          <button
            onClick={toggleAutostart}
            title="Iniciar automaticamente com o Windows"
            className={`rounded-full px-3.5 py-2 text-xs flex items-center gap-1.5 transition ${
              autostart
                ? "prism-pill-active"
                : "frosted-chrome text-[#A1A1AA] hover:text-white hover:border-white/30"
            }`}
          >
            <Power className="w-3.5 h-3.5" />
            <span>Autostart</span>
          </button>

          <label
            title={`Energia agora: ${POWER_PREFERENCE_LABELS[power.state]} — ${POWER_STATE_HINTS[power.state]}. Automático: Normal na tomada, Econômico na bateria, Ultra abaixo de 20%.`}
            className={`frosted-chrome rounded-full pl-3 pr-1.5 py-1 text-xs flex items-center gap-1.5 ${
              power.state === "NORMAL" ? "text-[#A1A1AA]" : "text-amber-100 border border-amber-300/25"
            }`}
          >
            <span>Energia</span>
            <select
              value={power.preference}
              onChange={(e) => power.setPreference(e.target.value as PowerPreference)}
              className="bg-black/60 border border-white/15 rounded-full px-2 py-1 text-[11px] text-white outline-none"
            >
              {(["AUTO", "NORMAL", "ECO", "ULTRA", "FROZEN"] as PowerPreference[]).map((p) => (
                <option key={p} value={p}>
                  {p === "AUTO" && power.preference === "AUTO"
                    ? `Automático (${POWER_PREFERENCE_LABELS[power.state]})`
                    : POWER_PREFERENCE_LABELS[p]}
                </option>
              ))}
            </select>
          </label>

          <button
            onClick={togglePause}
            className="frosted-chrome rounded-full px-4 py-2 text-xs flex items-center gap-2 text-white hover:border-white/30 transition"
          >
            <span
              className={`w-2 h-2 rounded-full ${
                !runtime.paused ? "bg-white" : "bg-[#52525B]"
              }`}
            />
            {runtime.paused ? "Pausado" : "Ativo"}
            {runtime.paused ? <Play className="w-3 h-3" /> : <Pause className="w-3 h-3" />}
          </button>
        </div>
      </header>

      {/* Hub Main Workspace */}
      <main className="relative z-10 flex-1 overflow-y-auto px-8 py-4">
        {hubTab === "WALLPAPERS" && (
          <div className="max-w-7xl mx-auto space-y-5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2 className="font-display text-lg font-semibold">Meus Wallpapers</h2>
                <p className="text-xs text-[#A1A1AA]">
                  Pausa automática quando janelas maximizadas ou jogos em tela cheia estiverem abertos.
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button
                  onClick={() => void addMissingDefaultPresets("WALLPAPER")}
                  className="prism-pill-idle rounded-full px-3.5 py-2 text-xs flex items-center gap-1.5"
                >
                  <Sparkles className="w-3.5 h-3.5" /> Modelos Base
                </button>
                <button
                  onClick={importPrismFromDisk}
                  className="prism-pill-idle rounded-full px-4 py-2 text-xs flex items-center gap-1.5"
                >
                  <Upload className="w-3.5 h-3.5" /> Importar .prism
                </button>
                <button
                  onClick={() => void createNew3DWallpaper()}
                  className="prism-pill-idle rounded-full px-3.5 py-2 text-xs flex items-center gap-1.5"
                >
                  <Box className="w-3.5 h-3.5" /> Novo 3D (.glb)
                </button>
                <button
                  onClick={() => createNew("WALLPAPER")}
                  className="prismatic-cta px-5 py-2 text-xs flex items-center gap-1.5"
                >
                  <Plus className="w-3.5 h-3.5" /> Novo Wallpaper
                </button>
              </div>
            </div>

            {monitors.length > 1 && (
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-[11px] text-[#A1A1AA] mr-1">Tela alvo:</span>
                <button
                  onClick={() => setTargetMonitorId(null)}
                  className={`px-3.5 py-1.5 rounded-full text-[11px] transition ${
                    targetMonitorId === null ? "prism-pill-active" : "prism-pill-idle"
                  }`}
                >
                  Todos os Monitores ({monitors.length})
                </button>
                {monitors.map((m, idx) => (
                  <button
                    key={m.id}
                    onClick={() => setTargetMonitorId(m.id)}
                    className={`px-3.5 py-1.5 rounded-full text-[11px] font-mono-fx transition ${
                      targetMonitorId === m.id ? "prism-pill-active" : "prism-pill-idle"
                    }`}
                  >
                    {m.is_primary ? "Principal" : `Tela ${idx + 1}`} · {m.width}×{m.height} ({Math.round(m.scale_factor * 100)}%)
                  </button>
                ))}
              </div>
            )}

            <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
              {wallpapers.map((wp) => {
                const isActive = targetMonitorId
                  ? runtime.monitor_wallpapers?.[targetMonitorId] === wp.id
                  : runtime.active_wallpaper_id === wp.id ||
                    Object.values(runtime.monitor_wallpapers ?? {}).includes(wp.id);
                return (
                  <article key={wp.id} className="prism-hub-card flex flex-col">
                    <div
                      onClick={() => setEditing(wp)}
                      className="relative aspect-video bg-black cursor-pointer overflow-hidden"
                    >
                      <HubThumbnail project={wp} />
                      {isActive && (
                        <span className="absolute top-2.5 left-2.5 px-2.5 py-1 rounded-full prism-pill-active text-[10px]">
                          Ativo no Desktop
                        </span>
                      )}
                    </div>
                    <div className="p-4 flex flex-col gap-3">
                      <div className="flex items-center justify-between">
                        <div className="min-w-0">
                          <h3 className="text-sm font-semibold text-white truncate">{wp.title}</h3>
                          <span className="text-[11px] text-[#A1A1AA]">
                            {wp.layers.length} {wp.layers.length === 1 ? "camada" : "camadas"}
                          </span>
                        </div>
                        <div className="flex items-center gap-1">
                          <button
                            onClick={() => exportProject(wp)}
                            title="Exportar .prism"
                            className="p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10"
                          >
                            <Download className="w-3.5 h-3.5" />
                          </button>
                          {wallpapers.length > 1 && (
                            <button
                              onClick={() => deleteProj(wp)}
                              title="Excluir"
                              className="p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10"
                            >
                              <Trash2 className="w-3.5 h-3.5" />
                            </button>
                          )}
                        </div>
                      </div>
                      <div className="grid grid-cols-2 gap-2">
                        {isActive ? (
                          <button
                            onClick={() => void clearWallpaper(targetMonitorId)}
                            className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                          >
                            <X className="w-3.5 h-3.5" /> Remover
                          </button>
                        ) : (
                          <button
                            onClick={() => applyWallpaper(wp)}
                            className="prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5"
                          >
                            <Sparkles className="w-3.5 h-3.5" /> Aplicar
                          </button>
                        )}
                        <button
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
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2 className="font-display text-lg font-semibold">Widgets Independentes (.widget.prism)</h2>
                <p className="text-xs text-[#A1A1AA]">
                  Fixe direto sobre o papel de parede normal do Windows OU importe como grupo dentro de qualquer Wallpaper no Estúdio.
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button
                  onClick={() => void addMissingDefaultPresets("WIDGET")}
                  className="prism-pill-idle rounded-full px-3.5 py-2 text-xs flex items-center gap-1.5"
                >
                  <Sparkles className="w-3.5 h-3.5" /> Modelos Base
                </button>
                <button
                  onClick={importPrismFromDisk}
                  className="prism-pill-idle rounded-full px-4 py-2 text-xs flex items-center gap-1.5"
                >
                  <Upload className="w-3.5 h-3.5" /> Importar .widget.prism
                </button>
                <button
                  onClick={() => createNew("WIDGET")}
                  className="prismatic-cta px-5 py-2 text-xs flex items-center gap-1.5"
                >
                  <Plus className="w-3.5 h-3.5" /> Novo Widget
                </button>
              </div>
            </div>

            {runtime.pinned_widget_ids.length >= 3 && (
              <div className="rounded-xl border border-amber-400/30 bg-amber-400/[0.06] px-3.5 py-2.5 text-[11px] text-amber-200/90">
                Cada widget fixado roda em uma janela WebView própria (dezenas de MB de RAM cada).{" "}
                Você tem {runtime.pinned_widget_ids.length} fixados — considere agrupar widgets dentro de um Wallpaper.
              </div>
            )}

            <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
              {widgets.map((w) => {
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
                      <div className="flex items-center justify-between">
                        <div>
                          <h3 className="text-sm font-semibold text-white">{w.title}</h3>
                          <span className="text-[11px] font-mono-fx text-[#A1A1AA]">
                            {w.widget.width}×{w.widget.height}px · {w.layers.length} camadas
                          </span>
                        </div>
                        <div className="flex items-center gap-1">
                          {pinned && isLocked && (
                            <button
                              onClick={async () => {
                                const rt = await invokeCmd<RuntimeState>("set_widget_locked", {
                                  widgetId: w.id,
                                  locked: false,
                                }).catch(() => null);
                                if (rt) setRuntime(rt);
                                notify(`Widget "${w.title}" destravado`);
                              }}
                              title="Destravar posição"
                              className="px-2.5 py-1 rounded-full prism-pill-idle text-[10px] flex items-center gap-1"
                            >
                              <Unlock className="w-3 h-3" /> Destravar
                            </button>
                          )}
                          <button
                            onClick={() => exportProject(w)}
                            title="Exportar .widget.prism"
                            className="p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10"
                          >
                            <Download className="w-3.5 h-3.5" />
                          </button>
                          {widgets.length > 1 && (
                            <button
                              onClick={() => deleteProj(w)}
                              title="Excluir widget"
                              className="p-1.5 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10"
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
                            onClick={() => unpinWidget(w.id)}
                            className="prism-pill-idle rounded-full py-2 text-xs"
                          >
                            Remover da Tela
                          </button>
                        ) : (
                          <button
                            onClick={() => pinWidget(w)}
                            className="prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5"
                          >
                            <Sparkles className="w-3.5 h-3.5" /> Fixar na Tela
                          </button>
                        )}
                        <button
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

        {hubTab === "MEDIA_3D" && (
          <div className="max-w-7xl mx-auto space-y-6 pb-8">
            {/* Universal Importers (GLB 3D / Local Video-Image / .prism Archive) */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div className="frosted-chrome frosted-chrome-panel p-5 flex flex-col justify-between gap-4">
                <div className="space-y-2">
                  <div className="w-9 h-9 rounded-xl bg-white/10 border border-white/15 flex items-center justify-center">
                    <Box className="w-4 h-4 text-white" />
                  </div>
                  <h3 className="text-sm font-semibold text-white">
                    Importar Modelo 3D (.glb / .gltf)
                  </h3>
                  <p className="text-xs text-[#A1A1AA] leading-relaxed">
                    Traga modelos e animações do Blender com compressão Draco, Meshopt e KTX2
                    offline. Conecte ossos, áudio WASAPI e capa de música no Estúdio.
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <button
                    onClick={() => void importLocalModel3D(true)}
                    className="prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5"
                  >
                    <Wand2 className="w-3.5 h-3.5" /> No Estúdio
                  </button>
                  <button
                    onClick={() => void importLocalModel3D(false)}
                    className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                  >
                    <Sparkles className="w-3.5 h-3.5" /> Aplicar
                  </button>
                </div>
              </div>

              <div className="frosted-chrome frosted-chrome-panel p-5 flex flex-col justify-between gap-4">
                <div className="space-y-2">
                  <div className="w-9 h-9 rounded-xl bg-white/10 border border-white/15 flex items-center justify-center">
                    <Film className="w-4 h-4 text-white" />
                  </div>
                  <h3 className="text-sm font-semibold text-white">
                    Vídeo ou Imagem Local
                  </h3>
                  <p className="text-xs text-[#A1A1AA] leading-relaxed">
                    Carregue qualquer <code className="font-mono-fx text-white">.mp4</code>,{" "}
                    <code className="font-mono-fx text-white">.webm</code>,{" "}
                    <code className="font-mono-fx text-white">.mkv</code> ou imagem como fundo
                    interativo com camadas de vidro e relógio.
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <button
                    onClick={() => void importLocalVideoOrImage(true)}
                    className="prism-pill-active rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                  >
                    <Wand2 className="w-3.5 h-3.5" /> No Estúdio
                  </button>
                  <button
                    onClick={() => void importLocalVideoOrImage(false)}
                    className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                  >
                    <Sparkles className="w-3.5 h-3.5" /> Aplicar
                  </button>
                </div>
              </div>

              <div className="frosted-chrome frosted-chrome-panel p-5 flex flex-col justify-between gap-4">
                <div className="space-y-2">
                  <div className="w-9 h-9 rounded-xl bg-white/10 border border-white/15 flex items-center justify-center">
                    <Package className="w-4 h-4 text-white" />
                  </div>
                  <h3 className="text-sm font-semibold text-white">
                    Pacote Portátil (.prism)
                  </h3>
                  <p className="text-xs text-[#A1A1AA] leading-relaxed">
                    Importe temas e widgets completos em arquivo único contendo modelos 3D,
                    texturas, fontes e fórmulas prontas para uso imediato.
                  </p>
                </div>
                <button
                  onClick={() => void importPrismFromDisk()}
                  className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5 w-full"
                >
                  <Upload className="w-3.5 h-3.5" /> Importar .prism / .widget.prism
                </button>
              </div>
            </div>

            {/* Interactive 3D Templates Gallery */}
            <div className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h2 className="text-sm font-semibold text-white">
                    Templates 3D Interativos Nativos
                  </h2>
                  <p className="text-xs text-[#A1A1AA]">
                    Cenas 3D ultra-leves prontas para aplicar ou personalizar com seus próprios modelos do Blender.
                  </p>
                </div>
                <button
                  onClick={() => void createNew3DWallpaper()}
                  className="prism-pill-idle rounded-full px-3.5 py-1.5 text-xs flex items-center gap-1.5 shrink-0"
                >
                  <Plus className="w-3.5 h-3.5" /> Cena 3D em Branco
                </button>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                {templates3d.map(({ project: tpl, subtitle, badges }) => {
                  const openOrCloneTemplate = async (applyDirectly: boolean) => {
                    const existing = projects.find((p) => p.id === tpl.id);
                    const target: PrismProject = existing
                      ? existing
                      : (JSON.parse(JSON.stringify(tpl)) as PrismProject);
                    if (!existing) {
                      await persistProjectImmediate(target);
                    }
                    if (applyDirectly) {
                      await applyWallpaper(target);
                    } else {
                      setEditing(target);
                    }
                  };
                  return (
                    <article
                      key={tpl.id}
                      className="frosted-chrome frosted-chrome-panel overflow-hidden flex flex-col justify-between"
                    >
                      <div>
                        <div className="relative aspect-video bg-[#08080A] overflow-hidden border-b border-white/10">
                          <HubThumbnail project={tpl} />
                          <span className="absolute top-2.5 left-2.5 px-2.5 py-0.5 rounded-full bg-black/80 border border-white/15 text-[10px] font-mono-fx uppercase tracking-wider text-white">
                            3D Nativo
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
                          onClick={() => void openOrCloneTemplate(true)}
                          className="prismatic-cta py-2 text-xs flex items-center justify-center gap-1.5"
                        >
                          <Sparkles className="w-3.5 h-3.5" /> Aplicar
                        </button>
                        <button
                          onClick={() => void openOrCloneTemplate(false)}
                          className="prism-pill-idle rounded-full py-2 text-xs flex items-center justify-center gap-1.5"
                        >
                          <Sliders className="w-3.5 h-3.5" /> Estúdio
                        </button>
                      </div>
                    </article>
                  );
                })}
              </div>
            </div>

            {/* Blender -> PrismView Ultra-Light Export Checklist */}
            <div className="frosted-chrome frosted-chrome-panel p-5 space-y-3">
              <div className="flex items-center justify-between gap-2">
                <h3 className="text-xs font-semibold uppercase tracking-wider text-white">
                  Guia Rápido de Otimização: Blender → PrismView
                </h3>
                <span className="text-[10px] font-mono-fx text-[#A1A1AA]">
                  Meta: ≤ 1% CPU · GPU Ultra-Leve
                </span>
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3 text-xs">
                <div className="rounded-xl bg-white/[0.03] border border-white/10 p-3 space-y-1">
                  <span className="font-mono-fx text-[10px] text-white/60">01 · FORMATO</span>
                  <p className="font-medium text-white">Exporte como glTF Binary (.glb)</p>
                  <p className="text-[11px] text-[#A1A1AA]">
                    Ative <strong>Compression (Draco)</strong> na exportação do Blender para arquivos até 8× menores.
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
                  <p className="font-medium text-white">WebP / KTX2 até 2048px</p>
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
            </div>
          </div>
        )}
      </main>

      <ToastOverlay message={toast} />
    </div>
  );
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <StudioHubApp />
  </React.StrictMode>
);
