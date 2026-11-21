# AUDITORIA PrismView — modo caveman

Foco: bug, vazamento memória 3D, gargalo CPU/GPU/RAM.
Raiz app: `apps/prismview-studio/`. Linha = número aproximado no arquivo hoje.
Ordem = prioridade. Fazer Etapa 1 primeiro.

---

## Etapa 1 — Bug que quebra coisa (fazer já)

### 1.1 Vidro sobre cena 3D congela
- Arquivo: `apps/prismview-studio/src/utils/opticalGlassWebGL.ts` (~722, `uploadBackdrop`)
- Problema: só re-upload se `kind === "video"`. Backdrop 3D é `kind: "canvas", live: true`. Chave fixa. Vidro mostra 1º quadro pra sempre.
- Conserto: `needsUpload = key mudou || ((bg.kind === "video" || bg.live) && frameToken mudou)`.

### 1.2 `blob:` URL salva no projeto
- Arquivo: `apps/prismview-studio/src/components/StudioEditor.tsx` (~1122, ~1126, ~1144, ~1247)
- Problema: vídeo/imagem/.glb vira `URL.createObjectURL` e vai pro projeto salvo. Janela wallpaper (outro WebView) não lê blob da janela main. Reiniciar app = link morto.
- Conserto: usar picker nativo Tauri (já existe `PickedFileDto` em `src/types/prism.ts`) → caminho real / copiar pra pasta assets. Blob só pra preview temporário.

### 1.3 Contexto WebGL volta, ambiente 3D preto
- Arquivo: `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~1206–1218, `onContextRestored`)
- Problema: PMREM `envTarget` some com contexto perdido. Código só acorda render. Reflexo fica preto.
- Conserto: no restore → `envPresetLoaded = null`, refazer `buildProceduralEnvironment`, `dynamicTexCache` marcar `needsUpdate = true`.

### 1.4 Textura vaza ao trocar/tirar modelo com textura dinâmica
- Arquivos:
  - `apps/prismview-studio/src/engine/scene3d/modelIndex.ts` (~260, `disposeObject3D`)
  - `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~1344 `clearRuntimeModel`, ~1662–1765 efeito texturas dinâmicas)
- Problema A: material com capa de música no `map` → `baseMap` original não está mais no material → `disposeObject3D` não acha → VRAM vaza.
- Problema B: `disposeObject3D` dá `dispose()` na textura do `dynamicTexCache` (compartilhada) → re-upload escondido.
- Conserto: antes de dispor, devolver `baseMap`/`baseEmissiveMap` ao material (ou dispor os dois na mão). Passar `Set` de texturas do cache pra `disposeObject3D` pular.

### 1.5 Pirâmide de blur fica refazendo
- Arquivo: `apps/prismview-studio/src/utils/opticalGlassWebGL.ts` (~750–826 `updateBlurPyramid`, ~452 `lastBackdropKey`)
- Problema: cache guarda 1 blur só e 1 backdrop só. Dois vidros com blur diferente (ou backdrop diferente) = upload + blur inteiro de novo POR VIDRO POR QUADRO.
- Conserto: cache pequeno (Map, 3–4 entradas) chave `backdropKey|frameToken|blurQuant` → textura. Textura sharp por backdrop key.

### 1.6 Load duplo de .glb
- Arquivo: `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~1460–1465 cleanup, ~1420 `loadingKey`)
- Problema: efeito roda de novo com mesmo objeto carregando → cleanup zera `loadingKey` → baixa arquivo outra vez. 2× RAM/CPU no parse.
- Conserto: guardar promessa em voo por `nextKey`, não cancelar se chave igual.

---

## Etapa 2 — Vazamento memória 3D / GPU / RAM

### 2.1 Miniatura do Hub = 1 WebGL inteiro cada
- Arquivos:
  - `apps/prismview-studio/src/App.tsx` (~1020 wallpapers, ~1155 widgets, ~1380 templates 3D com `paused={false}`)
  - `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~367 `new WebGLRenderer`, ~407 PMREM, ~418 Draco)
