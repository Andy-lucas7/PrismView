import type { WebGLRenderer } from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { DRACOLoader } from "three/examples/jsm/loaders/DRACOLoader.js";
import { KTX2Loader } from "three/examples/jsm/loaders/KTX2Loader.js";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";

/**
 * Module-level singleton glTF stack shared by every Scene3DLayer (2.2).
 * Draco/KTX2 own worker pools + WASM, so one per layer multiplied RAM/CPU.
 * - Created lazily on the first .glb load (never for layers with only sculptures).
 * - No `dracoLoader.preload()`: WASM is fetched only when a Draco model really shows up.
 * - Worker pools capped to 1 each.
 * - Reference counted: freed when the last layer releases it.
 */
let gltfLoader: GLTFLoader | null = null;
let dracoLoader: DRACOLoader | null = null;
let ktx2Loader: KTX2Loader | null = null;
let refCount = 0;

/** Call once per layer (engine init). Pair with `releaseSharedLoaders`. */
export function retainSharedLoaders(): void {
  refCount++;
}

export function releaseSharedLoaders(): void {
  refCount = Math.max(0, refCount - 1);
  if (refCount > 0) return;
  dracoLoader?.dispose();
  ktx2Loader?.dispose();
  dracoLoader = null;
  ktx2Loader = null;
  gltfLoader = null;
}

/** Returns the shared GLTFLoader, building it on first use. `renderer` is only used for KTX2 GPU-format detection. */
export function getSharedGltfLoader(renderer: WebGLRenderer): GLTFLoader {
  if (gltfLoader) return gltfLoader;

  dracoLoader = new DRACOLoader();
  dracoLoader.setDecoderPath("/decoders/draco/");
  dracoLoader.setWorkerLimit(1);

  ktx2Loader = new KTX2Loader();
  ktx2Loader.setTranscoderPath("/decoders/basis/");
  ktx2Loader.setWorkerLimit(1);
  ktx2Loader.detectSupport(renderer);

  gltfLoader = new GLTFLoader();
  gltfLoader.setDRACOLoader(dracoLoader);
  gltfLoader.setKTX2Loader(ktx2Loader);
  gltfLoader.setMeshoptDecoder(MeshoptDecoder);
  return gltfLoader;
}
