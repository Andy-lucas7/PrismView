# Plano: bugs e evoluções do PrismView

Documento autocontido para abrir uma sessão nova. Leia também `CLAUDE.md` (regras e mapa do código) e `docs/PRISMVIEW_IDENTIDADE_MOVIMENTO.md` (tokens e movimento já feitos).

## Decisões já tomadas
- **Fonte da interface:** Outfit Light (300). Samsung Sharp Sans não entra (licença).
- **Studio:** um painel só, à **direita**: **propriedades em cima, camadas embaixo**, divisória arrastável entre os dois (proporção salva em `localStorage`). O canvas ocupa todo o resto da largura, o maior possível.
- **Meta de UX:** completo como o Canva (alterar tudo de qualquer forma, organizado e intuitivo), sempre **leve e otimizado**: nada alocado por quadro, nada re-renderizando à toa, menus e painéis só montam quando abertos.
- **Idioma:** tudo em português.
- **Git:** desenvolver na branch indicada na sessão, abrir PR para `main` só quando pedirem.

## Lotes (ordem de execução)

| Lote | Etapas | Por que juntas / sozinhas |
|---|---|---|
| **A** | 1 + 2 + 3 | Carrossel, transições grandes, fontes e menus. UI e animação, arquivos diferentes, baixo risco. |
| **B** | 4 + 9 | 4 = bugs de seleção e de escala texto × motor (Rust + TS), exige verificação. 9 = Ajuda no app, não toca nos mesmos arquivos. |
| **C** | 6 | Sozinha. Leva o alongamento vetorial para o Studio via WASM. Mexe em fontes e render, é a mais arriscada. |
| **D** | 7 | Sozinha. Reorganiza o Studio inteiro em um painel só. Mexe em quase tudo. |
| **E** | 5 + 8 | Multiseleção e menu de botão direito dependem do painel novo e da mesma lógica de seleção. Só depois da 7. |

Observação: o lote A pode ser dividido em dois (1 primeiro; 2 + 3 depois) se quiser mais segurança.

---

## Etapa 1: Carrossel
Arquivos: `src/components/hub/ViewCarousel.tsx`, `HubLayout.tsx`, `src/components/HubThumbnail.tsx`, `src/engine/hubLiveBudget.ts`, `hubStillCapture.ts`.

Itens:
1. **Remover a luz do mouse** nos itens do carrossel (feia). Hoje é o overlay `spotlight` + `glow` no `Slot` (e o tilt `tx/ty` junto). Remover o spotlight; decidir se mantém o tilt leve ou tira tudo.
2. **Pulinhos nos itens laterais a cada ~2 s.** Ainda sem causa. Candidatos a verificar: `HubStillWarmer` capturando stills e notificando `hubStills` (todos os `HubThumbnail` re-renderizam via `useSyncExternalStore`), o relógio mock, `onThumbFrame` com debounce de 600 ms refazendo `captureStill`, trocas `still` ↔ `full` mudando a altura/posição do card, o timer de 1850 ms das frases... Reproduzir medindo o `getBoundingClientRect` dos cards laterais por quadro.
3. **Trilho de baixo interativo:** clicar vai para a posição, arrastar o polegar move o carrossel (usar a mesma física `goTo`/`pos`). Hoje é só visual (`thumbLeft` em `ViewCarousel`).
4. **Contador "3 / 5":** só o número da esquerda anima (blur-reveal já existe); o "/ 5" fica parado.
5. **Trocar carrossel ↔ grade trava.** O morph usa `layoutId` (`hub-card-WP/WG/TP-<id>`) em `motion.article` no `App.tsx` + `LayoutGroup` no `HubLayout`. Suspeitas: muitos cards com miniaturas WebGL medindo layout, `HubThumbnail` mudando de tier durante o morph, projeção de layout em subárvore com `filter`. Medir com o Performance do Chromium; opções: limitar o morph aos cards visíveis, congelar miniaturas (`paused` + tier `still`) durante a troca, ou trocar por FLIP manual só nos cards visíveis.

Pronto quando: sem pulinhos em 30 s parado; trocar de visualização sem queda perceptível de quadros; trilho clicável e arrastável; só o número da esquerda anima.

