import type { Material, Mesh, Object3D } from "three";
import type { ModelSceneIndex } from "./modelIndex";

// Outliner tree of a loaded model (bones / meshes / groups + morphs + materials).
// Studio-only: imported exclusively by the lazy Sala 3D editor chunk (never by the Desktop runtime).

export type Scene3DTreeNodeType = "bone" | "mesh" | "group";

export interface Scene3DTreeNode {
  /** Stable for the lifetime of the model: index into `Scene3DObjectTree.flat` (pre-order) */
  id: number;
  name: string;
  type: Scene3DTreeNodeType;
  depth: number;
  parentId: number;
  children: Scene3DTreeNode[];
  /** Morph target names (meshes only) */
  morphs: string[];
  /** Unique material names (meshes only) */
  materials: string[];
}

export interface Scene3DObjectTree {
  roots: Scene3DTreeNode[];
  /** Pre-order; `flat[node.id] === node` */
  flat: Scene3DTreeNode[];
  boneCount: number;
  meshCount: number;
}

interface TreeCacheEntry {
  tree: Scene3DObjectTree;
  /** Parallel to `tree.flat`: the live Object3D of every node */
  objects: Object3D[];
}

// Keyed by the model index: a reloaded/replaced model gets a new index and the old entry is collected.
const treeCache = new WeakMap<ModelSceneIndex, TreeCacheEntry>();

function nodeType(obj: Object3D): Scene3DTreeNodeType {
  if ((obj as Object3D & { isBone?: boolean }).isBone) return "bone";
  if ((obj as Mesh).isMesh) return "mesh";
  return "group";
}

function buildEntry(idx: ModelSceneIndex): TreeCacheEntry {
  const flat: Scene3DTreeNode[] = [];
  const objects: Object3D[] = [];
  const roots: Scene3DTreeNode[] = [];
  let boneCount = 0;
  let meshCount = 0;

  const visit = (obj: Object3D, depth: number, parentId: number, siblings: Scene3DTreeNode[]) => {
    const type = nodeType(obj);
    const morphs: string[] = [];
    const materials: string[] = [];
    if (type === "mesh") {
      meshCount++;
      const mesh = obj as Mesh;
      if (mesh.morphTargetDictionary) {
        for (const key of Object.keys(mesh.morphTargetDictionary)) morphs.push(key);
      }
      const mats = (Array.isArray(mesh.material) ? mesh.material : [mesh.material]) as Array<Material | undefined>;
      for (const m of mats) {
        const n = m?.name;
        if (n && !materials.includes(n)) materials.push(n);
      }
    } else if (type === "bone") {
      boneCount++;
    }
    const node: Scene3DTreeNode = {
      id: flat.length,
      name: obj.name || "(sem nome)",
      type,
      depth,
      parentId,
      children: [],
      morphs,
      materials,
    };
    flat.push(node);
    objects.push(obj);
    siblings.push(node);
    for (const child of obj.children) visit(child, depth + 1, node.id, node.children);
  };

  // The holder itself is the model root ("__ROOT__"): the outliner lists what hangs under it
  for (const child of idx.holder.children) visit(child, 0, -1, roots);

  return { tree: { roots, flat, boneCount, meshCount }, objects };
}

function entryFor(idx: ModelSceneIndex): TreeCacheEntry {
  let entry = treeCache.get(idx);
  if (!entry) {
    entry = buildEntry(idx);
    treeCache.set(idx, entry);
  }
  return entry;
}

/** Tree of a loaded model; built once per model index, then served from the cache. */
export function getModelTree(idx: ModelSceneIndex): Scene3DObjectTree {
  return entryFor(idx).tree;
}

/** Live Object3D behind a tree node id (null if the id is out of range). */
export function getTreeNodeObject(idx: ModelSceneIndex, nodeId: number): Object3D | null {
  return entryFor(idx).objects[nodeId] ?? null;
}
