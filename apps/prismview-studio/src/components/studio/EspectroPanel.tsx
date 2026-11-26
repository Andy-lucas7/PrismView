// Painel do Modo Espectro: código WGSL do efeito, erros do compilador, parâmetros gerados,
// modelos prontos e importar/exportar `.prismfx`. Montado só enquanto o modo está aberto.
import React, { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, BookOpen, Download, ImagePlus, Plus, Rainbow, Upload, X } from "lucide-react";
import {
  ESPECTRO_MAX_CODE_CHARS,
  ESPECTRO_TEMPLATES,
  PRISMFX_MAX_BYTES,
  createEspectroEffect,
  invokeCmd,
  isTauriRuntime,
  parseEspectroParams,
  parsePrismFxFull,
  resolveMediaUrl,
  serializePrismFx,
  setTextureSrc,
  textureSrc,
  type EspectroEffect,
  type EspectroParamValue,
  type PickedFileDto,
  type PrismFxTexture,
} from "../../types/prism";
import { compileEspectro, webgpuAvailable, type EspectroDiagnostic } from "../../engine/espectro/espectroGpu";
import { EspectroParams } from "./EspectroParams";

export interface EspectroPanelProps {
  /** Efeito da camada SHADER selecionada (null = nenhuma selecionada). */
  effect: EspectroEffect | null;
  layerName?: string;
  onChangeEffect: (fn: (e: EspectroEffect) => EspectroEffect, commit: "raw" | "discrete" | "coalesced") => void;
  onCreateLayer: (effect: EspectroEffect) => void;
  onClose: () => void;
  notify: (msg: string) => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

const CODE_DEBOUNCE_MS = 400;

/** Cola rápida do que o efeito pode usar (o prelúdio do Espectro). */
const REFERENCE: [string, string][] = [
  ["fn effect(uv: vec2f) -> vec4f", "obrigatória: cor do pixel em uv (0..1, origem no topo)"],
  ["sample_backdrop(uv)", "fundo do wallpaper (vídeo, imagem ou 3D)"],
  ["sample_previous(uv)", "quadro anterior deste efeito (memória: rastros, simulações)"],
  ["tex_NOME(uv) · tex_NOME_size()", "imagem declarada com //@texture NOME"],
  ["params.NOME", "valor de //@param (float, vec2, color, bool)"],
  ["prism.time · prism.delta · prism.frame", "segundos, segundos do quadro, nº do quadro"],
  ["prism.resolution · pixel()", "tamanho em px do efeito · 1 pixel em uv"],
  ["prism.cursor", "mouse em uv da camada"],
  ["prism.click · prism.click_age", "uv do último clique · segundos desde ele"],
  ["prism.input", "x: mouse apertado · y: tecla · z: digitação · w: ocioso (s)"],
  ["prism.audio · prism.beat", "x nível, y grave, z médio, w agudo · batida"],
  ["prism.media", "x tocando · y progresso 0..1 · z há sessão"],
  ["prism.battery", "bateria 0..1"],
  ["hash21(p) · noise2(p)", "aleatório e ruído suave"],
];

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(blob);
  });
}

