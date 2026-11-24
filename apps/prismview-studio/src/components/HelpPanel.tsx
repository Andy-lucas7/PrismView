import React, { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AnimatePresence, motion } from "framer-motion";
import { Check, Copy, Search, X } from "lucide-react";
import { SPRING_TACTILE } from "../types/prism";
import { useDialogFocus } from "../hooks/useDialogFocus";
import {
  buildHelpSections,
  buildSearchIndex,
  HELP_GROUPS,
  normalizeSearch,
  searchSections,
  type HelpBlock,
  type HelpSection,
} from "./helpContent";

export interface HelpPanelProps {
  open: boolean;
  onClose: () => void;
  /** Seção aberta ao abrir (ex.: "atalhos"). Padrão: a primeira. */
  initialSectionId?: string;
}

// ───────────── Realce dos termos buscados ─────────────

/** Divide `text` em trechos, marcando os que casam com algum termo (sem diferenciar acentos). */
function splitByTerms(text: string, terms: readonly string[]): { s: string; hit: boolean }[] {
  if (terms.length === 0) return [{ s: text, hit: false }];
  const norm = normalizeSearch(text);
  // Só realça se a normalização preservou as posições (texto sem decomposições exóticas).
  if (norm.length !== text.length) return [{ s: text, hit: false }];
  const marks = new Uint8Array(text.length);
  for (const t of terms) {
    let from = 0;
    for (;;) {
      const at = norm.indexOf(t, from);
      if (at < 0) break;
      marks.fill(1, at, at + t.length);
      from = at + t.length;
    }
  }
  const out: { s: string; hit: boolean }[] = [];
  let start = 0;
  for (let i = 1; i <= text.length; i++) {
    if (i === text.length || marks[i] !== marks[start]) {
      out.push({ s: text.slice(start, i), hit: marks[start] === 1 });
      start = i;
    }
  }
  return out;
}

const Hl: React.FC<{ text: string; terms: readonly string[] }> = ({ text, terms }) => {
  if (terms.length === 0) return <>{text}</>;
  return (
    <>
      {splitByTerms(text, terms).map((part, i) =>
        part.hit ? (
          <mark key={i} className="rounded-sm bg-white/25 px-0.5 text-white">
            {part.s}
          </mark>
        ) : (
          <React.Fragment key={i}>{part.s}</React.Fragment>
        )
      )}
    </>
  );
};

// ───────────── Blocos ─────────────