- Problema: cada card 3D cria renderer + PMREM + Draco + KTX2 + modelo. Chrome limita ~16 contextos → perde contexto velho. RAM e VRAM sobem muito. Template roda animado.
- Conserto: 1 renderer compartilhado gera foto (ImageBitmap/dataURL) uma vez → cache por `project.id + hash` → `<img>`. Depois `dispose`. Ou pôster estático.

### 2.2 Loaders e workers duplicados por camada
- Arquivo: `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~418–429)
- Problema: cada camada 3D cria `DRACOLoader` (até 4 workers + WASM) e `KTX2Loader` (workers + WASM). `dracoLoader.preload()` carrega WASM mesmo sem modelo Draco.
- Conserto: loaders singleton no módulo (`src/engine/scene3d/loaders.ts` novo). `setWorkerLimit(1)` ou 2. Sem `preload()`. Criar só quando 1º .glb aparece.

### 2.3 Wallpaper escondido segura GPU pra sempre
- Arquivos:
  - `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~1609–1615 efeito pause)
  - `apps/prismview-studio/src/hooks/useDesktopWindowBridge.ts` (~101 `visibility-changed`)
- Problema: jogo tela cheia horas → renderer, modelos, texturas, env ficam na VRAM/RAM.
- Conserto: "sono profundo": escondido > 60 s → dispor renderer e modelos, guardar só config. Voltar visível → recriar. Bônus Rust: WebView2 `ICoreWebView2_19::put_MemoryUsageTargetLevel(LOW)` quando oculto.

### 2.4 Cache de glifo come RAM
- Arquivo: `apps/prismview-studio/src/utils/glassLens.ts` (~427 `GLYPH_CACHE_LIMIT = 48`, ~495 `getGlyphGlassMaps`)
- Problema: até 48 canvas de até 1080×1080 RGBA ≈ 4,6 MB cada → ~220 MB pior caso. + dataURL base64 de fallback. Relógio muda texto todo segundo → entrada nova todo segundo.
- Conserto: limite 6–8 + orçamento em bytes. Texto que muda (relógio/fx) → atlas por caractere (0–9, :, etc.) gerado uma vez.

### 2.5 Histórico guarda 100 cópias do projeto
- Arquivo: `apps/prismview-studio/src/hooks/useHistory.ts` (~4 `cloneSnapshot`, ~13 `snapshotsEqual`, ~83 `pushPast`)
- Problema: `structuredClone` projeto inteiro a cada passo (50 undo + 50 redo). `JSON.stringify` duas vezes pra comparar. Projeto grande (SVG custom, muitas camadas) = RAM e travada no commit.
- Conserto: estado já é imutável (patch com spread) → guardar referência, sem clone. Comparar por `===`.

### 2.6 Blob URL acumula no Studio
- Arquivo: `apps/prismview-studio/src/components/StudioEditor.tsx` (~276–282, ~1122–1150, ~1247)
- Problema: cada troca de arquivo cria blob novo; antigo só sai quando fecha editor. Vídeo de 1 GB trocado 5× = 5 GB presos.
- Conserto: revogar URL antiga ao trocar (junto com 1.2).

### 2.7 `<video>` não solta decoder
- Arquivo: `apps/prismview-studio/src/components/UnifiedCompositor.tsx` (~317–384 `RealVideoLayer`)
- Problema: desmontar não limpa `src`. WebView2 pode segurar buffer de decode.
- Conserto: cleanup no `useEffect`: `el.pause(); el.removeAttribute("src"); el.load();`.

### 2.8 AnimationMixer sem `uncacheRoot`
- Arquivo: `apps/prismview-studio/src/components/Scene3DLayer.tsx` (~1344 `clearRuntimeModel`)
- Conserto: `rt.mixer.uncacheRoot(rt.mixer.getRoot())` antes de `rt.mixer = null`. Pequeno, mas limpo.