## Etapa 2: Transições grandes
Arquivos: `src/components/hub/StudioHero.tsx`, `openStudio`/`closeStudio` em `src/App.tsx`, `src/components/intro/PrismIntro.tsx`, `heroShader.ts`.

1. **View → Studio:** hoje o hero cresce até encher a tela (spring `expand`), só então monta o Studio (`setEditing`) e espera ociosidade + fade de 240 ms. Mudar para: ao chegar em ~70% do progresso `p` já montar o Studio por baixo e começar a dissolver o hero em **blur + fade** (sem filtro pesado: usar `opacity` e, no máximo, `blur` curto na camada do hero); quando `p = 1` já está terminando a dissolução. Sem tela cheia parada esperando. Cuidado: o mount do Studio é pesado; o hero precisa continuar animando por transform/clip (`StudioHero` já usa só transform + clip-path).
2. **Derretimento da intro** (`uMelt` em `heroShader.ts`): menos derretido, mais blur, mais leve. Ideias: reduzir a distorção `dy/dx` e a dispersão cromática; trocar o campo de derretimento por um mais suave (menos listras, mais blob/blur); borda de vidro com blur em vez de linha branca; reduzir `fbm` (3 oitavas → 2) e as 3 chamadas de `hero()` (separação RGB) para 1; escala de render menor durante o derretimento (0.6×); duração 1,9 s mantida ou um pouco menor. Medir quadros no derretimento (hoje trava e fica feio).

Pronto quando: View → Studio sem espera com a tela cheia; derretimento fluido e discreto (blur dominante).

## Etapa 3: Fontes e menus
Arquivos: `src/components/ui/Controls.tsx` (`Select`, `ColorInput`), `src/styles/prism.css`, seletor de fonte no `StudioInspector.tsx`, botão/popover de configurações (`App.tsx`, painel de energia), `tailwind.config.js`.

1. **Fonte da interface = Outfit Light (300)** em todo o app (`body` e variável Tailwind `font-sans`); manter `font-mono-fx` para código/fórmulas. Garantir `@font-face` do peso 300 carregado.
2. **Lista de fontes** (campo "Família da fonte" e outros `Select`): cada opção desenhada **na própria fonte** (aplicar `style={{fontFamily}}` por opção; carregar as fontes do projeto/sistema sob demanda para o menu). Hoje todas aparecem na fonte do app.
3. **Popovers com vidro:** `POPOVER_CLS` em `Controls.tsx` é fundo escuro sólido com borda. Trocar por vidro real: `backdrop-filter: blur(24px) saturate(140%)`, fundo translúcido, borda sutil de luz, sombra suave, cantos maiores. Aplicar em: `Select`, `ColorInput`, popover de configurações, popovers do Studio (atalhos, HUD, dicas) e o menu de contexto da etapa 8. Criar uma classe única `.prism-glass-pop`.

Pronto quando: UI inteira em Outfit Light; menu de fontes mostra cada fonte; todos os popovers com blur consistente.

## Etapa 4: Seleção e escala canvas × aplicado
Arquivos: `src/components/UnifiedCompositor.tsx` (seleção/`onPointerDown`/`isTopLevel`), `src/utils/layerTree.ts`, `apps/prismview-studio/src-tauri/src/native_scene.rs`, `crates/prismview-engine/src/core/text.rs`, `core/scene2d/`.

