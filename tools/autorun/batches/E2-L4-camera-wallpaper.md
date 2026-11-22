model: sonnet
effort: high
gate: both
---
# E2-L4 — "Definir Câmera do Wallpaper" + persistência + motor

Arquivos: `src/components/Scene3DLayer.tsx`, `src/components/StudioEditor.tsx`, `src/types/prism.ts` (`camera`), `src-tauri/src/native_scene.rs`, `crates/prismview-engine/src/core/scene3d/{config.rs,renderer.rs}`.

Tarefas:
1. Botão "Definir Câmera do Wallpaper" na toolbar ROOM: lê `getCameraState()` e grava `camera.position/target/fov` na config (um patch, um undo).
2. Fora do modo ROOM, a câmera do preview parte exatamente desse enquadramento; amplitude de paralaxe suave (`camera.parallax`) aplicada sobre ele, com vetores pré-alocados.
3. Slider de FOV e de paralaxe no Inspetor.
4. Motor: `project_to_scene` emite `camera`; `config.rs` com `#[serde(default)]`; `Scene3dRenderer` usa position/target/fov e o mesmo cálculo de paralaxe do preview (cursor já disponível via `set_pointer`).
5. Projetos sem `camera` mantêm o enquadramento atual (default compatível).

Aceite: o gate sem erros novos; reabrir projeto restaura o enquadramento.
Proibido: ações de ossos (E3).