### 2.9 Fonte custom carrega duplicada
- Arquivo: `apps/prismview-studio/src/components/UnifiedCompositor.tsx` (~162 `ensureCustomFontLoaded`)
- Problema: cache só marca depois do load. Várias camadas = vários downloads ao mesmo tempo.
- Conserto: `Map<key, Promise>` em voo.

---

## Etapa 3 — Loop de render 3D (CPU por quadro)

Arquivo único: `apps/prismview-studio/src/components/Scene3DLayer.tsx`

### 3.1 Lixo alocado todo quadro
- ~699 `normalizeScene3DObjects(cfg)` todo quadro → cachear quando `config` muda (ref).
- ~806–826 `filter` de bindings duas vezes todo quadro → separar cursor/sinal uma vez por mudança de config.
- ~871–875 5 `new Map()` + ~877–904 closures novas todo quadro → Maps fixas no `engineRef`, `clear()` no começo.
- ~880 `q.clone()` e ~896 `new Vector3()` → pool reusado.
- ~833 string com `toFixed` pra chave do raycast → comparar números direto.
- ~856 `new Date()` todo quadro → só quando dia/noite ligado e a cada 30 s.
- Resultado: menos GC, menos travada, menos CPU em 60–144 FPS.

### 3.2 Trabalho repetido todo quadro
- ~855–864 `applyDayNightLighting` / `applySceneBackground` todo quadro → só quando hora muda (intervalo já existe ~1648) ou config muda.
- ~777 + ~800 `camera.fov` + `updateProjectionMatrix()` todo quadro → só quando fov/aspect muda.
- ~1120–1121 exposure/envIntensity todo quadro → só na mudança.

### 3.3 `preserveDrawingBuffer: true`
- ~373
- Problema: força cópia extra do framebuffer todo quadro. Mais VRAM, menos FPS.
- Conserto: vidro lê canvas síncrono em `onFrameReady` (logo após `render`) → pode ser `false`. Testar vidro sobre 3D depois do 1.1.

### 3.4 Acorda 3D a cada telemetria sem precisar
- ~1617–1645 deps `cpuPercent`, `mediaPlaying`
- Problema: telemetria chega a cada 1,5 s → quadro extra mesmo sem binding de CPU/mídia. Em FROZEN/ULTRA isso é desperdício puro.
- Conserto: wake só se existe binding ativo `CPU_USAGE` / `MEDIA_PLAYING`.

### 3.5 Componente re-renderiza toda telemetria
- ~1314–1317, ~1567, ~1601, ~1659 (`JSON.stringify` de dynamicTextures todo render)
- Problema: pai re-renderiza a cada 1,5 s (telemetria) e 1 s (relógio) → recalcula todas as chaves.
- Conserto: `React.memo(Scene3DLayer)` + callbacks estáveis no pai (`UnifiedCompositor.tsx` ~1191–1206 cria arrow nova todo render) + `useMemo` nas chaves.

### 3.6 MSAA caro em tela grande
- ~369 `antialias: true` + dpr até 1,5
- Conserto: `antialias` só quando dpr efetivo < 1,25. Em ECO/ULTRA criar sem MSAA (precisa recriar renderer na troca, ou decidir no boot pelo estado de energia).

### 3.7 Raycast hover em toda malha
- ~527–535 `intersectObjects(meshes)` sem BVH
- Conserto: checar `Box3` do pivot antes; modelo grande → `three-mesh-bvh` (opcional).

---

## Etapa 4 — Vidro óptico e compositor 2D

### 4.1 Loop de vídeo no ritmo do monitor
- Arquivo: `apps/prismview-studio/src/components/UnifiedCompositor.tsx` (~554–583)
- Problema: rAF a 144 Hz faz `texImage2D` do vídeo inteiro + blur + todos os vidros, mesmo vídeo sendo 30 FPS.
- Conserto: `video.requestVideoFrameCallback` → só quando quadro novo. Respeitar `powerCaps.fpsCap`.

