# Prismlock: Identidade, Navegação e Movimento (Android / Kotlin + Compose)

Documento irmão de `PRISMVIEW_IDENTIDADE_MOVIMENTO.md`. Seções 0, 1 e as etapas 1–7 são **iguais em conceito**; a seção 3 traz o que é **específico do Prismlock**.

## 0. Princípios (idênticos nos dois apps)

**Identidade:** estúdio de vidro óptico + fórmulas `ƒx`. Monocromático carbono, vidro fosco, sem cor de acento fixa. Prismlock (Android) e PrismView (Windows) são irmãos: mesmo vocabulário, mesmos tokens, mesmo pacote `.prism`.

**Vocabulário único**

| Termo | Significado |
|---|---|
| **View** | Uma tela completa (wallpaper/lock) ou um widget, como aparece no Hub. Substitui "tema", "tela", "projeto" na UI. |
| **Hub** | Tela inicial. Principal = **Carrossel**; alternativa = **Grid**. |
| **Studio** | Tela de edição de uma View. |
| **Aplicar** | Definir a View como wallpaper/lock ativo ou fixar o widget. |
| Abas do Hub | `Wallpapers` · `Widgets` · `Cenas 3D` (PrismView) / `Telas` · `Widgets` (Prismlock) |

**Tokens de movimento: fonte única** `docs/motion.tokens.json`, consumida por TS (PrismView) e Kotlin (Prismlock) via script de geração. Nenhum número de duração/easing fora dele.

```json
{
  "duration": { "instant": 90, "fast": 160, "base": 280, "slow": 420, "hero": 650, "intro": 5200 },
  "easing": {
    "emphasized": [0.2, 0.0, 0.0, 1.0],
    "decelerate": [0.05, 0.7, 0.1, 1.0],
    "accelerate": [0.3, 0.0, 0.8, 0.15]
  },
  "spring": {
    "tactile":  { "stiffness": 420, "damping": 34, "mass": 1 },
    "carousel": { "stiffness": 260, "damping": 30, "mass": 1 },
    "expand":   { "stiffness": 190, "damping": 26, "mass": 1 }
  },
  "blur":  { "enter": 12, "exit": 8 },
  "scale": { "enter": 0.96, "carouselSide": 0.84 },
  "gesture": { "commitDistance": 0.22, "commitVelocity": 900, "rubberBand": 0.35 }
}
```

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
- Criar `docs/motion.tokens.json` + geradores (TS e Kotlin).
- Unificar paleta: `#000000` / `#0D0D0F` / `#141418` / `#A1A1AA` / `#27272A`. *(PrismView hoje mistura `#060608`/`#0d0d10` com `#000`: escolher um conjunto e aplicar em `tailwind.config.js` + `prism.css`.)*
- Renomear na UI: "tema/tela/projeto" → **View**; abas e botões conforme o vocabulário.
- Atualizar `CLAUDE.md` / `AGENTS.md` com a seção "Movimento" apontando para este documento.
- Suite compartilhada de casos de fórmula (`fx_parity_cases.json`) também consumida pelo Kotlin.

---
## 3. Específico do Prismlock

**Stack:** Kotlin · Jetpack Compose · `HorizontalPager` · AGSL · processo isolado `:wallpaper` · DataStore.

| Etapa | Como implementar no Prismlock |
|---|---|
| 1 Expandir | `SharedTransitionLayout` + `sharedBounds` (Compose ≥ 1.7; confirmar versão em `libs.versions.toml`) **ou** overlay próprio com `Animatable<Float>` de progresso `p` e `graphicsLayer`. Overlay no topo da árvore (acima do Pager, **sem** `clip`). Gesto: `detectVerticalDragGestures` já existente → traduzir para `p` com rubber-band (fim do corte ao arrastar para baixo). Studio (`StudioScreen`) pré-composto em `p > 0.08` com `Modifier.alpha(0f)` até o 1º frame; cross-fade do snapshot. Voltar: **Predictive Back** (`PredictiveBackHandler`) dirigindo o mesmo `p`. Cima = `onEdit…` (expande e vira o Studio); baixo = `onApply…`: expande, faz fade + `RenderEffect` blur e, no início do fade, `startActivity` com `ACTION_CHANGE_LIVE_WALLPAPER` (reaparece no carrossel se cancelar); hápticos (`HapticFeedbackType`) no cruzamento do limiar. |
| 2 Carrossel | Manter `HorizontalPager` (`ThemeHubScreen.kt`), `beyondViewportPageCount = 1`. Transformações **só** em `Modifier.graphicsLayer { … }` lendo `pagerState.currentPageOffsetFraction` **dentro do lambda** (leitura adiada, sem recomposição). `contentPadding` para mostrar vizinhas; `pageSpacing` negativo + escala. `flingBehavior` com snap (`PagerDefaults.flingBehavior`) calibrado pela mola `carousel`. Não alterar o Hz nativo da tela. |
| Viva × snapshot | `HubPreviewCache` (420 px + ambient 72×156 pré-desfocado) como fonte dos vizinhos; render real só na página central (`derivedStateOf` para `settledPage`). `Bitmap.recycle()`/`LruCache` ao sair de ±2. |
| 3 Intro | Já existe `PrismHeroScreen` (AGSL). Manter a duração atual (5,2 s, sem encurtar); apenas tornar pulável e ler `intro`/`hero` dos tokens. Só no cold start com a Activity em primeiro plano. |
| 4 Páginas | `AnimatedContent`/`Crossfade` com `graphicsLayer { renderEffect = BlurEffect(...) }` (API 31+) **apenas durante** a transição; abaixo disso, fade. O fade + blur de 420 ms do Hub (Telas ↔ Widgets) passa a usar os tokens `slow`/`blur`. |
| 5 Grid | `LazyVerticalGrid` + `Modifier.animateItem()`; morph via `SharedTransitionLayout` mantendo `key = view.id`. Preferência no DataStore. Botão canto superior direito. |
| 6 Micro | `Modifier.pressScale()`, `BlurText` (`RenderEffect`), `spotlight` por `drawWithCache` (sem alocar no draw). |
| 7 Tokens | `motion.tokens.json` → `PrismMotion.kt` (Gradle task/`generateMotionTokens`). Cores já em `PrismTheme.kt` (referência para o PrismView). Atualizar `AGENTS.md`. |

**Cuidados próprios do Android**
- 120 Hz: todo desenho de animação em `graphicsLayer`/`drawWithCache`; **proibido** alocar `Path/Paint/Brush` dentro de draw.
- `:wallpaper` continua isolado: nada desta UI (Compose, intro) é carregado lá.
- "Reduzir movimento": respeitar `Settings.Global.ANIMATOR_DURATION_SCALE == 0` → sem blur/intro, fade `fast`.
- Térmico/energia: em modo economia de bateria, desligar blur e reduzir a janela viva a 1 View.
- Edge-to-edge e gesto de voltar do sistema não podem conflitar com o gesto vertical da View (zona morta de borda ~24 dp).
- Verificação: Macrobenchmark (`FrameTimingMetric`) no fluxo Hub → arrastar → Studio; Baseline Profile; `.\gradlew.bat assembleDebug testDebugUnitTest`.

**Plano de entrega sugerido (1 etapa por sessão):** 7 → 2 → 1 → 3 → 4 → 5 → 6.