1. **Clique no relógio seleciona o grupo que está abaixo dele.** Reproduzir: grupo no Z abaixo, relógio acima, clicar no relógio. Investigar: ordem de empilhamento (zIndex = índice no array), `pointerEvents` do grupo, `stopPropagation` e o filtro `!isTopLevel && !selectedSet.has(...)`, grupos com caixa grande capturando o clique, filhos de grupo; hit-test deve respeitar a ordem visual (de cima para baixo) e a área real do item.
2. **Aplicado ≠ canvas.** As proporções do relógio são as mesmas, mas a **caixa do texto** (`text.maxWidth`, "o quadrado") fica menor no motor e o "06" quebra para a linha de baixo. No Studio o texto cabe; no motor as medidas saem um pouco maiores (números tabulares `tnum` do CSS, espaçamento somado depois do último caractere, métricas do cosmic-text) e o motor usa `Wrap::WordOrGlyph`, que quebra no meio da palavra. Correção: quebrar só em espaços (nunca no meio de "00:06"), dar tolerância à largura da caixa, alinhar medidas com o Studio (tabular-nums, espaçamento final), e se a palavra ainda não couber, estender a caixa em vez de quebrar.
3. **Itens "bem maiores" no aplicado** (também o texto de fundo "Solus lesus salvat" muito maior que no canvas): montar um **comparativo lado a lado Studio × motor por tipo de camada** (texto com e sem `maxWidth`, relógio, vidro, barra, imagem, SVG) com um teste/roteiro que falhe se algo quebrar de linha ou mudar de tamanho só no motor. Conferir `PrismScene.logical_size`, `ui = min(w,h)/1080`, `size_px * ui`, `letter_spacing_em`, `line_spacing`, peso/fonte efetivos.

Pronto quando: clique sempre pega o item visualmente acima; o relógio aplicado não quebra linha; comparativo sem diferenças de tamanho.

## Etapa 5: Multiseleção (lote E, depois da 7)
Arquivos: `StudioEditor.tsx`, `StudioInspector.tsx`, `UnifiedCompositor.tsx` (seleção múltipla já existe: `selectedLayerIds`, `onSelectLayer` com Ctrl).

- Com 2+ itens do **mesmo tipo** selecionados, o inspetor edita todos juntos: cor, fonte, tamanho, peso, espaçamento, sombra, vidro, opacidade, posição relativa etc. Valores diferentes mostram "misto"; mexer aplica a todos (uma entrada no histórico).
- Pílula de vidro embaixo do canvas: **"modo de multiseleção"** (com a contagem e o tipo).
- Tipos diferentes: mostrar só as propriedades comuns (opacidade, posição, tamanho, rotação, bloqueio).
- Separar a lógica em função pura testável (`mergeProps`, `applyPatchToMany`) com testes em `tests/`.

## Etapa 6: Alongamento vertical (sozinha)
Arquivos: `src/utils/stemStretch.ts`, `src/utils/stemGlyphMaps.ts`, `src/components/StemText.tsx`, `crates/prism-core/src/vstem.rs` (+ novo build WASM), `crates/prismview-engine/src/core/stem_text.rs`.

Problema no Studio (a versão raster do TS): retas distorcem, curvas (2, 3, 5, 6, 9) entortam, pontas quadradas viram apontadas. O motor já tem a versão vetorial (`vstem`: mapa vertical por glifo com âncoras, compensação de espessura por raio, união de contornos).
- Levar `vstem` ao Studio: compilar `prism-core::vstem` para **WASM** e ler os contornos das fontes (fontes embarcadas: ler os bytes; fontes do sistema: comando Tauri que devolve os bytes do arquivo). Renderizar o texto alongado em canvas a partir dos contornos.
- **Pontas:** preservar cantos vivos e pontas quadradas (hoje o miter + compensação pode apontar). Marcar vértices convexos ~90° e manter a geometria da quina (sem solução de miter nas arestas de topo/base de haste); testar com fontes de pontas retas (Outfit, Inter, DM Mono, serifadas).
- Verificar no motor também (cantos e pontas) com o `cargo test -p prism-core` e o gerador SVG (`render_svg`).
- Critérios: retas intactas, círculos e curvas sem torcer, barras e diagonais com a mesma espessura, pontas quadradas continuam quadradas; Studio e motor iguais.

## Etapa 7: Studio com um painel (sozinha)
Arquivos: `StudioEditor.tsx`, `StudioInspector.tsx`, `Scene3DOutliner.tsx`, `InspectorControls.tsx`, `Scene3DQuickActions.tsx`.

