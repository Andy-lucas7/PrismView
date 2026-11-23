# PrismView: Identidade, Navegação e Movimento (Windows / Tauri + React)

Documento irmão de `PRISMLOCK_IDENTIDADE_MOVIMENTO.md`. Seções 0, 1 e as etapas 1–7 são **iguais em conceito**; abaixo de cada etapa, a seção 3 traz o que é **específico do PrismView**.

## 0. Princípios (idênticos nos dois apps)

**Identidade:** estúdio de vidro óptico + fórmulas `ƒx`. Monocromático carbono, vidro fosco, sem cor de acento fixa. Prismlock (Android) e PrismView (Windows) são irmãos: mesmo vocabulário, mesmos tokens, mesmo pacote `.prism`.

**Vocabulário único**

| Termo | Significado |
|---|---|
| **View** | Uma tela completa (wallpaper/lock) ou um widget, como aparece no Hub. Substitui "tema", "tela", "projeto" na UI. |
| **Hub** | Tela inicial. Principal = **Carrossel**; alternativa = **Grid**. |
| **Studio** | Tela de edição de uma View. |
| **Aplicar** | Definir a View como wallpaper/lock ativo ou fixar o widget. |
| Abas do Hub | `Wallpapers` · `Widgets` · `Templates` (PrismView) / `Telas` · `Widgets` (Prismlock) |

**Tokens de movimento e paleta: fonte única** `docs/motion.tokens.json` (durações, easings, springs, blur, escala, gesto e cores). Editar **só** ali. PrismView: `npm run gen:motion` gera `src/engine/motionTokens.gen.ts`, e `npm test` falha se o gerado ficar defasado ou se `prism.css`/`tailwind.config.js` divergirem da paleta. Prismlock: gerar `PrismMotion.kt` a partir do mesmo JSON e adicionar a mesma checagem (a fazer).

**Regras de performance (inegociáveis)**
1. Orçamento de quadro = `1000/Hz` do monitor (6,9 ms a 144 Hz, 8,3 ms a 120 Hz). Nunca fixar 60 fps: seguir o refresh real.
2. Integração por `dt` (física independente de FPS): o "peso" da animação é o mesmo a 60/120/144 Hz.
3. Só animar `transform`, `opacity`, `clip-path` / `graphicsLayer`. Nada de layout/medição por quadro.
4. Zero alocação por quadro (reusar objetos/arrays). Zero estado de UI por quadro (valor animado lido na camada de desenho, não via recomposição/re-render).
5. Blur só em transições curtas e nunca em cartões vivos: usar **imagem pré-desfocada** com cross-fade (mesma técnica do `HubPreviewCache`).
6. Só a View central é "viva" (render real). Vizinhas e grid usam **snapshot** em cache; descartar com `dispose()`/`recycle()` ao sair da janela de ±2.
7. Respeitar modo de energia e "reduzir movimento": blur → fade simples; spring → tween `fast`; intro pulada.
8. Nada anima durante render pesado em tela cheia (Studio 3D ativo, jogo em foreground).

**Critérios de aceite globais:** p95 do tempo de quadro ≤ orçamento; 0 quadros > 2× orçamento em 30 s de uso; CPU/GPU em repouso ≈ 0%; nenhuma View "pisca" ou aparece cortada em qualquer ponto de qualquer gesto.

---

## 1. Fluxo View → Studio (conceito comum às etapas 1–2)

```
Hub (Carrossel/Grid)
   │ toque/clique numa View  ──► EXPANDIR até preencher a tela
   ▼
[snapshot da View] escala do retângulo do card → retângulo da tela inteira
   │  em paralelo: Studio monta "por trás" (pré-aquecido)
   ▼
Handoff: assim que o Studio entrega o 1º quadro, cross-fade snapshot → Studio real (≤ `fast`)
```

- A transição é **uma única animação de progresso `p ∈ [0,1]`**: raio dos cantos, retângulo, opacidade do chrome do Hub, escurecimento do fundo, tudo derivado de `p`. Gesto, clique e botão **voltar** usam o mesmo `p` (voltar = `p` de 1→0, reversível a qualquer instante).
- O Studio **nunca** é construído no meio da animação principal: montar em frame ocioso, ou antes (ao pressionar/hover/arrastar > 8%).
- O retângulo de origem é medido **uma vez** no início (não por quadro).

---
## 2. Etapas (da mais complexa/pesada à mais simples)

> Ordem de leitura = consumo estimado de esforço/tokens (maior → menor), ajustada ao contexto. **Ordem de execução recomendada:** 7 → 2 → 1 → 3 → 4 → 5 → 6 (tokens primeiro; o carrossel antes da expansão, pois esta parte do retângulo do card).

