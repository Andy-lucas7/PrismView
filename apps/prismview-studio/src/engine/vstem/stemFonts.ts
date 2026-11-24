/**
 * Carrega o módulo WebAssembly do alongamento vetorial e entrega os bytes de cada fonte a ele.
 *
 * Origem dos bytes (nesta ordem):
 * 1. fonte do PROJETO (`fontPath`): protocolo de assets do Tauri;
 * 2. app Tauri: comando `read_font_bytes` (fontes do sistema e as embutidas do Estúdio);
 * 3. `npm run dev` no navegador: `/__dev-fonts/` (plugin do Vite) só para as fontes embutidas.
 * Sem bytes (fonte web, falha, WebAssembly indisponível) devolve `null` e quem chama usa o
 * alongamento raster antigo (`utils/stemStretch.ts`). Falhas são lembradas: nada de novas tentativas
 * a cada render.
 */
import { invokeCmd, isTauriRuntime, resolveMediaUrl } from "../../types/prism";
import { VstemModule } from "./vstemWasm";

let modulePromise: Promise<VstemModule | null> | null = null;

/** Módulo único e preguiçoso; `null` se o .wasm não pôde ser carregado. */
export function loadVstem(): Promise<VstemModule | null> {
  if (modulePromise) return modulePromise;
  const url = `${import.meta.env.BASE_URL}wasm/prism_vstem.wasm`;
  modulePromise = (async () => {
    try {
      if (typeof WebAssembly === "undefined") return null;
      const res = await fetch(url);
      if (!res.ok) return null;
      return await VstemModule.instantiate(await res.arrayBuffer());
    } catch (e) {
      console.warn("[PrismView] alongamento vetorial indisponível:", e);
      return null;
    }
  })();
  return modulePromise;
}

/** Primeira família da pilha CSS, sem aspas. */
export function primaryFamily(stack: string): string {
  const first = stack.split(",")[0] ?? "";
  return first.trim().replace(/^["']|["']$/g, "").trim();
}

/** Peso para escolher o arquivo: múltiplo de 100 entre 100 e 900. */
export function weightBucket(weight: number): number {
  const w = Number.isFinite(weight) ? weight : 400;
  return Math.min(900, Math.max(100, Math.round(w / 100) * 100));
}

interface FontFile {
  bytes: Uint8Array;
  faceIndex: number;
}

/** Nome do TTF embutido (`Outfit-400.ttf`, `JetBrainsMono-300.ttf`) ou `null` se não for uma delas. */
export function bundledFontFile(family: string, weight: number): string | null {
  const stems: Record<string, { stem: string; weights: number[] }> = {
    outfit: { stem: "Outfit", weights: [300, 400, 500, 600, 700, 800] },
    inter: { stem: "Inter", weights: [300, 400, 500, 600, 700] },
    "jetbrains mono": { stem: "JetBrainsMono", weights: [300, 400, 500, 600] },
  };
  const entry = stems[family.trim().toLowerCase()];
  if (!entry) return null;
  const w = weightBucket(weight);
  const nearest = entry.weights.reduce((best, cur) => (Math.abs(cur - w) < Math.abs(best - w) ? cur : best));
  return `${entry.stem}-${nearest}.ttf`;
}

async function fetchBytes(url: string): Promise<Uint8Array | null> {
  const res = await fetch(url);
  if (!res.ok) return null;
  return new Uint8Array(await res.arrayBuffer());
}

async function loadFontFile(family: string, weight: number, fontPath: string): Promise<FontFile | null> {
  if (fontPath) {
    const bytes = await fetchBytes(resolveMediaUrl(fontPath));
    return bytes ? { bytes, faceIndex: 0 } : null;
  }
  if (isTauriRuntime()) {
    const raw = await invokeCmd<ArrayBuffer | number[]>("read_font_bytes", { family, weight: weightBucket(weight) });
    const all = raw instanceof ArrayBuffer ? new Uint8Array(raw) : Uint8Array.from(raw);
    if (all.length <= 4) return null;
    const faceIndex = new DataView(all.buffer, all.byteOffset, 4).getUint32(0, true);
    return { bytes: all.subarray(4), faceIndex };
  }
  if (import.meta.env.DEV) {
    const file = bundledFontFile(family, weight);
    if (!file) return null;
    const bytes = await fetchBytes(`/__dev-fonts/${file}`);
    return bytes ? { bytes, faceIndex: 0 } : null;
  }
  return null;
}

const fontIds = new Map<string, Promise<number | null>>();

/** Id da fonte no módulo (carrega e registra na 1ª vez; falhas ficam em cache como `null`). */
export function getStemFontId(vs: VstemModule, stack: string, weight: number, fontPath: string): Promise<number | null> {
  const family = primaryFamily(stack);
  if (!family && !fontPath) return Promise.resolve(null);
  const w = weightBucket(weight);
  const key = `${family}|${w}|${fontPath}`;
  const hit = fontIds.get(key);
  if (hit) return hit;
  const p = (async () => {
    try {
      const file = await loadFontFile(family, w, fontPath);
      if (!file) return null;
      const id = vs.addFont(file.bytes, file.faceIndex);
      return id > 0 ? id : null;
    } catch {
      return null;
    }
  })();
  fontIds.set(key, p);
  return p;
}

export interface StemFont {
  vs: VstemModule;
  id: number;
}

const resolved = new Map<string, StemFont>();
const keyOf = (stack: string, weight: number, fontPath: string) => `${primaryFamily(stack)}|${weightBucket(weight)}|${fontPath}`;

/** Módulo + fonte prontos (carrega o que faltar). `null` = sem alongamento vetorial para esta fonte. */
export async function resolveStemFont(stack: string, weight: number, fontPath: string): Promise<StemFont | null> {
  const key = keyOf(stack, weight, fontPath);
  const known = resolved.get(key);
  if (known) return known;
  const vs = await loadVstem();
  if (!vs) return null;
  const id = await getStemFontId(vs, stack, weight, fontPath);
  if (id === null) return null;
  const font = { vs, id };
  resolved.set(key, font);
  return font;
}

/** Versão síncrona: só devolve se `resolveStemFont` já terminou com sucesso (sem piscar a cada render). */
export function peekStemFont(stack: string, weight: number, fontPath: string): StemFont | undefined {
  return resolved.get(keyOf(stack, weight, fontPath));
}

/** Esquece uma fonte (ex.: o arquivo do projeto mudou): a próxima chamada recarrega. */
export function forgetStemFont(stack: string, weight: number, fontPath: string, vs?: VstemModule): void {
  const key = keyOf(stack, weight, fontPath);
  const hit = fontIds.get(key);
  fontIds.delete(key);
  resolved.delete(key);
  if (vs && hit) void hit.then((id) => id && vs.removeFont(id));
}