### 4.2 Cópia WebGL → canvas 2D por vidro
- Arquivo: `apps/prismview-studio/src/utils/opticalGlassWebGL.ts` (~929–938)
- Problema: cada vidro = canvas 2D próprio + `drawImage` todo quadro. Mais RAM, mais cópia.
- Conserto: `canvas.getContext("bitmaprenderer")` + `transferToImageBitmap()` (zero cópia), ou um canvas overlay único pra todos os vidros.

### 4.3 Sobel do glifo na thread principal
- Arquivo: `apps/prismview-studio/src/utils/glassLens.ts` (~595–670)
- Problema: 2× `getImageData` + loop em até 1 milhão de pixels a cada texto novo (relógio = todo segundo). Engasga UI.
- Conserto: calcular normal no shader a partir do alpha (GPU), ou `OffscreenCanvas` em Worker. Combinar com atlas (2.4).

### 4.4 setState no caminho quente
- Arquivo: `apps/prismview-studio/src/components/OpticalGlassSurface.tsx` (~165–195 `drawFrame`)
- Problema: `setWebglOk` chamado todo tick de vídeo/cursor.
- Conserto: guardar `ok` em ref, só `setState` quando muda de verdade (checar ref antes).

### 4.5 Backdrop recalculado por vidro
- Arquivo: `apps/prismview-studio/src/components/UnifiedCompositor.tsx` (~772–910 `resolveBackdropForTopIndex`, ~998)
- Problema: cada vidro varre camadas de baixo e roda `evaluateFormula` de novo, todo render.
- Conserto: calcular uma vez por render (array por `topIndex`), reusar.

### 4.6 Compositor inteiro re-renderiza
- Arquivo: `apps/prismview-studio/src/components/UnifiedCompositor.tsx` (~937 `renderLayer`, ~696 `setSnapGuide`)
- Problema: telemetria/relógio/arrastar = árvore toda refeita, fórmulas todas avaliadas. `setSnapGuide` todo pointermove mesmo igual.
- Conserto: extrair `<LayerView>` com `React.memo`, passar só o que a camada usa. `setSnapGuide` só se x/y mudou.

### 4.7 Miniaturas do Hub seguem telemetria
- Arquivo: `apps/prismview-studio/src/App.tsx` (~1022, ~1155, ~1384)
- Problema: todo card re-renderiza a cada 1,5 s.
- Conserto: miniatura recebe `DEFAULT_TELEMETRY` fixo + `React.memo`. (Junto com 2.1.)

---

## Etapa 5 — Bundle e processos WebView (RAM por janela)

### 5.1 Janela wallpaper/widget carrega Studio inteiro
- Arquivo: `apps/prismview-studio/src/App.tsx` (~1–50 imports estáticos, ~1480–1487 rota por `?monitor` / `?id`)
- Problema: cada monitor e cada widget baixa e parseia `StudioEditor` (~2k linhas), `StudioInspector` (~3,5k), `framer-motion`, `lucide-react`. Tudo na RAM de cada WebView.
- Conserto: `React.lazy(() => import("./components/StudioEditor"))` e Hub em chunk próprio. Melhor: entrada separada `desktop.html` + `src/desktop.tsx` só com `UnifiedCompositor`. Ajustar `apps/prismview-studio/vite.config.ts` (`build.rollupOptions.input`) e URL em `apps/prismview-studio/src-tauri/src/lib.rs` (~1349, ~1754).

### 5.2 Um WebView por widget
- Arquivo: `apps/prismview-studio/src-tauri/src/lib.rs` (~1744–1760 `WebviewWindowBuilder` widget)
- Problema: 5 widgets = 5 renderers (dezenas de MB cada).
- Conserto: avaliar 1 janela transparente por monitor hospedando todos os widgets daquele monitor. Mínimo: aviso no Hub sobre custo.

---

## Etapa 6 — Backend Rust (CPU ociosa)

### 6.1 Loop de fundo aloca e trava muito
- Arquivo: `apps/prismview-studio/src-tauri/src/lib.rs` (~2458–2880 `spawn_background_monitors`)
- Problema: a cada 24 ms (modo rápido): 3+ `state.lock()`, clone de `monitors`, `monitor_visibility`, labels `String`, `reactive_sensors`.
- Conserto: guardar "geração" do estado; só reconstruir alvos quando algo muda. `Arc<str>` pros labels.

