import React from "react";
import { AudioLines, ImageIcon, MousePointer2, Sparkles, Upload } from "lucide-react";
import type {
  ReactiveBinding3D,
  Scene3DAutoSuggestion,
  Scene3DDynamicTexture,
  Scene3DPickedPart,
} from "../types/prism";
import { matchBody, splitNameTokens } from "../engine/scene3d/partHeuristics";
import {
  QUICK_RANGES,
  QUICK_SPECS,
  applyQuickBinding,
  applyQuickCover,
  findQuickBindings,
  findQuickCover,
  quickBindingId,
  suggestedQuickTarget,
  type QuickActionKind,
} from "../engine/scene3d/quickActions";

type Commit = "raw" | "discrete" | "coalesced";

export interface Scene3DQuickActionsProps {
  objectId: string;
  primaryObjectId: string;
  isGlb: boolean;
  suggestions: Scene3DAutoSuggestion[];
  materialNames: string[];
  /** Bones + named meshes/groups of the active object */
  nodeNames: string[];
  bindings: ReactiveBinding3D[];
  dynamicTextures: Scene3DDynamicTexture[];
  pickedPart: Scene3DPickedPart | null;
  onPatchBindings: (fn: (cur: ReactiveBinding3D[] | undefined) => ReactiveBinding3D[], commit: Commit) => void;
  onPatchDynTex: (fn: (cur: Scene3DDynamicTexture[] | undefined) => Scene3DDynamicTexture[], commit: Commit) => void;
  onPickCustomImage?: (texId: string) => void;
  onPickModel: () => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
}

const ROOT = "__ROOT__";

const selectCls =
  "min-w-0 flex-1 bg-black border border-white/15 rounded-lg px-1.5 py-0.5 text-[10px] text-white outline-none";

/** Toggle switch (no deps) */
const Toggle: React.FC<{ on: boolean; onChange: (v: boolean) => void; label: string }> = ({ on, onChange, label }) => (
  <button
    type="button"
    role="switch"
    aria-checked={on}
    aria-label={label}
    onClick={() => onChange(!on)}
    className={`relative w-7 h-4 rounded-full shrink-0 transition ${on ? "bg-emerald-400/80" : "bg-white/15"}`}
  >
    <span
      className={`absolute top-0.5 w-3 h-3 rounded-full bg-white transition-all ${on ? "left-3.5" : "left-0.5"}`}
    />
  </button>
);

/**
 * "Ações Rápidas" (modo fácil). Edits the same `config.bindings` / `object.dynamicTextures` the Pro
 * editor shows; every toggle is idempotent (see engine/scene3d/quickActions.ts).
 */