### Etapa 1: Expandir View → Studio + gesto de editar/aplicar ★ pesada
**Objetivo:** ao escolher uma View, ela **cresce até preencher a tela e se torna o Studio**. Mesma animação para clique e gesto.

- Implementa o fluxo da seção 1. Retângulo origem = card atual (com cantos arredondados e sombra) → tela cheia (cantos 0 no final).
- **Gesto vertical no card central** (um único `p` contínuo; Prismlock):
  - **Arrastar para cima e soltar = Editar:** o card cresce até preencher a tela e **vira o Studio** (handoff abaixo).
  - **Arrastar para baixo e soltar = Aplicar:** o card cresce até preencher a tela e, com **fade de transparência + blur** (`slow`), desaparece. Ao começar o fade, dispara a tela do Android de aplicar o wallpaper animado (`WallpaperManager.ACTION_CHANGE_LIVE_WALLPAPER` para o `PrismWallpaperService`; widget = fluxo de fixar), de modo que a animação cobre a latência do sistema. Se o usuário cancelar e voltar ao app, a View reaparece no carrossel (fade-in + escala inversa).
  - Confirmar ao passar de `commitDistance` (22% da altura) **ou** `commitVelocity`; senão volta com `spring.expand`.
  - No PrismView o equivalente é o botão/atalho **Aplicar** (sem gesto vertical): mesma animação de expandir + fade/blur e depois aplica pelo motor nativo, com toast de confirmação.
- **Correção do bug atual ("fica cortado ao arrastar muito para baixo"):**
  1. O card **não é transladado dentro de um container com `clip`**. O arraste move o snapshot numa camada **sem clip**, acima do Hub (overlay em tela cheia).
  2. O deslocamento passa por **rubber-band**: `d' = c·d / (c + d)` (limite assintótico), nunca passa de ~35% da altura; o resto vira escala + brilho, não deslocamento.
  3. Cantos e sombra interpolados por `p`; sem recortes por `overflow: hidden` do pai.
  4. Reversível: soltar antes do limiar anima `p` de volta com a velocidade atual (sem "teleporte").
- **Handoff:** Studio pré-aquecido a partir de `p > 0.08`; cross-fade no 1º quadro pronto. Se o Studio demorar, o snapshot segue preenchendo a tela (nunca tela preta).
- **Botão voltar / gesto de voltar:** mesma animação invertida (Studio → snapshot → posição do card, que pode ter mudado de lugar no carrossel/grid).
- **Aceite:** em qualquer ponto do arraste a View está inteira (nenhum corte), soltar reverte suavemente, 120/144 Hz sem engasgo, back reversível.

### Etapa 2: Carrossel de Views (principal) ★ pesada
**Objetivo:** carrossel central, fluido, no FPS do monitor, com inércia natural e snap.

- **Layout:** View ativa grande ao centro (`scale 1`), vizinhas menores (`carouselSide`), levemente rotacionadas em Y e com opacidade decrescente; profundidade (z) pela distância ao centro. Largura do cartão respeita proporção real da View (wallpaper vertical/horizontal, widget 2×2/4×2…).
- **Física:** posição contínua `x` (em unidades de card), mola `spring.carousel` + inércia pelo `velocity` do gesto; **snap** ao card mais próximo com projeção de inércia (`x + v·τ`). Resistência elástica (`rubberBand`) nas pontas, nunca corte seco.
- **Entradas:** arrastar/touch, roda do mouse/trackpad (horizontal e vertical→horizontal), teclado ←/→, clique em vizinha (anima até ela), setas visíveis.
- **Virtualização:** renderizar só ±2 cards. Cada card é função pura de `(x − índice)` → `translateX/scale/rotateY/opacity`.
- **Viva × snapshot:** só o card central tem render real (e pausa quando a View está parada/oculta); vizinhos = snapshot + versão pré-desfocada para o cross-fade lateral. Troca de "central" faz handoff snapshot↔viva sem salto.
- **Fundo ambiente:** cor/blur do wallpaper da View central vaza suavemente atrás do carrossel (imagem pré-desfocada pequena, cross-fade `slow`).
- **Indicadores:** pontos/paginação mínima + nome da View com blur-reveal (Etapa 6).
- **Aceite:** 0 re-render/recomposição por quadro; arraste acompanha o dedo/mouse 1:1 sem atraso; solta e para exatamente em um card; 100+ Views sem queda de FPS.

### Etapa 3: Intro animada ★ média-pesada
**Objetivo:** abertura holográfica igual nos dois apps (existe no Prismlock em AGSL; portar para o PrismView).

