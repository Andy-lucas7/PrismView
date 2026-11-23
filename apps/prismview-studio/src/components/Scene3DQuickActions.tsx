import React from "react";
import { Select } from "./ui/Controls";
import { AudioLines, ImageIcon, MousePointer2, MousePointerClick, Sparkles, Upload } from "lucide-react";
import type {
  ReactiveBinding3D,
  Scene3DAutoSuggestion,
  Scene3DDynamicTexture,
  Scene3DPickedPart,
} from "../types/prism";
import {
  QUICK_RANGES,
  QUICK_SPECS,
  applyQuickBinding,
  applyQuickCover,
  findQuickBindings,
  findQuickCover,
  quickBindingId,
  suggestedQuickTarget,
  toggleSuggestionBinding,
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
 * "Ações Rápidas" (modo fácil): comportamentos universais para qualquer objeto 3D (seguir cursor,
 * pulso ao clicar, reagir ao áudio, textura dinâmica em material) + chips contextuais somente para
 * partes realmente detectadas no modelo. Edita a mesma fonte da verdade do Modo Pro.
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
  const bindingSuggestions = suggestions.filter((s) => s.kind === "BINDING" && Boolean(s.binding));

  /** Current quick binding of `kind` (quick id first, else an equivalent binding on this object) */
  const current = (kind: QuickActionKind): ReactiveBinding3D | null => {
    const byId = bindings.find((b) => b.id === quickBindingId(kind, objectId));
    if (byId) return byId;
    const target = suggestedQuickTarget(suggestions, kind) ?? ROOT;
    return findQuickBindings(bindings, kind, objectId, primaryObjectId, target)[0] ?? null;
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

  const isSuggestionActive = (sug: Scene3DAutoSuggestion): boolean => {
    const tpl = sug.binding;
    if (!tpl) return false;
    return bindings.some(
      (b) =>
        b.enabled &&
        (b.objectId || primaryObjectId) === objectId &&
        b.source === tpl.source &&
        b.targetType === tpl.targetType &&
        b.targetName === tpl.targetName
    );
  };

  const renderBehavior = (kind: QuickActionKind, title: string, hint: string, icon: React.ReactNode) => {
    const b = current(kind);
    const on = Boolean(b?.enabled);
    const defaultTarget = suggestedQuickTarget(suggestions, kind) ?? ROOT;
    const target = b?.targetName ?? defaultTarget;
    const range = QUICK_RANGES[kind];
    const strength = b?.strength ?? QUICK_SPECS[kind].strength;
    const hasSubParts = nodeNames.length > 0;
    const options = target && !nodeNames.includes(target) && target !== ROOT ? [target, ...nodeNames] : nodeNames;
    return (
      <div className="rounded-xl bg-white/[0.03] border border-white/10 px-2 py-1.5 space-y-1">
        <div className="flex items-center gap-1.5">
          {icon}
          <span className="text-[11px] text-white flex-1 truncate" title={hint}>
            {title}
          </span>
          {on && target !== ROOT && (
            <span className="text-[9px] text-[#A1A1AA] truncate max-w-[72px]" title={target}>
              {target}
            </span>
          )}
          <Toggle label={title} on={on} onChange={(v) => setQuick(kind, v, target || ROOT)} />
        </div>
        {on && (
          <div className="flex items-center gap-1.5">
            {hasSubParts ? (
              <Select
                aria-label="Alvo da ação"
                value={target || ROOT}
                onChange={(e) => setQuick(kind, true, e.target.value, undefined)}
                className={selectCls}
                title="Objeto inteiro ou peça específica"
              >
                <option value={ROOT}>Modelo inteiro</option>
                {options.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </Select>
            ) : (
              <span className="text-[10px] text-[#71717A] flex-1">Intensidade</span>
            )}
            <input
              aria-label="Intensidade"
              type="range"
              min={range.min}
              max={range.max}
              step={range.step}
              value={strength}
              onPointerDown={onBeginTx}
              onPointerUp={onCommitTx}
              onChange={(e) => setQuick(kind, true, target || ROOT, Number(e.target.value), "raw")}
              className={hasSubParts ? "w-20 accent-white" : "w-28 accent-white"}
              title={`Intensidade: ${strength}`}
            />
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

      {/* Contextual bar when the user clicks a specific part/mesh in the 3D viewport */}
      {picked && (
        <div className="flex flex-wrap items-center gap-1 rounded-lg bg-amber-400/10 border border-amber-300/25 px-2 py-1 text-[10px] text-amber-100">
          <span className="truncate max-w-full">
            Peça: <b>{picked.name === ROOT ? "Modelo inteiro" : picked.name}</b> →
          </span>
          <button
            type="button"
            className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
            onClick={() => setQuick("LOOK", true, picked.name)}
          >
            Seguir cursor
          </button>
          <button
            type="button"
            className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
            onClick={() => setQuick("CLICK", true, picked.name)}
          >
            Reagir ao clique
          </button>
          <button
            type="button"
            className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
            onClick={() => setQuick("AUDIO", true, picked.name)}
          >
            Pulsar áudio
          </button>
          {picked.materials.length > 0 && (
            <button
              type="button"
              className="px-1.5 py-0.5 rounded-full bg-white/10 hover:bg-white/20"
              onClick={() => setCover({ enabled: true, materialName: picked.materials[0], source: "MEDIA_COVER" })}
            >
              Aplicar capa/imagem
            </button>
          )}
        </div>
      )}

      {/* 4 comportamentos universais (funcionam em qualquer modelo 3D) */}
      {renderBehavior(
        "LOOK",
        "Seguir o cursor",
        "Inclina o objeto (ou peça escolhida) na direção do mouse",
        <MousePointer2 className="w-3 h-3 text-[#A1A1AA]" />
      )}
      {renderBehavior(
        "CLICK",
        "Reagir ao clique",
        "Pulso tátil com mola ao clicar no objeto",
        <MousePointerClick className="w-3 h-3 text-[#A1A1AA]" />
      )}
      {renderBehavior(
        "AUDIO",
        "Pulsar com o áudio",
        "Reage aos graves da música tocando no PC",
        <AudioLines className="w-3 h-3 text-[#A1A1AA]" />
      )}

      {/* Textura dinâmica em material (Capa da Música ou Imagem do PC) */}
      <div className="rounded-xl bg-white/[0.03] border border-white/10 px-2 py-1.5 space-y-1">
        <div className="flex items-center gap-1.5">
          <ImageIcon className="w-3 h-3 text-[#A1A1AA]" />
          <span className="text-[11px] text-white flex-1 truncate" title="Projeta a capa da música atual ou uma imagem em um material do modelo">
            {cover?.source === "CUSTOM_IMAGE" ? "Imagem em Material" : "Capa / Imagem em Material"}
          </span>
          {!coverOn && coverSuggestion && (
            <span className="text-[9px] text-amber-200/80 truncate max-w-[80px]" title={`Material detectado: ${coverSuggestion}`}>
              {coverSuggestion}
            </span>
          )}
          <Toggle
            label="Textura dinâmica em material"
            on={coverOn}
            onChange={(v) => setCover({ enabled: v, materialName: v ? coverMat : undefined })}
          />
        </div>
        {coverOn && cover && (
          <div className="space-y-1">
            <div className="flex items-center gap-1.5">
              <Select
                aria-label="Material da textura"
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
              </Select>
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
                Capa da música
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
                    : "Imagem do PC"}
                </button>
              )}
            </div>
          </div>
        )}
      </div>

      {/* Sugestões contextuais: só aparecem se o .glb realmente tiver partes nomeadas detectadas */}
      {bindingSuggestions.length > 0 && (
        <div className="pt-0.5 space-y-1">
          <div className="text-[9px] uppercase tracking-wider text-[#71717A] px-0.5">
            Detectado neste modelo
          </div>
          <div className="flex flex-wrap gap-1">
            {bindingSuggestions.map((sug) => {
              const active = isSuggestionActive(sug);
              return (
                <button
                  key={sug.id}
                  type="button"
                  onClick={() =>
                    onPatchBindings(
                      (cur) => toggleSuggestionBinding(cur, sug, objectId, primaryObjectId),
                      "discrete"
                    )
                  }
                  title={sug.detail}
                  className={`px-2 py-0.5 rounded-full text-[10px] border transition ${
                    active
                      ? "bg-emerald-400/15 border-emerald-400/40 text-emerald-200"
                      : "bg-white/[0.03] border-white/10 text-[#D4D4D8] hover:bg-white/10"
                  }`}
                >
                  {active ? "✓ " : "+ "}
                  {sug.label}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
};
