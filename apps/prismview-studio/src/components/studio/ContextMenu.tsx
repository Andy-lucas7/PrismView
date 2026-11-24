import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { motion } from "framer-motion";
import {
  AlignCenter,
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignEndVertical,
  AlignHorizontalSpaceAround,
  AlignLeft,
  AlignRight,
  AlignStartHorizontal,
  AlignStartVertical,
  AlignVerticalSpaceAround,
  ArrowDown,
  ArrowDownToLine,
  ArrowUp,
  ArrowUpToLine,
  Box,
  CheckSquare,
  Clipboard,
  ClipboardPaste,
  Copy,
  Crosshair,
  Eye,
  EyeOff,
  FlipHorizontal2,
  FlipVertical2,
  FolderPlus,
  Lock,
  Paintbrush,
  Pencil,
  Plus,
  RotateCcw,
  RotateCw,
  Scissors,
  Sparkles,
  Trash2,
  Type,
  Ungroup,
  Unlock,
  Upload,
} from "lucide-react";
import { POP } from "../motion/Presence";
import { GLASS_STYLE_PRESETS, type LayerKind, type SceneLayer } from "../../types/prism";
import type { AlignMode, DistributeAxis, FlipAxis, MatchSizeMode } from "../../utils/align";
import { LAYER_KIND_LABELS, type TextCaseMode } from "../../utils/multiSelect";

export interface ContextMenuActions {
  onRename: () => void;
  onDuplicate: () => void;
  onCopy: () => void;
  onCut: () => void;
  onPaste: () => void;
  onCopyStyle: () => void;
  onPasteStyle: () => void;
  onDelete: () => void;
  onToggleLock: () => void;
  onToggleHidden: () => void;
  onSelectAll: () => void;
  onSelectSameKind: (kind: LayerKind) => void;
  onBringForward: () => void;
  onSendBackward: () => void;
  onBringToFront: () => void;
  onSendToBack: () => void;
  onGroup: () => void;
  onUngroup: () => void;
  onAlign: (mode: AlignMode, reference: "screen" | "selection") => void;
  onCenterOnScreen: () => void;
  onDistribute: (axis: DistributeAxis) => void;
  onMatchSize: (mode: MatchSizeMode) => void;
  onFlip: (axis: FlipAxis) => void;
  onRotate90: (dir: 1 | -1) => void;
  // Específicos por tipo
  onTextAlign: (align: "left" | "center" | "right") => void;
  onTextCase: (mode: TextCaseMode) => void;
  onTextEditFx: () => void;
  onTextResetStretch: () => void;
  onTextAutoWidth: () => void;
  onMediaFit: (fit: "cover" | "contain") => void;
  onMediaRadius: (radius: number) => void;
  onMediaPickFile: (kind: "video" | "image" | "svg" | "model3d") => void;
  onImageToggleMediaCover: () => void;
  onScene3DOpenRoom: () => void;
  onScene3DResetCamera: () => void;
  onScene3DResetTransform: () => void;
  onGlassPreset: (presetIndex: number) => void;
  onGlassReset: () => void;
  onProgressSource: (source: "MEDIA" | "CPU" | "RAM" | "BATTERY") => void;
  onAddNewLayer: (kind: LayerKind) => void;
}

export interface ContextMenuProps {
  x: number;
  y: number;
  selectedLayers: SceneLayer[];
  hasClipboard: boolean;
  hasCopiedStyle: boolean;
  actions: ContextMenuActions;
  onClose: () => void;
}

interface MenuItemProps {
  icon?: React.ComponentType<{ className?: string }>;
  label: string;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  active?: boolean;
  onClick: () => void;
}

const MenuItem: React.FC<MenuItemProps> = ({
  icon: Icon,
  label,
  shortcut,
  danger = false,
  disabled = false,
  active = false,
  onClick,
}) => (
  <button
    type="button"
    role="menuitem"
    disabled={disabled}
    onClick={onClick}
    className={`flex w-full items-center justify-between gap-3 rounded-lg px-2.5 py-1.5 text-left text-[11px] transition-colors disabled:opacity-35 ${
      danger
        ? "text-red-300 hover:bg-red-500/15 hover:text-red-200"
        : active
        ? "bg-white/15 text-white font-normal"
        : "text-white/85 hover:bg-white/12 hover:text-white"
    }`}
  >
    <span className="flex items-center gap-2 min-w-0 truncate">
      {Icon ? <Icon className="w-3.5 h-3.5 shrink-0 text-[#A1A1AA]" /> : <span className="w-3.5 shrink-0" />}
      <span className="truncate">{label}</span>
    </span>
    {shortcut && (
      <span className="font-mono-fx text-[10px] text-[#71717A] shrink-0">{shortcut}</span>
    )}
  </button>
);