- Mesmo roteiro: fundo preto → feixes coloridos convergem → "pool" de luz → revelação radial (`hero`, 650 ms) para o Hub. Duração total `intro` = **5,2 s, a mesma do Prismlock; não encurtar nem acelerar**. O PrismView ainda não tem intro: a etapa é um port fiel.
- **Pulável** (toque/tecla/clique), executada **uma vez por sessão**, **não** executa em abertura em segundo plano/minimizado nem sob "reduzir movimento".
- Shader de referência = o AGSL do Prismlock, portado 1:1 para GLSL (mesma timeline, cores e curvas); parâmetros por `uTime`, vindo de um relógio, não de contador de frames.
- **Aceite:** primeiro quadro < 100 ms após abrir; o Hub já carregando por trás; ao terminar, o shader é liberado (sem custo em repouso).

### Etapa 4: Transição de páginas e abas ★ média
**Objetivo:** toda troca de aba/página tem a mesma coreografia moderna.

- Saída (`base`, `accelerate`): opacidade 1→0, blur 0→`blur.exit`, translate −8 px, scale 1→0,98.
- Entrada (`slow`, `decelerate`): opacidade 0→1, blur `blur.enter`→0, translate +12 px→0, scale `scale.enter`→1, com **stagger** (30–45 ms) nos filhos (cards, títulos).
- **Corte fluido** opcional entre abas: revelação por `clip-path` (inset/círculo) a partir do botão da aba, em vez de fade puro.
- Direção coerente: avançar = entra da direita, voltar = da esquerda (índice da aba decide).
- Blur ligado só durante a animação e desligado ao fim (sem `will-change` permanente).
- Degradação: modo ECO/ULTRA/reduzir movimento → só fade `fast`.

### Etapa 5: Modo Grid + botão de alternância ★ média
**Objetivo:** o Hub tem 2 visualizações: **Carrossel (padrão)** e **Grid**.

- Botão discreto no canto superior direito (ícone alterna carrossel/grid), com a preferência persistida.
- **Morph compartilhado:** cada View mantém identidade entre as visualizações (mesmo elemento animando posição/tamanho: *layout animation*), com `spring.expand`. Sem remontar os cards.
- Grid usa só snapshots (nenhum render vivo); apenas a View sob foco/hover pode "acordar".
- Clicar numa View no Grid usa a **mesma expansão da Etapa 1**, partindo do retângulo dela.
- Rolagem do grid em FPS do monitor, com virtualização.

### Etapa 6: Micro-interações ★ leve
- Hover/press: escala 1→1,03 / 0,97 com `tactile`; spotlight (gradiente que segue o ponteiro) nos cards.
- Texto com **blur-reveal** (títulos e nome da View), listas com stagger, toasts/diálogos já em `spring.tactile` (padronizar nomes).
- Botões primários: feedback de pressão + brilho de borda `GlassBorderStrong`.
- Inspiração de padrões do react-bits; implementar como componentes próprios (verificar licença antes de copiar qualquer código).

### Etapa 7: Tokens, vocabulário e docs ★ simples (fazer primeiro)
- **Já feito no PrismView:** `<MotionConfig reducedMotion="user">` na raiz do `App.tsx` e `@media (prefers-reduced-motion)` no `prism.css`. Falta o resto abaixo.
- Criar `docs/motion.tokens.json` + geradores (TS e Kotlin).
- Unificar paleta: `#000000` / `#0D0D0F` / `#141418` / `#A1A1AA` / `#27272A`. *(PrismView hoje mistura `#060608`/`#0d0d10` com `#000`: escolher um conjunto e aplicar em `tailwind.config.js` + `prism.css`.)*
- Renomear na UI: "tema/tela/projeto" → **View**; abas e botões conforme o vocabulário.
- Atualizar `CLAUDE.md` / `AGENTS.md` com a seção "Movimento" apontando para este documento.
- Suite compartilhada de casos de fórmula (`fx_parity_cases.json`) também consumida pelo Kotlin.

---
## 3. Específico do PrismView

**Stack:** Tauri 2 (WebView2/Chromium) · React 18 · framer-motion 11 · Tailwind 3 · Three 0.187 · Vite 6.