export const EspectroPanel: React.FC<EspectroPanelProps> = ({
  effect,
  layerName,
  onChangeEffect,
  onCreateLayer,
  onClose,
  notify,
  onBeginTx,
  onCommitTx,
  onRecordStep,
  onFlushSave,
}) => {
  const [draft, setDraft] = useState(effect?.code ?? "");
  const [diags, setDiags] = useState<EspectroDiagnostic[]>([]);
  const [compiling, setCompiling] = useState(false);
  const textRef = useRef<HTMLTextAreaElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const commitTimer = useRef<number | null>(null);
  const lastCommitted = useRef(effect?.code ?? "");
  const hasGpu = useMemo(() => webgpuAvailable(), []);

  // Outra camada selecionada / undo: o rascunho segue o código salvo.
  useEffect(() => {
    const code = effect?.code ?? "";
    if (code !== lastCommitted.current) {
      lastCommitted.current = code;
      setDraft(code);
    }
  }, [effect?.code]);

  // Diagnósticos do rascunho (cache por código no módulo de GPU).
  useEffect(() => {
    if (!effect) return;
    let alive = true;
    setCompiling(true);
    const t = window.setTimeout(() => {
      compileEspectro(draft).then((c) => {
        if (!alive) return;
        setCompiling(false);
        setDiags(c ? c.diagnostics : []);
      });
    }, CODE_DEBOUNCE_MS);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft, Boolean(effect)]);

  useEffect(
    () => () => {
      if (commitTimer.current) window.clearTimeout(commitTimer.current);
    },
    []
  );

  const commitCode = (code: string) => {
    if (commitTimer.current) window.clearTimeout(commitTimer.current);
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null;
      lastCommitted.current = code;
      onChangeEffect((e) => ({ ...e, code }), "coalesced");
    }, CODE_DEBOUNCE_MS);
  };

  const onEdit = (code: string) => {
    if (code.length > ESPECTRO_MAX_CODE_CHARS) return;
    setDraft(code);
    commitCode(code);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Tab" && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      const el = e.currentTarget;
      const { selectionStart: a, selectionEnd: b } = el;
      const next = draft.slice(0, a) + "  " + draft.slice(b);
      onEdit(next);
      requestAnimationFrame(() => {
        el.selectionStart = el.selectionEnd = a + 2;
      });
    }
    // Esc fecha o modo sem passar pelos atalhos do canvas.
    if (e.key === "Escape") {
      e.stopPropagation();
      textRef.current?.blur();
    }
  };

  const jumpToLine = (line: number) => {
    const el = textRef.current;
    if (!el || line < 1) return;
    const lines = draft.split("\n");
    let pos = 0;
    for (let i = 0; i < Math.min(line - 1, lines.length); i++) pos += lines[i].length + 1;
    el.focus();
    el.setSelectionRange(pos, pos + (lines[line - 1]?.length ?? 0));
    const lh = parseFloat(getComputedStyle(el).lineHeight) || 18;
    el.scrollTop = Math.max(0, (line - 4) * lh);
  };

  const lineCount = useMemo(() => draft.split("\n").length, [draft]);
  const textureDefs = useMemo(() => parseEspectroParams(draft).textures, [draft]);

  const pickTexture = async (name: string) => {
    if (!isTauriRuntime()) {
      notify("Escolher imagem só funciona no app.");
      return;
    }
    try {
      const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "image" });
      if (picked) onChangeEffect((e) => setTextureSrc(e, name, picked.path), "discrete");
    } catch (err) {
      notify(`Não foi possível abrir a imagem: ${String(err)}`);
    }
  };
  const errorLines = useMemo(() => new Set(diags.filter((d) => d.severity === "error").map((d) => d.line)), [diags]);

  const exportEffect = async () => {
    if (!effect) return;
    const current = { ...effect, code: draft };
    const textures: PrismFxTexture[] = [];
    for (const def of textureDefs) {
      const src = textureSrc(current, def.name);
      if (!src) continue;
      try {
        const blob = await (await fetch(resolveMediaUrl(src))).blob();
        const dataUrl = await blobToDataUrl(blob);
        if (!/^data:image\/(png|jpeg|webp);base64,/.test(dataUrl)) {
          notify(`Textura "${def.name}" fora do efeito: use PNG, JPEG ou WebP.`);
          continue;
        }
        textures.push({ name: def.name, fileName: src.split(/[\\/]/).pop() ?? `${def.name}.png`, dataUrl });
      } catch {
        notify(`Não foi possível ler a textura "${def.name}".`);
      }
    }
    const contents = serializePrismFx(current, textures);
    if (contents.length > PRISMFX_MAX_BYTES) {
      notify("Efeito grande demais para exportar (limite de 24 MB). Use imagens menores.");
      return;
    }
    const base = (current.name || "efeito").replace(/[^\p{L}\p{N}_ -]+/gu, "_").trim() || "efeito";
    if (isTauriRuntime()) {
      try {
        const path = await invokeCmd<string | null>("export_prismfx", { defaultFileName: `${base}.prismfx`, contents });
        if (path) notify(`Efeito exportado: ${path.split(/[\\/]/).pop()}`);
      } catch (err) {
        notify(`Não foi possível exportar: ${String(err)}`);
      }
      return;
    }
    const url = URL.createObjectURL(new Blob([contents], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${base}.prismfx`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const importEffect = async (file: File) => {
    try {
      if (file.size > PRISMFX_MAX_BYTES) throw new Error("Arquivo grande demais (limite de 24 MB).");
      const { effect: parsed, textures } = parsePrismFxFull(await file.text());
      let withTex = parsed;
      for (const t of textures) {
        // No app as imagens vão para %APPDATA%\PrismView\assets\espectro; no navegador ficam em data URL.
        const path = isTauriRuntime()
          ? await invokeCmd<string>("save_espectro_texture", { fileName: t.fileName, dataUrl: t.dataUrl })
          : t.dataUrl;
        withTex = setTextureSrc(withTex, t.name, path);
      }
      onCreateLayer(withTex);
      notify(`Efeito "${parsed.name}" importado como nova camada.`);
    } catch (err) {
      notify(err instanceof Error ? err.message : "Não foi possível importar o efeito.");
    }
  };

  const errors = diags.filter((d) => d.severity === "error");

  return (
    <section
      aria-label="Modo Espectro"
      className="frosted-chrome rounded-2xl flex flex-col overflow-hidden text-white"
      style={{ width: "min(460px, 42vw)", height: "100%" }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <header className="flex items-center gap-2 px-3 py-2 border-b border-white/10">
        <Rainbow className="w-4 h-4 text-white/80" />
        <span className="text-xs font-semibold tracking-wide">Modo Espectro</span>
        {effect && (
          <input
            aria-label="Nome do efeito"
            value={effect.name}
            onChange={(e) => onChangeEffect((x) => ({ ...x, name: e.target.value.slice(0, 80) }), "coalesced")}
            className="ml-1 flex-1 min-w-0 bg-white/5 rounded-md px-2 py-1 text-xs outline-none"
          />
        )}
        {!effect && <span className="flex-1" />}
        <button
          type="button"
          aria-label="Fechar o Modo Espectro"
          title="Fechar o Modo Espectro"
          onClick={onClose}
          className="w-7 h-7 rounded-full flex items-center justify-center text-[#A1A1AA] hover:text-white hover:bg-white/15"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </header>

      <div className="flex items-center gap-1.5 px-3 py-2 border-b border-white/10 flex-wrap">
        <label className="sr-only" htmlFor="espectro-template">Novo efeito a partir de um modelo</label>
        <select
          id="espectro-template"
          defaultValue=""
          onChange={(e) => {
            if (!e.target.value) return;
            onCreateLayer(createEspectroEffect(e.target.value));
            e.target.value = "";
          }}
          className="bg-white/5 rounded-md px-2 py-1 text-xs outline-none"
          title="Cria uma camada de efeito a partir de um modelo"
        >
          <option value="">+ Novo efeito…</option>
          {ESPECTRO_TEMPLATES.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => fileRef.current?.click()}
          className="prism-pill-idle rounded-md px-2 py-1 text-xs flex items-center gap-1"
          title="Importar um efeito .prismfx como nova camada"
        >
          <Upload className="w-3.5 h-3.5" /> Importar
        </button>
        <button
          type="button"
          onClick={exportEffect}
          disabled={!effect}
          className="prism-pill-idle rounded-md px-2 py-1 text-xs flex items-center gap-1 disabled:opacity-40"
          title="Exportar só este efeito (.prismfx) para usar em outro wallpaper"
        >
          <Download className="w-3.5 h-3.5" /> Exportar efeito
        </button>
        <input
          ref={fileRef}
          type="file"
          accept=".prismfx,application/json"
          className="hidden"
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f) void importEffect(f);
          }}
        />
      </div>

      {!hasGpu && (
        <p className="mx-3 mt-2 text-[11px] text-amber-100/90 bg-amber-400/10 border border-amber-300/20 rounded-lg px-2 py-1.5">
          Este WebView não tem WebGPU: sem prévia nem checagem de erros aqui. O desktop ainda desenha o efeito ao Aplicar.
        </p>
      )}

      {!effect ? (
        <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
          <p className="text-xs text-[#A1A1AA] leading-relaxed">
            Selecione uma camada de efeito ou crie uma nova. Efeitos são shaders WGSL que leem o fundo
            (<code>sample_backdrop</code>), o tempo, o cursor e o áudio.
          </p>
          <button
            type="button"
            onClick={() => onCreateLayer(createEspectroEffect())}
            className="prism-pill-active rounded-full px-4 py-2 text-xs flex items-center gap-1.5"
          >
            <Plus className="w-3.5 h-3.5" /> Chuva no vidro
          </button>
        </div>
      ) : (
        <>
          <div className="relative flex-1 min-h-0 flex font-mono text-[12px] leading-[18px] bg-black/30">
            <div
              ref={gutterRef}
              aria-hidden="true"
              className="select-none text-right pr-2 pl-2 py-2 text-[#52525B] overflow-hidden shrink-0"
              style={{ minWidth: 38 }}
            >
              {Array.from({ length: lineCount }, (_, i) => (
                <div key={i} className={errorLines.has(i + 1) ? "text-red-400" : undefined}>
                  {i + 1}
                </div>
              ))}
            </div>
            <textarea
              ref={textRef}
              aria-label={`Código WGSL de ${layerName ?? effect.name}`}
              spellCheck={false}
              value={draft}
              onChange={(e) => onEdit(e.target.value)}
              onKeyDown={onKeyDown}
              onScroll={(e) => {
                if (gutterRef.current) gutterRef.current.scrollTop = e.currentTarget.scrollTop;
              }}
              wrap="off"
              className="flex-1 min-w-0 bg-transparent resize-none outline-none py-2 pr-2 text-[#E4E4E7] whitespace-pre overflow-auto"
            />
          </div>

          <details className="border-t border-white/10 px-3 py-1.5 group">
            <summary className="cursor-pointer text-[11px] text-[#A1A1AA] hover:text-white flex items-center gap-1.5 select-none">
              <BookOpen className="w-3 h-3" /> Referência
            </summary>
            <dl className="mt-1.5 max-h-40 overflow-auto space-y-1 text-[11px]">
              {REFERENCE.map(([k, v]) => (
                <div key={k}>
                  <dt className="font-mono text-[#E4E4E7]">{k}</dt>
                  <dd className="text-[#71717A] pl-2">{v}</dd>
                </div>
              ))}
            </dl>
          </details>

          <div className="border-t border-white/10 px-3 py-2 max-h-28 overflow-auto" aria-live="polite">
            {compiling ? (
              <span className="text-[11px] text-[#71717A]">Compilando…</span>
            ) : errors.length === 0 ? (
              <span className="text-[11px] text-emerald-300/90">Sem erros.</span>
            ) : (
              <ul className="space-y-1">
                {errors.slice(0, 8).map((d, i) => (
                  <li key={i}>
                    <button
                      type="button"
                      onClick={() => jumpToLine(d.line)}
                      className="text-left text-[11px] text-red-300 hover:text-red-200 flex gap-1.5"
                    >
                      <AlertTriangle className="w-3 h-3 mt-0.5 shrink-0" />
                      <span>
                        {d.line > 0 ? `Linha ${d.line}: ` : ""}
                        {d.message}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="border-t border-white/10 px-3 py-2 max-h-[38%] overflow-auto">
            {textureDefs.length > 0 && (
              <div className="mb-2 space-y-1.5">
                <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">Texturas</span>
                {textureDefs.map((def) => {
                  const src = textureSrc(effect, def.name);
                  return (
                    <div key={def.name} className="flex items-center gap-2 frosted-chrome-card px-2 py-1.5">
                      <span className="text-xs text-[#A1A1AA] shrink-0">{def.label}</span>
                      <span className="text-[11px] text-[#71717A] truncate flex-1 min-w-0" title={src || undefined}>
                        {src ? src.split(/[\\/]/).pop() : "nenhuma (branco)"}
                      </span>
                      <button
                        type="button"
                        onClick={() => void pickTexture(def.name)}
                        className="prism-pill-idle rounded-md px-2 py-0.5 text-[11px] flex items-center gap-1"
                        title={`Escolher a imagem de ${def.label}`}
                      >
                        <ImagePlus className="w-3 h-3" /> Escolher
                      </button>
                      {src && (
                        <button
                          type="button"
                          aria-label={`Remover a imagem de ${def.label}`}
                          title="Remover"
                          onClick={() => onChangeEffect((e) => setTextureSrc(e, def.name, ""), "discrete")}
                          className="w-6 h-6 rounded-full flex items-center justify-center text-[#A1A1AA] hover:text-white hover:bg-white/15"
                        >
                          <X className="w-3 h-3" />
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
            <span className="block text-[10px] uppercase tracking-wider text-[#71717A] mb-1.5">Parâmetros</span>
            <EspectroParams
              effect={{ ...effect, code: draft }}
              onChange={(name: string, value: EspectroParamValue, commit) =>
                onChangeEffect((e) => ({ ...e, params: { ...e.params, [name]: value } }), commit)
              }
              onBeginTx={onBeginTx}
              onCommitTx={onCommitTx}
              onRecordStep={onRecordStep}
              onFlushSave={onFlushSave}
            />
          </div>
        </>
      )}
    </section>
  );
};

export default EspectroPanel;
