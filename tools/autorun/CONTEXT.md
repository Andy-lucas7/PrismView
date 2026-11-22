# CONTEXT — Refatoração Sala 3D (fonte: prismview_3d_architecture.md)

## Objetivo
A. 3D com fundo transparente real (`backgroundMode`: TRANSPARENT padrão | SOLID | ENVIRONMENT; `autoRotateSpeed` padrão 0).
B. Sala 3D multi-objetos (`objects[]`), câmera livre no Estúdio, gizmos, câmera do wallpaper persistida.
C. Interação por ossos/partes: novos `NODE_ROTATE_SWING` e `NODE_MOVE_OFFSET`, outliner, Alt+Clique, chips de auto-detecção.

## Realidade da arquitetura (diferente do .md original)
- Quem desenha o wallpaper é o motor nativo (`crates/prismview-engine`, wgpu). `Scene3DLayer.tsx` é só o preview do Estúdio.
- Toda mudança de schema precisa ser espelhada: TS (`types/prism.ts`) → `src-tauri/src/native_scene.rs::project_to_scene` → contrato `core/scene3d/config.rs` (camelCase) → renderer/bindings do motor (`core/bindings/scene3d.rs`).
- Motor hoje: só a 1ª camada 3D desenha; SHOWCASE não renderiza; `Scene3dRenderer` já entrega RGBA pré-multiplicado.
- `desktop.tsx` é só widget. "0% overhead no Desktop" = helpers de edição (gizmo, grade, esqueleto, voo WASD) só no chunk `three-editor`, só no Estúdio.
- Validação: SOMENTE `node tools/autorun/gate.mjs <ts|rust|both>`. Saída já condensada e só com erros NOVOS (compara com baseline-*.txt gerado antes do 1º lote; erros pré-existentes do motor, que nunca foi compilado, são ignorados). Nunca rode `npm run build`/`cargo check` direto, nunca leia logs brutos nem `tools/autorun/logs`.
- Gate Rust "NAO VALIDADO" (sem cargo): escreva o código completo e registre no HANDOFF o que precisa de teste em máquina.

## Regras fixas
- CLAUDE.md vale sempre. Zero alocação em loop de render (pools/Maps fixas). `dispose()` em geometria/material/textura descartada, exceto texturas do `dynamicTexCache` (compartilhadas: devolver `baseMap` antes).
- Respeitar `powerCaps` (fpsCap/dprCap) em todo loop novo.
- Sem `blob:` salvo no projeto: caminho real via `pick_local_file`/`PickedFileDto`.
- Drift-free: movimento de nó por binding é delta aditivo sobre a pose do `AnimationMixer`, refeito por quadro; nunca sobrescrever a base.
- Mudou FX? espelhar `fxEngine.ts` ↔ `prism-core/src/fx.rs`.

## Contrato (rascunho; E1-L1 finaliza e anexa o CONTRATO FINAL abaixo)
```ts
type Scene3DBackgroundMode = "TRANSPARENT" | "SOLID" | "ENVIRONMENT";
interface Scene3DObject {
  id: string; name: string;
  kind: "GLB" | "DISPLAY_SLAB" | "FLOOR_GRID" | "SHOWCASE";
  modelSrc?: string;
  position: [number, number, number];
  rotation: [number, number, number];   // graus
  scale: [number, number, number];
  visible: boolean;
  autoRotateSpeed: number;              // 0
  playback: { clipName: string | null; speed: number; loop: boolean };
  dynamicTextures: /* tipo já existente */[];
  parentId?: string;
}
// Scene3DConfig += backgroundMode, backgroundColor, objects[],
//   camera: { position, target, fov, parallax }
// Binding 3D += objectId?: string, axis: "X"|"Y"|"Z", strength, smoothing, invert,
//   targetType += "NODE_ROTATE_SWING" | "NODE_MOVE_OFFSET"
```
Se já existir campo equivalente, reutilize e registre no CONTRATO FINAL.

## CONTRATO FINAL
Fonte: `apps/prismview-studio/src/types/prism.ts`. `SCENE3D_SCHEMA_VERSION = 3`.
```ts
type Scene3DObjectKind = "GLB" | "SHOWCASE";          // DISPLAY_SLAB = SHOWCASE + showcaseVariant "KINETIC_SLAB"; FLOOR_GRID não existe (grade = helper do editor)
type Scene3DShowcaseVariant = "MONOLITH" | "GYROSCOPE" | "KINETIC_SLAB";
type Scene3DBackgroundMode = "TRANSPARENT" | "SOLID" | "ENVIRONMENT";
type Scene3DAxis = "X" | "Y" | "Z";
type Scene3DBindingTarget = "NODE_LOOK_AT" | "PLAY_CLIP" | "EMISSIVE_GLOW" | "NODE_SCALE_PULSE"
  | "MORPH_WEIGHT" | "NODE_ROTATE_SPIN" | "NODE_ROTATE_SWING" | "NODE_MOVE_OFFSET";
interface Scene3DObject {
  id: string; name: string; kind: Scene3DObjectKind;
  modelSrc: string;                         // "" p/ SHOWCASE
  showcaseVariant?: Scene3DShowcaseVariant; // só SHOWCASE
  position: [n,n,n]; rotation: [n,n,n] /*graus, Euler*/; scale: [n,n,n];
  visible: boolean;
  autoRotateSpeed: number;                  // rad/s, padrão 0
  playback: { clipName: string /* "" = nenhum */; speed: number; loop: boolean };
  dynamicTextures?: Scene3DDynamicTexture[]; // normalizado sempre vira []
  parentId?: string;                         // transform local ao pai; pendente/cíclico → raiz
}
interface Scene3DCamera {                    // câmera persistida do wallpaper (órbita)
  fov: number; distance: number; azimuthDeg: number; elevationDeg: number;
  target?: [n,n,n];                          // sempre preenchido pelo normalize
  targetY: number;                           // @deprecated = target[1]
  orbitParallaxX: number; orbitParallaxY: number; // parallax do cursor (graus)
  damping: number;
}                                            // position = target + esférico(distance, azimuth, elevation)
interface Scene3DConfig {
  schemaVersion?: number; objects: Scene3DObject[] /* ≥1 */;
  backgroundMode: Scene3DBackgroundMode; backgroundColor: string /* SOLID */;
  envPreset; envIntensity; exposure; renderScale /*0.5–1*/; maxFps /*5–200*/; fpsMode?: "CAP"|"MONITOR";
  autoDayNight?; dayNightHourOverride?; camera: Scene3DCamera; bindings?: ReactiveBinding3D[];
  // @deprecated (≤v2, nunca mais salvos): modelSrc, showcaseVariant, autoRotateSpeed, modelScale, modelOffsetY, playback, dynamicTextures
}
interface ReactiveBinding3D {
  id; enabled; objectId?: string /* vazio = objects[0] */; source; sourceNode; targetType; targetName;
  strength; smoothing; invertX?; invertY?; axis?: Scene3DAxis /* padrão "Y" */; invertAxis?: boolean;
}
```
Migração: `normalizeScene3DConfig` (idempotente, mesma ref se atual) ← `normalizeProjectScene3D` em load/import/save/bridge. ≤v2: legado → `objects[0]` id `obj-primary` (escala uniforme `modelScale`, `position[1]=modelOffsetY`), spin 0.22→0, `backgroundMode=TRANSPARENT`. Helpers: `createScene3DObject`, `normalizeScene3DObjects`, `setScene3DObjectTransform`, `duplicateScene3DObject`, `removeScene3DObject`.