| Etapa | Como implementar no PrismView |
|---|---|
| 1 Expandir | Overlay `position: fixed` em `#root` (fora de qualquer `overflow:hidden`). Animar `transform` do snapshot (FLIP: First/Last/Invert/Play) + `clip-path: inset(... round Rpx)` para cantos. `framer-motion` `layoutId` entre card e Studio. Transformar o `if (editing) return` do `App.tsx` em `AnimatePresence` com Studio montado atrás (`display` oculto, `visibility`, não `unmount`) a partir de `p > 0.08`. Gesto com Pointer Events (`setPointerCapture`) e roda do mouse opcional. |
| 2 Carrossel | Componente `ViewCarousel` em `src/components/`. Posição em `useMotionValue` + `useSpring` (ou loop `requestAnimationFrame` próprio com `dt`); cada card lê `useTransform(x, …)` → `transform/opacity` **sem re-render do React**. `requestAnimationFrame` já segue o refresh do monitor (144 Hz inclusive); integrar por `dt`, limitar por `powerCapsFor(...).fpsCap`. Roda do mouse via listener `wheel` passivo=false; touchpad com inércia própria. |
| Viva × snapshot | Hoje cada `HubThumbnail` pode custar 1 WebGL (limite de contextos do Chromium). Só o card central usa a cena; vizinhos = `ImageBitmap`/`<img>` gerado 1× (cache por `id + versão`), descartar com `close()`. Pré-desfocado 72×156 como no Prismlock. |
| 3 Intro | Canvas WebGL2 puro (≈100 linhas, sem Three) ou `three-core` já em chunk; GLSL portado do AGSL. Só na janela `main`, **não** em `desktop.tsx` (widget standalone) nem com `--minimized`. Liberar contexto no fim (`loseContext`). |
| 4 Páginas | `PageTransition` com `AnimatePresence mode="wait"` nas abas do Hub (`hubTab`, hoje render condicional) e no Hub↔Studio. Blur com `filter` só durante a animação. Integrar com `powerGovernor`: AUTO/ECO/ULTRA/FROZEN ⇒ fade simples. |
| 5 Grid | `layout` do framer-motion (`LayoutGroup`) com `layoutId={view.id}`. Persistir em `localStorage` (`prismview.hubView.v1`). |
| 6 Micro | Componentes `BlurText`, `SpotlightCard`, `Stagger` em `src/components/motion/`. |
| 7 Tokens | `docs/motion.tokens.json` → `src/engine/motion.ts` (script `npm run gen:motion`). Consolidar paleta em Tailwind + `prism.css`. |

**Cuidados próprios do Windows/Tauri**
- WebView2 compõe na GPU: manter animações em camadas compostas (`transform/opacity`); `will-change` só durante a animação.
- Janela do Hub oculta/minimizada ⇒ pausar rAF (`visibilitychange` + `prism://visibility-changed`).
- Jogo/app em tela cheia ⇒ Hub não anima (já existe pausa do motor; estender ao Hub).
- `desktop.html`/widgets **não** carregam framer-motion nem estes componentes (manter leve).
- Teclado: ←/→ no carrossel, Enter = Editar, Esc = voltar do Studio, `G` = alternar Grid.
- Verificação: DevTools → Performance (frames > orçamento), `npm run build` e checar tamanho do chunk principal após cada etapa.

**Pré-requisito novo: snapshots das miniaturas.** `HubThumbnail` (`App.tsx`) ainda é um `UnifiedCompositor` completo por card (480×270, `paused`), ou seja, 1 WebGL cada. Carrossel, grid e expansão dependem de um cache de `ImageBitmap` por `id + versão`. Fica como **Etapa 2a**, dentro do carrossel, e vem antes dele.

**Estado atual do código (auditado no `main` de 23/11/2026, 18:18)**

| Item | Estado |
|---|---|
| Movimento reduzido (`MotionConfig` + CSS) | Feito |
| Toast/diálogos com `SPRING_TACTILE` + `AnimatePresence`, `useDialogFocus` | Feito |
| Módulos separados (`UnifiedCompositorParts`, `InspectorControls`, `Scene3DLayerParts`, `types/prism*`) | Feito: componentes de movimento vão em `src/components/motion/`, sem inflar o `App.tsx` |
| `motion.ts`, tokens, paleta única | Pendente (Etapa 7) |
| Snapshots de miniaturas | Pendente (2a) |
| Carrossel | Pendente (2); o Hub é grid de 3 colunas nas abas `WALLPAPERS`, `WIDGETS`, `TEMPLATES` |
| Expandir View → Studio | Pendente (1); `if (editing)` é corte seco |
| Transição de abas | Pendente (4); `hubTab ===` é render condicional |
| Grid como modo alternativo | Pendente (5); não há modo de visualização |
| Intro | Pendente (3) |

**Aplicar já é instantâneo no PrismView** (commit `2363599`): o fade + blur da Etapa 1 roda **em paralelo** ao aplicar, sem atrasar o motor nativo.

**Plano de entrega sugerido (1 etapa por sessão):** 7 → 2a → 2 → 1 → 4 → 5 → 3 → 6.
*(Intro depois do grid porque o PrismView ainda não tem nenhuma; o ganho visual de Hub/Studio vem antes. Se preferir o ganho rápido, a Etapa 4 é independente e pode vir logo após a 7.)*
