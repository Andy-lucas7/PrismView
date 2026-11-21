import { useCallback, useEffect, useRef, useState } from "react";
import {
  DEFAULT_REACTIVE_SIGNAL,
  DEFAULT_TELEMETRY,
  invokeCmd,
  isTauriRuntime,
  LiveTelemetry,
  MonitorInfoDto,
  normalizeProjectScene3D,
  PrismProject,
  ReactiveSignal,
} from "../types/prism";

interface WindowBootstrapDto {
  project: PrismProject | null;
  visible: boolean;
  telemetry: LiveTelemetry;
  monitor?: MonitorInfoDto | null;
}

export interface DesktopWindowBridgeOptions {
  sceneEvent: "prism://wallpaper-scene" | "prism://widget-scene";
  readyCommand: "wallpaper_window_ready" | "widget_window_ready";
  readyArgs?: Record<string, unknown>;
  acceptScene?: (project: PrismProject) => boolean;
  listenMonitorChange?: boolean;
  onProjectUpdate?: (project: PrismProject) => void;
}

/**
 * Shared runtime bridge for DesktopWallpaperHost and StandaloneWidgetHost:
 * registers listeners BEFORE firing the ready handshake, guards against stale
 * bootstrap overwrites, freezes telemetry while occluded, and coalesces cursor
 * events to display refresh rate via rAF (zero React re-renders on mouse movement).
 */