### 6.2 `outer_position()` por widget a 40 Hz
- Arquivo: `apps/prismview-studio/src-tauri/src/lib.rs` (~2769–2773, também ~2578–2582)
- Problema: cada chamada pula pra thread principal. N widgets × 40/s.
- Conserto: cachear posição; atualizar em `WindowEvent::Moved`.

### 6.3 Tick rápido mesmo com mouse parado
- Arquivo: `apps/prismview-studio/src-tauri/src/lib.rs` (~2868–2872)
- Conserto: mouse parado > 1 s e sem áudio alto → subir pra 100 ms. Mexeu → 24 ms de novo.

### 6.4 Visibilidade por polling
- Arquivo: `apps/prismview-studio/src-tauri/src/desktop_host.rs` (~607 `compute_monitors_visibility`), chamado a cada 500 ms em `lib.rs` (~2515)
- Conserto: `SetWinEventHook` (`EVENT_SYSTEM_FOREGROUND`, `EVENT_OBJECT_LOCATIONCHANGE`) e manter polling lento (2 s) só de reserva.

### 6.5 FFT recalcula janela Hann
- Arquivo: `apps/prismview-studio/src-tauri/src/reactive_sensors.rs` (~121–125)
- Problema: 256 `cos()` 50×/s.
- Conserto: tabela estática (`OnceLock<[f32; 256]>`). Barato, mas grátis.

---

## Etapa 7 — Código morto e limpeza

### 7.1 Crates fora do workspace
- Arquivos: `Cargo.toml` (raiz, `members` só tem `apps/prismview-studio/src-tauri`), `crates/prism-core/`, `crates/prismview-engine/`
- Problema: não compila, não roda. Confunde manutenção.
- Conserto: apagar ou mover pra `archive/`. Se voltar a usar: `crates/prismview-engine/src/video.rs` (~241–290) troca BGRA→RGBA pixel a pixel na CPU todo quadro → usar textura `Bgra8Unorm` direto ou handle D3D11 compartilhado.

### 7.2 Pasta `target/` gorda
- Caminho: `target/`
- Muitas builds incrementais duplicadas. `cargo clean` libera disco (não RAM).

### 7.3 Segurança (risco, não perf)
- Arquivo: `apps/prismview-studio/src-tauri/tauri.conf.json` (`"csp": null`, `assetProtocol.scope: ["**"]`)
- Disco inteiro legível via `asset:`. SVG já é sanitizado, mas CSP nula abre porta. Fechar scope pra pasta PrismView + pastas que usuário escolheu.

### 7.4 `CLAUDE.md` vazio
- Caminho: `CLAUDE.md` — escrever regras do projeto (dispose 3D, sem alocar no render loop, etc.).

---

## Etapa 8 — Medir antes e depois

- HUD 3D do Studio (`onStats` em `Scene3DLayer.tsx` ~621): `geometries` e `textures` devem voltar ao número base depois de adicionar e remover objeto / trocar capa 10×.
- DevTools do WebView2 (F12): Memory → heap snapshot antes/depois de abrir/fechar Studio; procurar `HTMLCanvasElement`, `WebGLRenderer`, `Texture` sobrando.
- Gerenciador de Tarefas → processos `msedgewebview2.exe`: RAM por janela wallpaper antes/depois da Etapa 5.
- Performance tab: gravar 10 s de wallpaper com relógio + vidro. Meta: sem long task > 16 ms por segundo (Etapa 4.3), GC raro (Etapa 3.1).
- Rust: CPU do processo PrismView com mouse parado. Meta: ~0% (Etapa 6).

---

Resumo caveman: arrumar vidro sobre 3D e blob primeiro. Depois matar renderer por miniatura e loader duplicado. Depois parar de alocar no loop. Depois cortar bundle das janelas desktop. Rust por último.