const Divider: React.FC = () => <div className="my-1 h-px bg-white/10" role="separator" />;

/**
 * Menu de contexto de vidro (`.prism-glass-pop`), montado via Portal apenas enquanto aberto.
 */
export const ContextMenu: React.FC<ContextMenuProps> = ({
  x,
  y,
  selectedLayers,
  hasClipboard,
  hasCopiedStyle,
  actions,
  onClose,
}) => {
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>({ left: x, top: y });

  const run = useCallback(
    (fn: () => void) => {
      onClose();
      fn();
    },
    [onClose]
  );

  // Ajusta posição para nunca vazar pelas bordas da janela
  useLayoutEffect(() => {
    const el = menuRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const pad = 8;
    const clampedLeft = Math.max(pad, Math.min(window.innerWidth - rect.width - pad, x));
    const clampedTop = Math.max(pad, Math.min(window.innerHeight - rect.height - pad, y));
    setPos({ left: clampedLeft, top: clampedTop });
  }, [x, y, selectedLayers.length]);

  // Fecha ao clicar fora, Esc, scroll ou resize
  useEffect(() => {
    const onPointerDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (t && menuRef.current?.contains(t)) return;
      onClose();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        const items = Array.from(
          menuRef.current?.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]:not(:disabled)') ?? []
        );
        if (items.length === 0) return;
        e.preventDefault();
        const curIdx = items.indexOf(document.activeElement as HTMLButtonElement);
        const nextIdx =
          e.key === "ArrowDown"
            ? (curIdx + 1) % items.length
            : (curIdx - 1 + items.length) % items.length;
        items[nextIdx]?.focus();
      }
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  const count = selectedLayers.length;
  const primary = selectedLayers[count - 1] ?? null;
  const sameKind: LayerKind | null =
    count > 0 && selectedLayers.every((l) => l.kind === selectedLayers[0].kind)
      ? selectedLayers[0].kind
      : null;
  const anyUnlocked = selectedLayers.some((l) => !l.locked);
  const anyVisible = selectedLayers.some((l) => !l.hidden);
  const canUngroup = count === 1 && primary?.kind === "GROUP";
  const alignRef: "screen" | "selection" = count >= 2 ? "selection" : "screen";

  return createPortal(
    <motion.div
      ref={menuRef}
      role="menu"
      aria-label="Menu de ações da camada"
      onContextMenu={(e) => e.preventDefault()}
      {...POP}
      style={{
        left: pos.left,
        top: pos.top,
        maxHeight: "calc(100vh - 16px)",
      }}
      className="fixed z-[340] w-[264px] overflow-y-auto p-1.5 prism-glass-pop select-none"
    >
      {count === 0 ? (
        <>
          <div className="px-2.5 py-1 text-[10px] uppercase tracking-wider text-[#A1A1AA]">
            Canvas · Área Vazia
          </div>
          <MenuItem
            icon={ClipboardPaste}
            label="Colar camada"
            shortcut="Ctrl+V"
            disabled={!hasClipboard}
            onClick={() => run(actions.onPaste)}
          />
          <MenuItem
            icon={CheckSquare}
            label="Selecionar tudo"
            shortcut="Ctrl+A"
            onClick={() => run(actions.onSelectAll)}
          />
          <Divider />
          <div className="px-2.5 py-1 text-[10px] uppercase tracking-wider text-[#71717A]">
            Criar camada
          </div>
          <div className="grid grid-cols-2 gap-1 px-1 pb-1">
            {(["TEXT", "GLASS", "IMAGE", "VIDEO", "SVG", "SCENE_3D"] as const).map((k) => (
              <button
                key={k}
                type="button"
                role="menuitem"
                onClick={() => run(() => actions.onAddNewLayer(k))}
                className="flex items-center gap-1.5 rounded-lg px-2 py-1.5 text-[11px] text-white/85 hover:bg-white/12 hover:text-white"
              >
                <Plus className="w-3 h-3 text-[#A1A1AA]" />
                <span className="truncate">{LAYER_KIND_LABELS[k]}</span>
              </button>
            ))}
          </div>
        </>
      ) : (
        <>
          {/* Cabeçalho de seleção */}
          <div className="flex items-center justify-between px-2.5 py-1">
            <span className="text-[10px] uppercase tracking-wider text-[#A1A1AA] truncate">
              {count === 1
                ? `${primary.name} · ${LAYER_KIND_LABELS[primary.kind]}`
                : `${count} itens · ${sameKind ? LAYER_KIND_LABELS[sameKind] : "Tipos mistos"}`}
            </span>
          </div>

          {/* Faixa rápida de alinhamento (6 direções + centralizar) */}
          <div className="px-1.5 py-1 space-y-1">
            <div className="flex items-center justify-between text-[10px] text-[#71717A] px-1">
              <span>{count >= 2 ? "Alinhar seleção" : "Alinhar à tela"}</span>
              {count >= 2 && (
                <button
                  type="button"
                  onClick={() => run(actions.onCenterOnScreen)}
                  className="text-[10px] text-white/80 hover:text-white underline-offset-2 hover:underline"
                >
                  Centralizar na tela
                </button>
              )}
            </div>
            <div className="grid grid-cols-7 gap-0.5">
              {(
                [
                  { m: "left" as const, title: "Alinhar à esquerda", Icon: AlignStartVertical },
                  { m: "centerX" as const, title: "Centralizar na horizontal", Icon: AlignCenterVertical },
                  { m: "right" as const, title: "Alinhar à direita", Icon: AlignEndVertical },
                  { m: "top" as const, title: "Alinhar ao topo", Icon: AlignStartHorizontal },
                  { m: "centerY" as const, title: "Centralizar na vertical", Icon: AlignCenterHorizontal },
                  { m: "bottom" as const, title: "Alinhar à base", Icon: AlignEndHorizontal },
                ] as const
              ).map(({ m, title, Icon }) => (
                <button
                  key={m}
                  type="button"
                  title={title}
                  aria-label={title}
                  disabled={!anyUnlocked}
                  onClick={() => run(() => actions.onAlign(m, alignRef))}
                  className="grid h-7 place-items-center rounded-lg text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
                >
                  <Icon className="w-3.5 h-3.5" />
                </button>
              ))}
              <button
                type="button"
                title="Centralizar exatamente na tela"
                aria-label="Centralizar exatamente na tela"
                disabled={!anyUnlocked}
                onClick={() => run(actions.onCenterOnScreen)}
                className="grid h-7 place-items-center rounded-lg text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
              >
                <Crosshair className="w-3.5 h-3.5" />
              </button>
            </div>

            {/* Transformação rápida: espelhar H/V, girar -90°/+90°, distribuir (3+), igualar tamanho (2+) */}
            <div className="grid grid-cols-4 gap-1 pt-0.5">
              <button
                type="button"
                title="Espelhar horizontalmente"
                disabled={!anyUnlocked}
                onClick={() => run(() => actions.onFlip("horizontal"))}
                className="flex items-center justify-center gap-1 rounded-lg py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
              >
                <FlipHorizontal2 className="w-3 h-3" /> Esp. H
              </button>
              <button
                type="button"
                title="Espelhar verticalmente"
                disabled={!anyUnlocked}
                onClick={() => run(() => actions.onFlip("vertical"))}
                className="flex items-center justify-center gap-1 rounded-lg py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
              >
                <FlipVertical2 className="w-3 h-3" /> Esp. V
              </button>
              <button
                type="button"
                title="Girar -90°"
                disabled={!anyUnlocked}
                onClick={() => run(() => actions.onRotate90(-1))}
                className="flex items-center justify-center gap-1 rounded-lg py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
              >
                <RotateCcw className="w-3 h-3" /> -90°
              </button>
              <button
                type="button"
                title="Girar +90°"
                disabled={!anyUnlocked}
                onClick={() => run(() => actions.onRotate90(1))}
                className="flex items-center justify-center gap-1 rounded-lg py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
              >
                <RotateCw className="w-3 h-3" /> +90°
              </button>
            </div>

            {count >= 2 && (
              <div className="grid grid-cols-2 gap-1 pt-0.5">
                <button
                  type="button"
                  disabled={!anyUnlocked}
                  onClick={() => run(() => actions.onMatchSize("width"))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
                >
                  Igualar largura
                </button>
                <button
                  type="button"
                  disabled={!anyUnlocked}
                  onClick={() => run(() => actions.onMatchSize("height"))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
                >
                  Igualar altura
                </button>
                {count >= 3 && (
                  <>
                    <button
                      type="button"
                      disabled={!anyUnlocked}
                      onClick={() => run(() => actions.onDistribute("horizontal"))}
                      className="flex items-center justify-center gap-1 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
                    >
                      <AlignHorizontalSpaceAround className="w-3 h-3" /> Distribuir H
                    </button>
                    <button
                      type="button"
                      disabled={!anyUnlocked}
                      onClick={() => run(() => actions.onDistribute("vertical"))}
                      className="flex items-center justify-center gap-1 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/80 hover:bg-white/15 hover:text-white disabled:opacity-35"
                    >
                      <AlignVerticalSpaceAround className="w-3 h-3" /> Distribuir V
                    </button>
                  </>
                )}
              </div>
            )}
          </div>

          <Divider />

          {/* Seção contextual específica por tipo */}
          {sameKind === "TEXT" && (
            <>
              <div className="px-2.5 py-0.5 text-[10px] uppercase tracking-wider text-[#71717A]">
                Texto
              </div>
              <div className="grid grid-cols-6 gap-1 px-1.5 py-1">
                <button
                  type="button"
                  title="Alinhar texto à esquerda"
                  onClick={() => run(() => actions.onTextAlign("left"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] text-white/85 hover:bg-white/15"
                >
                  <AlignLeft className="w-3.5 h-3.5" />
                </button>
                <button
                  type="button"
                  title="Centralizar texto"
                  onClick={() => run(() => actions.onTextAlign("center"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] text-white/85 hover:bg-white/15"
                >
                  <AlignCenter className="w-3.5 h-3.5" />
                </button>
                <button
                  type="button"
                  title="Alinhar texto à direita"
                  onClick={() => run(() => actions.onTextAlign("right"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] text-white/85 hover:bg-white/15"
                >
                  <AlignRight className="w-3.5 h-3.5" />
                </button>
                <button
                  type="button"
                  title="MAIÚSCULAS"
                  onClick={() => run(() => actions.onTextCase("upper"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] font-mono-fx text-[10px] text-white/85 hover:bg-white/15"
                >
                  AA
                </button>
                <button
                  type="button"
                  title="minúsculas"
                  onClick={() => run(() => actions.onTextCase("lower"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] font-mono-fx text-[10px] text-white/85 hover:bg-white/15"
                >
                  aa
                </button>
                <button
                  type="button"
                  title="Capitalizar palavras"
                  onClick={() => run(() => actions.onTextCase("capitalize"))}
                  className="grid h-7 place-items-center rounded-lg bg-white/[0.04] font-mono-fx text-[10px] text-white/85 hover:bg-white/15"
                >
                  Aa
                </button>
              </div>
              {count === 1 && (
                <MenuItem
                  icon={Type}
                  label="Editar fórmula ƒx…"
                  onClick={() => run(actions.onTextEditFx)}
                />
              )}
              <MenuItem
                label="Largura automática (1 linha)"
                onClick={() => run(actions.onTextAutoWidth)}
              />
              {selectedLayers.some((l) => (l.text?.stemStretch ?? 1) > 1.01) && (
                <MenuItem
                  label="Resetar alongamento (1,0×)"
                  onClick={() => run(actions.onTextResetStretch)}
                />
              )}
              <Divider />
            </>
          )}

          {(sameKind === "IMAGE" || sameKind === "VIDEO") && (
            <>
              <div className="px-2.5 py-0.5 text-[10px] uppercase tracking-wider text-[#71717A]">
                {sameKind === "IMAGE" ? "Imagem" : "Vídeo"}
              </div>
              <div className="grid grid-cols-2 gap-1 px-1.5 py-1">
                <button
                  type="button"
                  onClick={() => run(() => actions.onMediaFit("cover"))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
                >
                  Preencher (Cover)
                </button>
                <button
                  type="button"
                  onClick={() => run(() => actions.onMediaFit("contain"))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
                >
                  Ajustar (Contain)
                </button>
                <button
                  type="button"
                  onClick={() => run(() => actions.onMediaRadius(0))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
                >
                  Cantos retos (0 px)
                </button>
                <button
                  type="button"
                  onClick={() => run(() => actions.onMediaRadius(24))}
                  className="rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
                >
                  Arredondar (24 px)
                </button>
              </div>
              {count === 1 && (
                <MenuItem
                  icon={Upload}
                  label={sameKind === "IMAGE" ? "Substituir imagem…" : "Substituir vídeo…"}
                  onClick={() =>
                    run(() => actions.onMediaPickFile(sameKind === "IMAGE" ? "image" : "video"))
                  }
                />
              )}
              {sameKind === "IMAGE" && (
                <MenuItem
                  label={
                    primary?.image?.sourceMode === "MEDIA_COVER"
                      ? "Usar arquivo local"
                      : "Usar capa da música ao vivo"
                  }
                  onClick={() => run(actions.onImageToggleMediaCover)}
                />
              )}
              <Divider />
            </>
          )}

          {sameKind === "SCENE_3D" && count === 1 && (
            <>
              <div className="px-2.5 py-0.5 text-[10px] uppercase tracking-wider text-[#71717A]">
                Modelo 3D
              </div>
              <MenuItem
                icon={Box}
                label="Abrir Sala 3D"
                onClick={() => run(actions.onScene3DOpenRoom)}
              />
              <MenuItem
                icon={Crosshair}
                label="Centralizar / resetar câmera"
                onClick={() => run(actions.onScene3DResetCamera)}
              />
              <MenuItem
                icon={RotateCcw}
                label="Resetar transformação do objeto"
                onClick={() => run(actions.onScene3DResetTransform)}
              />
              <MenuItem
                icon={Upload}
                label="Trocar modelo .glb / .gltf…"
                onClick={() => run(() => actions.onMediaPickFile("model3d"))}
              />
              <Divider />
            </>
          )}

          {sameKind === "GLASS" && (
            <>
              <div className="px-2.5 py-0.5 text-[10px] uppercase tracking-wider text-[#71717A]">
                Vidro Óptico
              </div>
              <div className="grid grid-cols-2 gap-1 px-1.5 py-1">
                {GLASS_STYLE_PRESETS.map((preset, idx) => (
                  <button
                    key={preset.label}
                    type="button"
                    onClick={() => run(() => actions.onGlassPreset(idx))}
                    className="truncate rounded-lg bg-white/[0.04] px-2 py-1 text-left text-[10px] text-white/85 hover:bg-white/15"
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
              <MenuItem
                icon={Sparkles}
                label="Resetar parâmetros do vidro"
                onClick={() => run(actions.onGlassReset)}
              />
              <Divider />
            </>
          )}

          {sameKind === "PROGRESS" && (
            <>
              <div className="px-2.5 py-0.5 text-[10px] uppercase tracking-wider text-[#71717A]">
                Fonte da Barra
              </div>
              <div className="grid grid-cols-2 gap-1 px-1.5 py-1">
                {(
                  [
                    { id: "MEDIA" as const, label: "Música" },
                    { id: "CPU" as const, label: "CPU" },
                    { id: "RAM" as const, label: "RAM" },
                    { id: "BATTERY" as const, label: "Bateria" },
                  ] as const
                ).map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => run(() => actions.onProgressSource(s.id))}
                    className={`rounded-lg px-2 py-1 text-[10px] ${
                      primary?.progress?.source === s.id
                        ? "bg-white text-black font-medium"
                        : "bg-white/[0.04] text-white/85 hover:bg-white/15"
                    }`}
                  >
                    {s.label}
                  </button>
                ))}
              </div>
              <Divider />
            </>
          )}

          {sameKind === "SVG" && count === 1 && (
            <>
              <MenuItem
                icon={Upload}
                label="Importar arquivo .svg…"
                onClick={() => run(() => actions.onMediaPickFile("svg"))}
              />
              <Divider />
            </>
          )}

          {/* Edição geral */}
          {count === 1 && (
            <MenuItem
              icon={Pencil}
              label="Renomear"
              shortcut="F2"
              onClick={() => run(actions.onRename)}
            />
          )}
          <MenuItem
            icon={Copy}
            label="Duplicar"
            shortcut="Ctrl+D"
            onClick={() => run(actions.onDuplicate)}
          />
          <MenuItem
            icon={Clipboard}
            label="Copiar"
            shortcut="Ctrl+C"
            onClick={() => run(actions.onCopy)}
          />
          <MenuItem
            icon={Scissors}
            label="Recortar"
            shortcut="Ctrl+X"
            disabled={!anyUnlocked}
            onClick={() => run(actions.onCut)}
          />
          <MenuItem
            icon={ClipboardPaste}
            label="Colar"
            shortcut="Ctrl+V"
            disabled={!hasClipboard}
            onClick={() => run(actions.onPaste)}
          />
          {count === 1 && (
            <MenuItem
              icon={Paintbrush}
              label="Copiar estilo"
              shortcut="Ctrl+Alt+C"
              onClick={() => run(actions.onCopyStyle)}
            />
          )}
          <MenuItem
            icon={Paintbrush}
            label="Colar estilo"
            shortcut="Ctrl+Alt+V"
            disabled={!hasCopiedStyle}
            onClick={() => run(actions.onPasteStyle)}
          />

          <Divider />

          {/* Grupos & Ordem Z */}
          <MenuItem
            icon={FolderPlus}
            label="Agrupar selecionadas"
            shortcut="Ctrl+G"
            onClick={() => run(actions.onGroup)}
          />
          {canUngroup && (
            <MenuItem
              icon={Ungroup}
              label="Desagrupar"
              shortcut="Ctrl+Shift+G"
              onClick={() => run(actions.onUngroup)}
            />
          )}

          <div className="grid grid-cols-2 gap-1 px-1.5 py-1">
            <button
              type="button"
              role="menuitem"
              onClick={() => run(actions.onBringToFront)}
              className="flex items-center gap-1.5 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
            >
              <ArrowUpToLine className="w-3 h-3 text-[#A1A1AA]" />
              <span>Frente total</span>
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => run(actions.onSendToBack)}
              className="flex items-center gap-1.5 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
            >
              <ArrowDownToLine className="w-3 h-3 text-[#A1A1AA]" />
              <span>Fundo total</span>
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => run(actions.onBringForward)}
              className="flex items-center gap-1.5 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
            >
              <ArrowUp className="w-3 h-3 text-[#A1A1AA]" />
              <span>Para frente (])</span>
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => run(actions.onSendBackward)}
              className="flex items-center gap-1.5 rounded-lg bg-white/[0.04] px-2 py-1 text-[10px] text-white/85 hover:bg-white/15"
            >
              <ArrowDown className="w-3 h-3 text-[#A1A1AA]" />
              <span>Para trás ([)</span>
            </button>
          </div>

          <Divider />

          {/* Visibilidade, bloqueio, seleção e exclusão */}
          <MenuItem
            icon={anyUnlocked ? Lock : Unlock}
            label={anyUnlocked ? "Travar" : "Destravar"}
            shortcut="Ctrl+L"
            onClick={() => run(actions.onToggleLock)}
          />
          <MenuItem
            icon={anyVisible ? EyeOff : Eye}
            label={anyVisible ? "Ocultar" : "Mostrar"}
            shortcut="Ctrl+Shift+H"
            onClick={() => run(actions.onToggleHidden)}
          />
          {primary && (
            <MenuItem
              icon={CheckSquare}
              label={`Selecionar todos de ${LAYER_KIND_LABELS[primary.kind]}`}
              onClick={() => run(() => actions.onSelectSameKind(primary.kind))}
            />
          )}
          <MenuItem
            icon={CheckSquare}
            label="Selecionar tudo"
            shortcut="Ctrl+A"
            onClick={() => run(actions.onSelectAll)}
          />

          <Divider />

          <MenuItem
            icon={Trash2}
            label={count > 1 ? `Excluir (${count})` : "Excluir"}
            shortcut="Del"
            danger
            disabled={!anyUnlocked}
            onClick={() => run(actions.onDelete)}
          />
        </>
      )}
    </motion.div>,
    document.body
  );
};