- Hoje: painel esquerdo (árvore de camadas + botões de criar) e painel direito (inspetor), mais barra superior. Passa a ser **um painel só à direita**: propriedades em cima, camadas embaixo, divisória arrastável (proporção persistida); botões de criar camada em uma barra compacta (ou no menu "+" / botão direito no canvas).
- Canvas ocupa o resto da largura (esquerda livre): calcular o tamanho máximo mantendo a proporção do alvo.
- **Auditar redundâncias** e remover: botões duplicados entre árvore, barra e inspetor (ordem Z, bloquear, ocultar, duplicar, excluir aparecem em vários lugares), opções repetidas entre abas (`Conteúdo / Vidro & Cor / Posição`, `Camada / Fundo / Globais`), controles que fazem a mesma coisa. Fazer uma lista de "mantém / junta / tira" antes de mexer.
- Painel recolhível; atalho para mostrar/ocultar; manter acessibilidade (rótulos, foco).

## Etapa 8: Menu de botão direito e atalhos (lote E)
Arquivos: novo `components/studio/ContextMenu.tsx`, `StudioEditor.tsx`, `hooks/useKeyboardShortcuts.ts`, `utils/layerTree.ts`, novo `utils/align.ts` (funções puras + testes).

- **Menu de vidro (blur)** no botão direito sobre o canvas e sobre a lista de camadas, com itens que mudam por tipo:
  - todos: bloquear/destravar, ocultar, duplicar, copiar/colar (e colar estilo), renomear, excluir, selecionar tudo / todos do mesmo tipo, ordem (para frente, para trás, frente total, fundo total), agrupar/desagrupar;
  - alinhar à tela (esquerda, centro horizontal, direita, topo, meio vertical, base) e entre itens selecionados; distribuir horizontal/vertical; igualar largura/altura; centralizar; espelhar H/V; girar 90°;
  - texto: alinhamento do texto, caixa alta/baixa, editar fórmula `ƒx`, alongamento; imagem/vídeo: ajustar/preencher, recortar, substituir arquivo; 3D: centralizar câmera, resetar transformação; vidro/barra/SVG: opções próprias.
- **Atalhos** equivalentes (Ctrl+D, Ctrl+G, Ctrl+Shift+G, `[` `]` para ordem, Ctrl+L, setas movendo 1 px / Shift+setas 10 px, Alt+arrastar duplica, Shift mantém proporção) e **guias inteligentes** com encaixe nas bordas e centros dos outros itens.
- Desempenho: um único listener no container, menu montado só ao abrir, funções de alinhamento puras, sem alocar durante o arraste.

## Etapa 9: Ajuda no app
Arquivos: novo `components/HelpPanel.tsx`, `engine/fx/fxEngine.ts`, botão "Ajuda" no Studio e no Hub.

- Menu **Ajuda** com a documentação do motor de fórmulas `ƒx` do PrismView (que é diferente do Prismlock): sintaxe `$...$`, funções (`$df()$`, `$tf()$`, `$bi()$`, `$si()$`, `$wi()$`, `$mi()$`, `$ai()$`, `$gv()$`, `$ce()$`, `$fl()$`, `$if()$`, `$tc()$`, `$mu()$` e as demais que o motor tiver), globais, FX por propriedade, cores, texto, condições, exemplos prontos copiáveis com o resultado esperado.
- A lista de funções, assinaturas e exemplos deve **sair de uma tabela única no próprio `fxEngine`** (e ser conferida contra `crates/prism-core/src/fx_parity_cases.json`), para a ajuda nunca divergir do código. Teste que falha se uma função do parser não tiver verbete.
- Também: atalhos de teclado, tipos de camada, fluxo de trabalho (criar, aplicar, exportar), alongamento, multiseleção. Busca e navegação por seções.

---

## Regras gerais para todas as etapas
- Conferir `tsc`, `vite build`, `npm test` (em `apps/prismview-studio`) e, no Rust, `cargo test -p prism-core` e `cargo check -p prismview-engine --target x86_64-pc-windows-gnu` (e `-p prismview-studio` no mesmo alvo).
- Roteiro visual no Chromium (Playwright) para o que for de interface; capturas antes/depois quando houver bug visual. Lembrar: capturas saem atrasadas, medir animações por `getBoundingClientRect` por quadro.
- Não alocar objetos dentro de loops de render; respeitar `powerGovernor`/`reducedMotion`; sempre `dispose()` em recursos 3D descartados.
- Antes de uma etapa grande, propor os passos curtos e só então executar.