export const Scene3DQuickActions: React.FC<Scene3DQuickActionsProps> = ({
  objectId,
  primaryObjectId,
  isGlb,
  suggestions,
  materialNames,
  nodeNames,
  bindings,
  dynamicTextures,
  pickedPart,
  onPatchBindings,
  onPatchDynTex,
  onPickCustomImage,
  onPickModel,
  onBeginTx,
  onCommitTx,
}) => {
  const cover = findQuickCover(dynamicTextures, objectId);
  const coverSuggestion = suggestions.find((s) => s.kind === "DYNAMIC_TEXTURE")?.dynamicTexture?.materialName ?? null;
  const picked = pickedPart && pickedPart.objectId === objectId ? pickedPart : null;

  /** Current quick binding of `kind` (quick id first, else an equivalent one on the suggested target) */
  const current = (kind: QuickActionKind): ReactiveBinding3D | null => {
    const byId = bindings.find((b) => b.id === quickBindingId(kind, objectId));
    if (byId) return byId;
    const target = suggestedQuickTarget(suggestions, kind) ?? (kind === "AUDIO" ? ROOT : null);
    const hit = findQuickBindings(bindings, kind, objectId, primaryObjectId, target)[0];
    return hit ?? null;
  };

  const setQuick = (
    kind: QuickActionKind,
    enabled: boolean,
    targetName: string,
    strength?: number,
    commit: Commit = "discrete"
  ) =>
    onPatchBindings(
      (cur) => applyQuickBinding(cur, { kind, objectId, primaryObjectId, targetName, enabled, strength }),
      commit
    );

  const setCover = (patch: { enabled: boolean; materialName?: string; emissiveMap?: boolean; source?: Scene3DDynamicTexture["source"] }) =>
    onPatchDynTex((cur) => applyQuickCover(cur, { objectId, ...patch }), "discrete");

  const renderFollow = (kind: QuickActionKind, title: string, hint: string, icon: React.ReactNode) => {
    const b = current(kind);
    const on = Boolean(b?.enabled);
    const suggested = suggestedQuickTarget(suggestions, kind);
    const target = b?.targetName ?? suggested ?? (kind === "AUDIO" ? ROOT : "");
    const canEnable = Boolean(target);
    const range = QUICK_RANGES[kind];
    const strength = b?.strength ?? QUICK_SPECS[kind].strength;
    const options = target && !nodeNames.includes(target) && target !== ROOT ? [target, ...nodeNames] : nodeNames;
    return (
      <div className="rounded-xl bg-white/[0.03] border border-white/10 px-2 py-1.5 space-y-1">
        <div className="flex items-center gap-1.5">
          {icon}
          <span className="text-[11px] text-white flex-1 truncate" title={hint}>
            {title}
          </span>
          {!suggested && !b && kind !== "AUDIO" && <span className="text-[9px] text-[#71717A]">não detectado</span>}
          <Toggle
            label={title}
            on={on}
            onChange={(v) => {
              if (!canEnable && v) return;
              setQuick(kind, v, target || ROOT);
            }}
          />
        </div>
        {(on || (!suggested && kind !== "AUDIO")) && (
          <div className="flex items-center gap-1.5">
            <select
              value={target || ""}
              onChange={(e) => setQuick(kind, true, e.target.value, undefined)}
              className={selectCls}
              title="Osso / peça alvo"
            >
              {!target && <option value="">Escolher parte…</option>}
              <option value={ROOT}>Modelo inteiro</option>
              {options.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
            {on && (
              <input
                type="range"
                min={range.min}
                max={range.max}
                step={range.step}
                value={strength}
                onPointerDown={onBeginTx}
                onPointerUp={onCommitTx}
                onChange={(e) => setQuick(kind, true, target, Number(e.target.value), "raw")}
                className="w-20 accent-white"
                title={`Intensidade: ${strength}`}
              />
            )}
          </div>
        )}
      </div>
    );
  };

  const coverOn = Boolean(cover);
  const coverMat = cover?.materialName ?? coverSuggestion ?? materialNames[0] ?? "*";

  return (
    <div className="space-y-1.5 rounded-xl bg-gradient-to-b from-white/[0.05] to-transparent border border-white/10 p-2">
      <div className="flex items-center gap-1.5 text-[11px] font-medium text-white">
        <Sparkles className="w-3 h-3 text-amber-200" />
        <span className="flex-1">Ações Rápidas</span>
        <button
          type="button"
          onClick={onPickModel}
          className="prism-pill-idle px-2 py-0.5 rounded-full text-[10px] flex items-center gap-1"
          title="Importar / trocar modelo (.glb / .gltf)"
        >
          <Upload className="w-3 h-3" /> {isGlb ? "Trocar .glb" : "Importar .glb"}
        </button>
      </div>

      {picked && (
        <div className="flex flex-wrap items-center gap-1 rounded-lg bg-amber-400/10 border border-amber-300/25 px-2 py-1 text-[10px] text-amber-100">
          <span className="truncate max-w-full">
            Parte: <b>{picked.name === ROOT ? "Modelo inteiro" : picked.name}</b> →
          </span>
          <button
            type="button"
            className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
            onClick={() =>
              setQuick(matchBody(picked.name, splitNameTokens(picked.name)) ? "BODY" : "HEAD", true, picked.name)
            }
          >
            Seguir mouse
          </button>
          {picked.materials.length > 0 && (
            <button
              type="button"
              className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
              onClick={() => setCover({ enabled: true, materialName: picked.materials[0], source: "MEDIA_COVER" })}
            >
              Colocar capa da música
            </button>
          )}
          <button
            type="button"
            className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
            onClick={() => setQuick("AUDIO", true, picked.name)}
          >
            Reagir ao áudio
          </button>
        </div>
      )}

      {/* Capa da música em material/tela */}
      <div className="rounded-xl bg-white/[0.03] border border-white/10 px-2 py-1.5 space-y-1">
        <div className="flex items-center gap-1.5">
          <ImageIcon className="w-3 h-3 text-[#A1A1AA]" />
          <span className="text-[11px] text-white flex-1 truncate">
            {cover?.source === "CUSTOM_IMAGE" ? "Imagem em Material" : "Capa da Música em Tela"}
          </span>
          {!coverOn && coverSuggestion && <span className="text-[9px] text-amber-200/80 truncate max-w-[80px]">{coverSuggestion}</span>}
          <Toggle
            label="Capa da música"
            on={coverOn}
            onChange={(v) => setCover({ enabled: v, materialName: v ? coverMat : undefined })}
          />
        </div>
        {coverOn && cover && (
          <div className="space-y-1">
            <div className="flex items-center gap-1.5">
              <select
                value={cover.materialName || "*"}
                onChange={(e) => setCover({ enabled: true, materialName: e.target.value })}
                className={selectCls}
                title="Material que recebe a imagem"
              >
                <option value="*">Todos os materiais (*)</option>
                {(materialNames.includes(cover.materialName) || cover.materialName === "*"
                  ? materialNames
                  : [cover.materialName, ...materialNames]
                ).map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
              <label className="flex items-center gap-1 text-[10px] text-[#A1A1AA] shrink-0" title="Brilho de tela (emissive)">
                <input
                  type="checkbox"
                  checked={Boolean(cover.emissiveMap)}
                  onChange={(e) => setCover({ enabled: true, emissiveMap: e.target.checked })}
                />
                Brilho
              </label>
            </div>
            <div className="flex items-center gap-1">
              <button
                type="button"
                onClick={() => setCover({ enabled: true, source: "MEDIA_COVER" })}
                className={`flex-1 py-0.5 rounded-lg text-[10px] ${cover.source === "MEDIA_COVER" ? "prism-pill-active" : "prism-pill-idle"}`}
              >
                Música atual
              </button>
              {onPickCustomImage && (
                <button
                  type="button"
                  onClick={() => {
                    setCover({ enabled: true, source: "CUSTOM_IMAGE" });
                    onPickCustomImage(cover.id);
                  }}
                  className={`flex-1 py-0.5 rounded-lg text-[10px] truncate ${cover.source === "CUSTOM_IMAGE" ? "prism-pill-active" : "prism-pill-idle"}`}
                  title={cover.customSrc ?? "Escolher imagem do PC"}
                >
                  {cover.source === "CUSTOM_IMAGE" && cover.customSrc
                    ? cover.customSrc.split(/[\\/]/).pop()
                    : "Usar imagem do PC"}
                </button>
              )}
            </div>
          </div>
        )}
      </div>

      {renderFollow(
        "HEAD",
        "Cabeça segue o mouse",
        "Olhar p/ Cursor (NODE_LOOK_AT)",
        <MousePointer2 className="w-3 h-3 text-[#A1A1AA]" />
      )}
      {renderFollow(
        "BODY",
        "Corpo acompanha (leve)",
        "Inclinação sutil do tronco p/ o cursor; braços e objetos seguram a pose",
        <MousePointer2 className="w-3 h-3 text-[#71717A]" />
      )}
      {renderFollow(
        "AUDIO",
        "Pulsar com a batida",
        "Pulso de escala no Áudio Grave (NODE_SCALE_PULSE)",
        <AudioLines className="w-3 h-3 text-[#A1A1AA]" />
      )}
    </div>
  );
};