const CopyExample: React.FC<{ text: string }> = ({ text }) => {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    []
  );
  const copy = () => {
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setCopied(true);
        if (timer.current !== null) window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), 1400);
      })
      .catch(() => {
        /* sem permissão de área de transferência: nada a mostrar */
      });
  };
  return (
    <button
      type="button"
      onClick={copy}
      aria-label={copied ? "Copiado" : "Copiar exemplo"}
      title={copied ? "Copiado" : "Copiar exemplo"}
      className="shrink-0 rounded-lg p-1.5 text-[#A1A1AA] transition hover:bg-white/10 hover:text-white"
    >
      {copied ? <Check className="h-3.5 w-3.5 text-emerald-300" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
};

const Block: React.FC<{ block: HelpBlock; terms: readonly string[] }> = ({ block, terms }) => {
  switch (block.t) {
    case "p":
      return (
        <p className="text-[13px] leading-relaxed text-[#D4D4DA]">
          <Hl text={block.text} terms={terms} />
        </p>
      );
    case "h":
      return (
        <h3 className="mt-6 text-[11px] font-semibold uppercase tracking-wider text-[#A1A1AA]">
          <Hl text={block.text} terms={terms} />
        </h3>
      );
    case "ul":
      return (
        <ul className="list-disc space-y-1.5 pl-5 text-[13px] leading-relaxed text-[#D4D4DA] marker:text-white/30">
          {block.items.map((it, i) => (
            <li key={i}>
              <Hl text={it} terms={terms} />
            </li>
          ))}
        </ul>
      );
    case "ol":
      return (
        <ol className="list-decimal space-y-2 pl-5 text-[13px] leading-relaxed text-[#D4D4DA] marker:font-mono-fx marker:text-white/40">
          {block.items.map((it, i) => (
            <li key={i}>
              <Hl text={it} terms={terms} />
            </li>
          ))}
        </ol>
      );
    case "keys":
      return (
        <div className="divide-y divide-white/[0.06] rounded-xl border border-white/10 bg-white/[0.03]">
          {block.rows.map(([keys, desc]) => (
            <div key={keys} className="flex items-center justify-between gap-4 px-3 py-2 text-[12px]">
              <kbd className="font-mono-fx rounded bg-white/10 px-1.5 py-0.5 text-[10.5px] text-white/90">
                <Hl text={keys} terms={terms} />
              </kbd>
              <span className="text-right text-[#C4C4CC]">
                <Hl text={desc} terms={terms} />
              </span>
            </div>
          ))}
        </div>
      );
    case "table":
      return (
        <div className="overflow-x-auto rounded-xl border border-white/10">
          <table className="w-full border-collapse text-left text-[12px]">
            <thead>
              <tr className="bg-white/[0.05]">
                {block.head.map((h) => (
                  <th key={h} scope="col" className="px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wider text-[#A1A1AA]">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-white/[0.06]">
              {block.rows.map((row, ri) => (
                <tr key={ri} className="align-top">
                  {row.map((cell, ci) => (
                    <td
                      key={ci}
                      className={`px-3 py-2 leading-relaxed ${
                        block.mono?.includes(ci) ? "font-mono-fx text-[11px] text-white/90" : "text-[#C4C4CC]"
                      }`}
                    >
                      <Hl text={cell} terms={terms} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "example":
      return (
        <div className="rounded-xl border border-white/10 bg-black/30">
          <div className="flex items-start gap-2 px-3 py-2">
            <code className="min-w-0 flex-1 whitespace-pre-wrap break-words font-mono-fx text-[11.5px] leading-relaxed text-white">
              <Hl text={block.ex.input} terms={terms} />
            </code>
            <CopyExample text={block.ex.input} />
          </div>
          {block.ex.expected !== "" && (
            <div className="flex items-baseline gap-2 border-t border-white/[0.06] px-3 py-1.5 text-[12px]">
              <span className="text-[10.5px] uppercase tracking-wider text-[#71717A]">Resultado</span>
              <span className="min-w-0 break-words font-mono-fx text-[#E4E4E7]">
                <Hl text={block.ex.expected} terms={terms} />
              </span>
            </div>
          )}
          {block.ex.note && (
            <div className="border-t border-white/[0.06] px-3 py-1.5 text-[11.5px] text-[#A1A1AA]">
              <Hl text={block.ex.note} terms={terms} />
            </div>
          )}
        </div>
      );
    case "note":
      return (
        <p className="rounded-lg border-l-2 border-white/25 bg-white/[0.04] px-3 py-2 text-[11.5px] leading-relaxed text-[#A1A1AA]">
          <Hl text={block.text} terms={terms} />
        </p>
      );
  }
};

// ───────────── Painel ─────────────

const HelpPanel: React.FC<HelpPanelProps> = ({ open, onClose, initialSectionId }) => {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  useDialogFocus(open, panelRef);

  // Conteúdo e índice de busca só existem depois da primeira abertura.
  const sections = useMemo(() => (open ? buildHelpSections() : []), [open]);
  const index = useMemo(() => buildSearchIndex(sections), [sections]);

  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Ao abrir: limpa a busca e vai para a seção pedida.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setSelectedId(initialSectionId ?? null);
  }, [open, initialSectionId]);

  const terms = useMemo(() => normalizeSearch(deferredQuery).split(/\s+/).filter(Boolean), [deferredQuery]);
  const results = useMemo(() => searchSections(index, deferredQuery), [index, deferredQuery]);
  const searching = terms.length > 0;

  const selected: HelpSection | null = useMemo(() => {
    if (sections.length === 0) return null;
    const list = searching ? results : sections;
    return list.find((s) => s.id === selectedId) ?? list[0] ?? null;
  }, [sections, results, searching, selectedId]);

  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 });
  }, [selected?.id]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // Esc e F1 fecham; em fase de captura, para não chegarem aos atalhos do editor/Hub atrás.
      if (e.key === "Escape" || e.key === "F1") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  const pick = useCallback((id: string) => setSelectedId(id), []);

  const nav = searching ? (
    results.length === 0 ? (
      <p className="px-3 py-4 text-[12px] text-[#A1A1AA]" role="status">
        Nada encontrado para “{deferredQuery.trim()}”.
      </p>
    ) : (
      <ul className="space-y-0.5 px-2 pb-3">
        {results.map((s) => (
          <li key={s.id}>
            <button
              type="button"
              onClick={() => pick(s.id)}
              aria-current={selected?.id === s.id ? "page" : undefined}
              className={`w-full rounded-lg px-2.5 py-2 text-left transition ${
                selected?.id === s.id ? "bg-white/12 text-white" : "text-[#A1A1AA] hover:bg-white/[0.06] hover:text-white"
              }`}
            >
              <span className="block text-[12px] font-medium">
                <Hl text={s.title} terms={terms} />
              </span>
              <span className="block truncate text-[10.5px] text-[#71717A]">{s.group}</span>
            </button>
          </li>
        ))}
      </ul>
    )
  ) : (
    <div className="space-y-3 px-2 pb-3">
      {HELP_GROUPS.map((group) => {
        const items = sections.filter((s) => s.group === group);
        if (items.length === 0) return null;
        return (
          <div key={group}>
            {/* Sem `uppercase`: o CSS transformaria “ƒx” em “Ƒx”. */}
            <div className="px-2.5 pb-1 text-[10.5px] font-semibold tracking-wide text-[#71717A]">{group}</div>
            <ul className="space-y-0.5">
              {items.map((s) => (
                <li key={s.id}>
                  <button
                    type="button"
                    onClick={() => pick(s.id)}
                    aria-current={selected?.id === s.id ? "page" : undefined}
                    className={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-[12px] transition ${
                      selected?.id === s.id ? "bg-white/12 text-white" : "text-[#A1A1AA] hover:bg-white/[0.06] hover:text-white"
                    }`}
                  >
                    <span className="truncate">{s.title}</span>
                    {s.tag && <span className="shrink-0 text-[9.5px] tracking-wide text-[#71717A]">{s.tag}</span>}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        );
      })}
    </div>
  );

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          key="help-overlay"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-[120] flex items-center justify-center bg-black/65"
          onClick={onClose}
        >
          <motion.div
            ref={panelRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="help-title"
            initial={{ opacity: 0, y: 10, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.98 }}
            transition={SPRING_TACTILE}
            onClick={(e) => e.stopPropagation()}
            // Teclas digitadas aqui (setas, Delete, Ctrl+…) não podem virar atalhos do editor atrás.
            onKeyDown={(e) => e.stopPropagation()}
            className="prism-glass-pop flex h-[min(88vh,720px)] w-[min(94vw,1040px)] overflow-hidden"
          >
            <nav aria-label="Seções da Ajuda" className="flex w-[264px] shrink-0 flex-col border-r border-white/10">
              <div className="space-y-2.5 p-3 pb-2">
                <h2 id="help-title" className="px-1 text-sm font-semibold text-white">
                  Ajuda
                </h2>
                <label className="relative block">
                  <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[#71717A]" />
                  <input
                    ref={searchRef}
                    autoFocus
                    type="search"
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    aria-label="Buscar na Ajuda"
                    placeholder="Buscar: df, atalhos, vidro…"
                    className="w-full rounded-xl border border-white/10 bg-white/[0.05] py-2 pl-8 pr-2.5 text-[12px] text-white outline-none placeholder:text-[#71717A] focus:border-white/30"
                  />
                </label>
                {searching && results.length > 0 && (
                  <p className="px-1 text-[10.5px] text-[#71717A]" role="status">
                    {results.length} {results.length === 1 ? "seção encontrada" : "seções encontradas"}
                  </p>
                )}
              </div>
              <div className="min-h-0 flex-1 overflow-y-auto">{nav}</div>
            </nav>

            <section className="flex min-w-0 flex-1 flex-col">
              <header className="flex items-start justify-between gap-3 border-b border-white/10 px-6 py-4">
                <div className="min-w-0">
                  {selected && (
                    <>
                      <div className="text-[10.5px] font-medium tracking-wide text-[#71717A]">{selected.group}</div>
                      <h2 className="mt-0.5 truncate text-lg font-semibold text-white">{selected.title}</h2>
                    </>
                  )}
                </div>
                <button
                  type="button"
                  onClick={onClose}
                  aria-label="Fechar Ajuda"
                  title="Fechar (Esc)"
                  className="shrink-0 rounded-full p-1.5 text-[#A1A1AA] transition hover:bg-white/10 hover:text-white"
                >
                  <X className="h-4 w-4" />
                </button>
              </header>
              <div ref={contentRef} tabIndex={0} aria-label="Conteúdo da seção" className="min-h-0 flex-1 overflow-y-auto px-6 py-5 outline-none">
                {selected ? (
                  <div className="mx-auto max-w-[680px] space-y-3 pb-6">
                    {selected.blocks.map((b, i) => (
                      <Block key={`${selected.id}-${i}`} block={b} terms={terms} />
                    ))}
                  </div>
                ) : (
                  <p className="text-[13px] text-[#A1A1AA]">Escolha uma seção ao lado.</p>
                )}
              </div>
            </section>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body
  );
};

export default HelpPanel;
export { HelpPanel };
