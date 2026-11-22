import React, { useEffect, useMemo, useRef, useState } from "react";
import { Bone, Box, ChevronDown, ChevronRight, Folder, Search } from "lucide-react";
import type { Scene3DEditorApi } from "./Scene3DLayer";
import type { Scene3DObjectTree, Scene3DTreeNode } from "../engine/scene3d/modelTree";

// Studio-only outliner (bones / meshes / groups) of the selected Sala 3D object.
// The tree itself is built + cached per model by the lazy editor chunk (`Scene3DEditorApi.getObjectTree`).

const ROW_H = 22;
const VIEW_H = 240;
/** Above this many visible rows only the rows inside the viewport are mounted */
const VIRTUALIZE_AFTER = 300;
const OVERSCAN = 8;

const TYPE_ICON = {
  bone: Bone,
  mesh: Box,
  group: Folder,
} as const;

export interface Scene3DOutlinerProps {
  objectId: string;
  /** Sala 3D (ROOM) mode: the editor API (and therefore the tree) only exists there */
  roomMode: boolean;
  editorApiRef?: React.MutableRefObject<Scene3DEditorApi | null>;
  /** Changes whenever the editor API appears or a model (re)loads */
  rev: number;
}

export const Scene3DOutliner: React.FC<Scene3DOutlinerProps> = ({ objectId, roomMode, editorApiRef, rev }) => {
  const [tree, setTree] = useState<Scene3DObjectTree | null>(null);
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set());
  const [selectedNode, setSelectedNode] = useState<number | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // Refetch (cheap: cached per model) when object / editor / model changes; a new tree resets the local view state
  useEffect(() => {
    const next = roomMode ? editorApiRef?.current?.getObjectTree(objectId) ?? null : null;
    setTree((prev) => (prev === next ? prev : next));
  }, [objectId, roomMode, rev, editorApiRef]);

  useEffect(() => {
    setCollapsed(new Set());
    setSelectedNode(null);
    setQuery("");
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [tree]);

  // Viewport highlight follows the selected node; cleared on node change, object change, unmount (no leak)
  useEffect(() => {
    if (selectedNode === null) return;
    const api = editorApiRef?.current;
    if (!api) return;
    api.highlightNode(objectId, selectedNode);
    return () => {
      editorApiRef?.current?.highlightNode(objectId, null);
    };
  }, [selectedNode, objectId, tree, editorApiRef]);

  const q = query.trim().toLowerCase();

  const rows = useMemo<Scene3DTreeNode[]>(() => {
    if (!tree) return [];
    const flat = tree.flat;
    const out: Scene3DTreeNode[] = [];
    if (q) {
      // Matches + all their ancestors (kept expanded while searching)
      const keep = new Uint8Array(flat.length);
      for (const n of flat) {
        if (keep[n.id] === 1) continue;
        const hit =
          n.name.toLowerCase().includes(q) ||
          n.morphs.some((m) => m.toLowerCase().includes(q)) ||
          n.materials.some((m) => m.toLowerCase().includes(q));
        if (!hit) continue;
        for (let id = n.id; id >= 0 && keep[id] === 0; id = flat[id].parentId) keep[id] = 1;
      }
      for (const n of flat) if (keep[n.id] === 1) out.push(n);
      return out;
    }
    let skipBelow = -1; // depth of the collapsed node whose subtree is being skipped
    for (const n of flat) {
      if (skipBelow >= 0) {
        if (n.depth > skipBelow) continue;
        skipBelow = -1;
      }
      out.push(n);
      if (n.children.length > 0 && collapsed.has(n.id)) skipBelow = n.depth;
    }
    return out;
  }, [tree, q, collapsed]);

  if (!roomMode) {
    return (
      <p className="text-[10px] text-[#71717A]">Abra a Sala 3D para navegar pelos ossos e peças do modelo.</p>
    );
  }
  if (!tree) {
    return <p className="text-[10px] text-[#71717A]">Aguardando o modelo carregar…</p>;
  }
  if (tree.flat.length === 0) {
    return <p className="text-[10px] text-[#71717A]">Modelo sem nós.</p>;
  }

  const virtual = rows.length > VIRTUALIZE_AFTER;
  const viewH = Math.min(VIEW_H, Math.max(ROW_H, rows.length * ROW_H));
  let first = 0;
  let last = rows.length;
  if (virtual) {
    first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
    last = Math.min(rows.length, Math.ceil((scrollTop + VIEW_H) / ROW_H) + OVERSCAN);
  }

  const toggleCollapsed = (id: number) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  const selected = selectedNode !== null ? tree.flat[selectedNode] ?? null : null;

  const renderRow = (n: Scene3DTreeNode, i: number) => {
    const Icon = TYPE_ICON[n.type];
    const hasKids = n.children.length > 0;
    const open = q ? true : !collapsed.has(n.id);
    const active = n.id === selectedNode;
    return (
      <div
        key={n.id}
        role="treeitem"
        aria-selected={active}
        onClick={() => setSelectedNode(active ? null : n.id)}
        title={n.name}
        style={{
          position: virtual ? "absolute" : undefined,
          top: virtual ? i * ROW_H : undefined,
          left: 0,
          right: 0,
          height: ROW_H,
          paddingLeft: 4 + (q ? 0 : Math.min(n.depth, 14) * 10),
        }}
        className={`flex items-center gap-1 pr-1.5 cursor-pointer text-[10px] select-none ${
          active ? "bg-white/15 text-white" : "text-[#D4D4D8] hover:bg-white/5"
        }`}
      >
        {hasKids && !q ? (
          <button
            type="button"
            aria-label={open ? "Recolher" : "Expandir"}
            onClick={(e) => {
              e.stopPropagation();
              toggleCollapsed(n.id);
            }}
            className="w-3.5 h-3.5 shrink-0 flex items-center justify-center text-[#A1A1AA] hover:text-white"
          >
            {open ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
          </button>
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        <Icon className={`w-3 h-3 shrink-0 ${n.type === "bone" ? "text-amber-300" : "text-[#A1A1AA]"}`} />
        <span className="truncate font-mono-fx">{n.name}</span>
        {n.morphs.length > 0 && (
          <span className="ml-auto shrink-0 text-[9px] text-[#71717A]">{n.morphs.length} morph</span>
        )}
      </div>
    );
  };

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between text-[10px] font-mono-fx text-[#A1A1AA]">
        <span>
          {tree.boneCount} ossos · {tree.meshCount} malhas · {tree.flat.length} nós
        </span>
        {rows.length !== tree.flat.length && <span>{rows.length} visíveis</span>}
      </div>
      <div className="relative">
        <Search className="w-3 h-3 absolute left-2 top-1/2 -translate-y-1/2 text-[#71717A] pointer-events-none" />
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Buscar osso, malha, morph, material…"
          className="w-full bg-black/30 border border-white/10 rounded-lg pl-6 pr-2 py-1 text-[10px] text-white placeholder:text-[#71717A] focus:outline-none focus:border-white/30"
        />
      </div>
      <div
        ref={scrollRef}
        role="tree"
        onScroll={virtual ? (e) => setScrollTop(e.currentTarget.scrollTop) : undefined}
        style={{ height: viewH }}
        className="overflow-y-auto overflow-x-hidden rounded-lg bg-black/25 border border-white/10"
      >
        {rows.length === 0 ? (
          <p className="p-2 text-[10px] text-[#71717A]">Nada encontrado.</p>
        ) : virtual ? (
          <div style={{ height: rows.length * ROW_H, position: "relative" }}>
            {rows.slice(first, last).map((n, i) => renderRow(n, first + i))}
          </div>
        ) : (
          rows.map((n, i) => renderRow(n, i))
        )}
      </div>
      {selected && (
        <div className="text-[10px] text-[#A1A1AA] space-y-0.5">
          <p className="font-mono-fx text-white truncate">{selected.name}</p>
          {selected.morphs.length > 0 && (
            <p className="break-words">Morphs: {selected.morphs.join(", ")}</p>
          )}
          {selected.materials.length > 0 && (
            <p className="break-words">Materiais: {selected.materials.join(", ")}</p>
          )}
        </div>
      )}
    </div>
  );
};
