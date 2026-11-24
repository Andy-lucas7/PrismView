import { useEffect, useRef } from "react";

function isEditableElement(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    return true;
  }
  return Boolean(target.closest("[contenteditable='true']"));
}

/** Sala 3D shortcuts. While present, they replace every 2D shortcut except undo/redo. */
export interface RoomShortcuts {
  onGizmo: (mode: "translate" | "rotate" | "scale") => void;
  onFocus: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onExit: () => void;
}

export interface KeyboardShortcutsConfig {
  enabled: boolean;
  /** Defined only while the Sala 3D mode is active (undefined = 2D shortcuts) */
  room?: RoomShortcuts;
  onUndo: () => void;
  onRedo: () => void;
  /** Ctrl+S: força o salvamento pendente (funciona também com foco em campo de texto). */
  onSave?: () => void;
  onDelete: () => void;
  onNudge: (dxPx: number, dyPx: number) => void;
  onCopy?: () => void;
  onCut?: () => void;
  onPaste?: () => void;
  onCopyStyle?: () => void;
  onPasteStyle?: () => void;
  onDuplicate?: () => void;
  onRename?: () => void;
  onSelectAll?: () => void;
  onDeselect?: () => void;
  onGroup?: () => void;
  onUngroup?: () => void;
  onBringForward?: () => void;
  onSendBackward?: () => void;
  onBringToFront?: () => void;
  onSendToBack?: () => void;
  onToggleHidden?: () => void;
  onToggleLock?: () => void;
}

export function useKeyboardShortcuts(config: KeyboardShortcutsConfig): void {
  const ref = useRef(config);
  ref.current = config;

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const cfg = ref.current;
      if (!cfg.enabled) return;
      const mod = e.ctrlKey || e.metaKey;
      const key = e.key.toLowerCase();
      if (mod && key === "s" && !e.shiftKey && !e.altKey && cfg.onSave) {
        e.preventDefault();
        cfg.onSave();
        return;
      }
      if (isEditableElement(e.target)) return;

      if (mod && key === "z" && !e.shiftKey) {
        e.preventDefault();
        cfg.onUndo();
        return;
      }

      if (mod && (key === "y" || (key === "z" && e.shiftKey))) {
        e.preventDefault();
        cfg.onRedo();
        return;
      }

      // Sala 3D: its own key map; 2D shortcuts (nudge, group, copy…) never fire here
      const room = cfg.room;
      if (room) {
        if (mod && key === "d" && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          if (!e.repeat) room.onDuplicate();
          return;
        }
        if (mod || e.altKey) return;
        if (e.key === "Delete" || e.key === "Backspace") {
          e.preventDefault();
          if (!e.repeat) room.onDelete();
        } else if (e.key === "Escape") {
          e.preventDefault();
          room.onExit();
        } else if (!e.shiftKey && !e.repeat) {
          if (key === "g") room.onGizmo("translate");
          else if (key === "r") room.onGizmo("rotate");
          else if (key === "s") room.onGizmo("scale");
          else if (key === "f") room.onFocus();
        }
        return;
      }

      if (mod && e.altKey && !e.shiftKey && key === "c") {
        if (cfg.onCopyStyle) {
          e.preventDefault();
          cfg.onCopyStyle();
          return;
        }
      }

      if (mod && e.altKey && !e.shiftKey && key === "v") {
        if (cfg.onPasteStyle) {
          e.preventDefault();
          cfg.onPasteStyle();
          return;
        }
      }

      if (mod && key === "c" && !e.shiftKey && !e.altKey) {
        if (cfg.onCopy) {
          e.preventDefault();
          cfg.onCopy();
          return;
        }
      }

      if (mod && key === "x" && !e.shiftKey && !e.altKey) {
        if (cfg.onCut) {
          e.preventDefault();
          cfg.onCut();
          return;
        }
      }

      if (mod && key === "v" && !e.shiftKey && !e.altKey) {
        if (cfg.onPaste) {
          e.preventDefault();
          cfg.onPaste();
          return;
        }
      }

      if (mod && key === "d" && !e.shiftKey && !e.altKey) {
        if (cfg.onDuplicate) {
          e.preventDefault();
          cfg.onDuplicate();
          return;
        }
      }

      if (mod && key === "a" && !e.shiftKey && !e.altKey) {
        if (cfg.onSelectAll) {
          e.preventDefault();
          cfg.onSelectAll();
          return;
        }
      }

      if (mod && key === "g" && !e.altKey) {
        if (e.shiftKey && cfg.onUngroup) {
          e.preventDefault();
          cfg.onUngroup();
          return;
        }
        if (!e.shiftKey && cfg.onGroup) {
          e.preventDefault();
          cfg.onGroup();
          return;
        }
      }

      if (!e.altKey && (e.key === "]" || (mod && e.code === "BracketRight" && e.key !== "\\"))) {
        if (e.shiftKey && cfg.onBringToFront) {
          e.preventDefault();
          cfg.onBringToFront();
          return;
        }
        if (!e.shiftKey && cfg.onBringForward) {
          e.preventDefault();
          cfg.onBringForward();
          return;
        }
      }

      if (!e.altKey && (e.key === "[" || (mod && e.code === "BracketLeft" && e.key !== "\\"))) {
        if (e.shiftKey && cfg.onSendToBack) {
          e.preventDefault();
          cfg.onSendToBack();
          return;
        }
        if (!e.shiftKey && cfg.onSendBackward) {
          e.preventDefault();
          cfg.onSendBackward();
          return;
        }
      }

      if (!mod && !e.altKey && !e.shiftKey && e.key === "F2") {
        if (cfg.onRename) {
          e.preventDefault();
          cfg.onRename();
          return;
        }
      }

      if (mod && e.shiftKey && key === "h") {
        if (cfg.onToggleHidden) {
          e.preventDefault();
          cfg.onToggleHidden();
          return;
        }
      }

      if (mod && !e.shiftKey && key === "l") {
        if (cfg.onToggleLock) {
          e.preventDefault();
          cfg.onToggleLock();
          return;
        }
      }

      if (e.key === "Escape") {
        if (cfg.onDeselect) {
          e.preventDefault();
          cfg.onDeselect();
          return;
        }
      }

      if (mod || e.altKey) return;

      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        cfg.onDelete();
        return;
      }

      const step = e.shiftKey ? 10 : 1;
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        cfg.onNudge(-step, 0);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        cfg.onNudge(step, 0);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        cfg.onNudge(0, -step);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        cfg.onNudge(0, step);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);
}
