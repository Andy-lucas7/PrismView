import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { Move, Unlock, X } from "lucide-react";
import "./styles/prism.css";
import { invokeCmd, isTauriRuntime } from "./types/prism";
import { UnifiedCompositor } from "./components/UnifiedCompositor";
import { useDesktopWindowBridge } from "./hooks/useDesktopWindowBridge";
import { usePowerState } from "./engine/powerGovernor";

/**
 * MODE 1: Desktop Wallpaper Window (Attached to Win32 WorkerW per physical monitor, 0 UI chrome)
 */
const DesktopWallpaperHost: React.FC<{ monitorId: string | null }> = ({ monitorId }) => {
  const {
    hostRef,
    project,
    monitor,
    telemetry,
    visible,
    cursorNormRef,
    subscribeCursorTick,
    subscribeCursorClick,
    reactiveSignalRef,
    subscribeReactiveTick,
  } = useDesktopWindowBridge({
    sceneEvent: "prism://wallpaper-scene",
    readyCommand: "wallpaper_window_ready",
    readyArgs: { monitorId },
    listenMonitorChange: true,
  });
  const power = usePowerState(telemetry);

  if (!project) return <div ref={hostRef} className="w-screen h-screen bg-black" />;

  return (
    <div ref={hostRef} className="w-screen h-screen bg-black overflow-hidden">
      <UnifiedCompositor
        project={project}
        telemetry={telemetry}
        monitorInfo={monitor}
        cursorNormRef={cursorNormRef}
        subscribeCursorTick={subscribeCursorTick}
        subscribeCursorClick={subscribeCursorClick}
        reactiveSignalRef={reactiveSignalRef}
        subscribeReactiveTick={subscribeReactiveTick}
        paused={!visible}
        interactive={false}
        powerState={power.state}
      />
    </div>
  );
};

/**
 * MODE 2: Standalone Transparent Desktop Widget Window (works over native Windows wallpaper)
 */
const StandaloneWidgetHost: React.FC<{ widgetId: string }> = ({ widgetId }) => {
  const [locked, setLocked] = useState(false);

  useEffect(() => {
    document.documentElement.classList.add("prism-transparent");
    return () => {
      document.documentElement.classList.remove("prism-transparent");
    };
  }, []);

  const {
    hostRef,
    project,
    telemetry,
    visible,
    cursorNormRef,
    subscribeCursorTick,
    subscribeCursorClick,
    reactiveSignalRef,
    subscribeReactiveTick,
  } = useDesktopWindowBridge({
    sceneEvent: "prism://widget-scene",
    readyCommand: "widget_window_ready",
    readyArgs: { widgetId },
    acceptScene: (p) => p.id === widgetId,
    onProjectUpdate: (p) => setLocked(Boolean(p.widget?.locked)),
  });
  const power = usePowerState(telemetry);

  const startWindowDrag = async () => {
    if (locked || !isTauriRuntime()) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().startDragging();
  };

  const toggleLock = async () => {
    const next = !locked;
    setLocked(next);
    await invokeCmd("set_widget_locked", { widgetId, locked: next }).catch(() => {});
  };

  const unpinSelf = async () => {
    await invokeCmd("unpin_standalone_widget", { widgetId }).catch(() => {});
  };

  if (!project) return <div ref={hostRef} className="w-screen h-screen bg-transparent" />;

  return (
    <div ref={hostRef} className="group relative w-screen h-screen bg-transparent overflow-hidden">
      <UnifiedCompositor
        project={project}
        telemetry={telemetry}
        cursorNormRef={cursorNormRef}
        subscribeCursorTick={subscribeCursorTick}
        subscribeCursorClick={subscribeCursorClick}
        reactiveSignalRef={reactiveSignalRef}
        subscribeReactiveTick={subscribeReactiveTick}
        paused={!visible}
        interactive={false}
        powerState={power.state}
      />

      {/* Subtle Hover-Only Control Pill (Drag / Lock / Close) */}
      {!locked && (
        <div className="opacity-0 group-hover:opacity-100 transition-opacity absolute top-2 right-2 z-50 frosted-chrome rounded-full px-1.5 py-1 flex items-center gap-1">
          <button
            onMouseDown={startWindowDrag}
            title="Arrastar widget"
            className="p-1 rounded-full text-white/80 hover:text-white cursor-move"
          >
            <Move className="w-3 h-3" />
          </button>
          <button
            onClick={toggleLock}
            title="Travar posição (click-through)"
            className="p-1 rounded-full text-white/80 hover:text-white"
          >
            <Unlock className="w-3 h-3" />
          </button>
          <button
            onClick={unpinSelf}
            title="Remover widget"
            className="p-1 rounded-full text-white/80 hover:text-white"
          >
            <X className="w-3 h-3" />
          </button>
        </div>
      )}
    </div>
  );
};

/** Lightweight desktop entry (wallpaper / widget windows): no Studio, Hub or framer-motion. */
function DesktopRoot() {
  const params = new URLSearchParams(window.location.search);
  if (params.get("mode") === "widget") {
    return <StandaloneWidgetHost widgetId={params.get("id") ?? ""} />;
  }
  return <DesktopWallpaperHost monitorId={params.get("monitor")} />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopRoot />
  </React.StrictMode>
);