export function useDesktopWindowBridge(options: DesktopWindowBridgeOptions) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const visibleRef = useRef(true);
  const latestTelemetryRef = useRef<LiveTelemetry | null>(null);
  const cursorRafRef = useRef<number | null>(null);
  const pendingCursorRef = useRef<[number, number] | null>(null);
  const cursorNormRef = useRef<[number, number] | null>(null);
  const cursorSubsRef = useRef<Set<() => void>>(new Set());
  const cursorClickSubsRef = useRef<Set<(x: number, y: number) => void>>(new Set());
  const reactiveSignalRef = useRef<ReactiveSignal>(DEFAULT_REACTIVE_SIGNAL);
  const reactiveSubsRef = useRef<Set<() => void>>(new Set());
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const [project, setProject] = useState<PrismProject | null>(null);
  const [monitor, setMonitor] = useState<MonitorInfoDto | null>(null);
  const [telemetry, setTelemetry] = useState<LiveTelemetry>(DEFAULT_TELEMETRY);
  const [visible, setVisible] = useState(true);

  const subscribeCursorTick = useCallback((cb: () => void) => {
    cursorSubsRef.current.add(cb);
    return () => {
      cursorSubsRef.current.delete(cb);
    };
  }, []);

  const subscribeCursorClick = useCallback((cb: (x: number, y: number) => void) => {
    cursorClickSubsRef.current.add(cb);
    return () => {
      cursorClickSubsRef.current.delete(cb);
    };
  }, []);

  const subscribeReactiveTick = useCallback((cb: () => void) => {
    reactiveSubsRef.current.add(cb);
    return () => {
      reactiveSubsRef.current.delete(cb);
    };
  }, []);

  const argsKey = JSON.stringify(options.readyArgs ?? null);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unsubs: Array<() => void> = [];
    let receivedScene = false;
    let receivedVis = false;
    let receivedTelemetry = false;
    let receivedMonitor = false;

    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        const cfg = optionsRef.current;
        const listeners: Array<Promise<() => void>> = [
          listen<PrismProject>(cfg.sceneEvent, (ev) => {
            if (disposed || !ev.payload) return;
            if (optionsRef.current.acceptScene && !optionsRef.current.acceptScene(ev.payload)) {
              return;
            }
            receivedScene = true;
            const incoming = normalizeProjectScene3D(ev.payload);
            setProject(incoming);
            optionsRef.current.onProjectUpdate?.(incoming);
          }),
          listen<{ visible: boolean }>("prism://visibility-changed", (ev) => {
            if (disposed) return;
            receivedVis = true;
            const nextVis = ev.payload.visible;
            visibleRef.current = nextVis;
            if (!nextVis && cursorRafRef.current !== null) {
              cancelAnimationFrame(cursorRafRef.current);
              cursorRafRef.current = null;
              pendingCursorRef.current = null;
            }
            setVisible(nextVis);
            if (nextVis && latestTelemetryRef.current) {
              setTelemetry(latestTelemetryRef.current);
              latestTelemetryRef.current = null;
            }
          }),
          listen<LiveTelemetry>("prism://telemetry", (ev) => {
            if (disposed) return;
            receivedTelemetry = true;
            if (!visibleRef.current) {
              latestTelemetryRef.current = ev.payload;
              return;
            }
            setTelemetry(ev.payload);
          }),
          listen<{ x: number; y: number }>("prism://cursor", (ev) => {
            if (disposed || !visibleRef.current) return;
            // Coalesce high-frequency mouse events instead of cancelling rAF (prevents 1000Hz starvation)
            pendingCursorRef.current = [ev.payload.x, ev.payload.y];
            if (cursorRafRef.current !== null) return;
            cursorRafRef.current = requestAnimationFrame(() => {
              cursorRafRef.current = null;
              const cur = pendingCursorRef.current;
              pendingCursorRef.current = null;
              if (!cur || !visibleRef.current || !hostRef.current) return;
              cursorNormRef.current = cur;
              hostRef.current.style.setProperty("--px", ((cur[0] - 0.5) * 2).toFixed(4));
              hostRef.current.style.setProperty("--py", ((cur[1] - 0.5) * 2).toFixed(4));
              cursorSubsRef.current.forEach((cb) => cb());
            });
          }),
          listen<{ x: number; y: number }>("prism://cursor-click", (ev) => {
            if (disposed || !visibleRef.current) return;
            const cx = ev.payload.x;
            const cy = ev.payload.y;
            cursorNormRef.current = [cx, cy];
            cursorClickSubsRef.current.forEach((cb) => cb(cx, cy));
          }),
          listen<ReactiveSignal>("prism://reactive", (ev) => {
            if (disposed || !visibleRef.current) return;
            reactiveSignalRef.current = ev.payload;
            reactiveSubsRef.current.forEach((cb) => cb());
          }),
        ];

        if (cfg.listenMonitorChange) {
          listeners.push(
            listen<MonitorInfoDto>("prism://monitor-changed", (ev) => {
              if (!disposed) {
                receivedMonitor = true;
                setMonitor(ev.payload);
              }
            })
          );
        }

        const resolved = await Promise.all(listeners);
        if (disposed) {
          resolved.forEach((fn) => fn());
          return;
        }
        unsubs = resolved;

        try {
          const boot = await invokeCmd<WindowBootstrapDto>(cfg.readyCommand, cfg.readyArgs);
          if (disposed) return;
          if (!receivedVis) {
            visibleRef.current = boot.visible;
            setVisible(boot.visible);
          }
          if (!receivedTelemetry) {
            setTelemetry(boot.telemetry);
          }
          if (!receivedMonitor && boot.monitor) {
            setMonitor(boot.monitor);
          }
          if (
            !receivedScene &&
            boot.project &&
            (!optionsRef.current.acceptScene || optionsRef.current.acceptScene(boot.project))
          ) {
            const incoming = normalizeProjectScene3D(boot.project);
            setProject(incoming);
            optionsRef.current.onProjectUpdate?.(incoming);
          }
        } catch {
          /* ignore */
        }
      })
      .catch(() => {
        /* ignore event bridge errors */
      });

    return () => {
      disposed = true;
      if (cursorRafRef.current !== null) {
        cancelAnimationFrame(cursorRafRef.current);
        cursorRafRef.current = null;
      }
      pendingCursorRef.current = null;
      unsubs.forEach((fn) => fn());
    };
  }, [options.sceneEvent, options.readyCommand, argsKey, options.listenMonitorChange]);

  return {
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
  };
}
