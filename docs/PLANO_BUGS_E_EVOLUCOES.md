# Plano: bugs e evoluções (PrismView)

Decisões já tomadas: fonte do app = **Outfit Light (300)**; Studio com **um painel só à direita**, **propriedades em cima e camadas embaixo**, divisória arrastável entre os dois blocos; o canvas ocupa todo o resto da largura. Meta de UX: **completo como o Canva** (alterar tudo de qualquer forma, organizado e intuitivo), sempre **leve e otimizado** (nada alocado por quadro, nada re-renderizando à toa).

Regra de trabalho: cada etapa agrupa itens do mesmo lugar do código; antes de corrigir um bug, reproduzir e escrever um teste ou roteiro de verificação.

| Etapa | Itens | Onde mexe |
|---|---|---|
| **1. Carrossel** | Remover a luz do mouse nos itens (feia). Achar e eliminar os "pulinhos" a cada ~2 s nos itens laterais. Trilho de baixo clicável e arrastável. Só o número da esquerda do "3/5" anima. Acabar com a trava ao trocar carrossel ↔ grade. | `ViewCarousel`, `HubLayout`, `HubThumbnail` |
| **2. Transições grandes** | View → Studio: perto de encher a tela o hero vira blur e já dissolve no Studio (monta o Studio a ~70% do crescimento; sem espera com a tela cheia). Derretimento da intro: mais blur, menos derretido, shader mais leve e sem engasgo. | `StudioHero`, `openStudio` (`App`), `PrismIntro`, `heroShader` |
| **3. Fontes e menus** | Fonte da interface = Outfit Light. A lista de fontes mostra cada nome na própria fonte. Popovers (fontes, `Select`, cor, configurações) com blur de vidro em vez de fundo preto com borda. | `Controls`, `prism.css`, seletor de fonte do `StudioInspector`, botão de configurações |
| **4. Canvas × aplicado** | Clicar no relógio selecionava o grupo que está *abaixo* (hit-test/ordem de grupos). Itens bem maiores no wallpaper aplicado do que no canvas (em tudo, item por item, não é zoom; ver as 2 capturas do "Obsidian Editorial": texto de fundo e relógio). Montar um comparativo lado a lado Studio × motor por tipo de camada e corrigir a escala. | `UnifiedCompositor`, `native_scene.rs`, `core/scene2d`, `core/text.rs` |
| **5. Multiseleção** | Selecionou vários itens do mesmo tipo: editar tudo junto (cor, fonte, tamanho, peso, espaçamento, sombra...). Pílula de vidro embaixo: "modo de multiseleção". | `StudioEditor`, `StudioInspector` |
| **6. Alongamento vertical** | Retas não podem distorcer; curvas (2, 3, 5, 6, 9) não podem entortar; pontas quadradas não podem virar apontadas. Levar o algoritmo vetorial do motor (`prism_core::vstem`) para o Studio, com preservação de cantos, e testar com várias fontes. | `stemStretch.ts`, `StemText`, `stemGlyphMaps`, crate `prism-core` (WASM) |
| **7. Studio com um painel** | Painel único à direita: propriedades em cima, camadas embaixo, divisória arrastável (proporção salva). Canvas o maior possível na largura. Tirar opções e botões repetidos (auditar o que é redundante entre a árvore, a barra e o inspetor). | `StudioEditor`, `StudioInspector`, `Scene3DOutliner` |
| **8. Menu de botão direito e atalhos** | Menu de vidro (blur) que muda conforme o tipo: bloquear/destravar, subir/descer/frente/fundo, duplicar, agrupar/desagrupar, renomear, ocultar, excluir; alinhar ao centro/meio/cantos/topo/base/esquerda/direita; distribuir; espelhar; proporção; recortar (imagem/vídeo); copiar/colar estilo; copiar/colar; selecionar tudo/mesmo tipo; para texto: alinhar texto, caixa alta, fx; 3D: câmera/centralizar. Atalhos de teclado equivalentes, setas movendo, guias inteligentes e encaixe. Tudo leve (um único listener, itens montados só ao abrir). | `StudioEditor`, `useKeyboardShortcuts`, novo `ContextMenu`, `utils/layerTree` |
| **9. Ajuda no app** | Menu "Ajuda" com a documentação completa do motor de fórmulas `ƒx` (sintaxe `$...$`, todas as funções com exemplos e resultado, globais `$gv`, FX por propriedade), atalhos, tipos de camada e fluxo de trabalho. Busca, exemplos copiáveis. A lista de funções sai do próprio motor (`fxEngine`), para a ajuda nunca divergir do código. | novo `HelpPanel`, `engine/fx/fxEngine.ts` |

## Ordem sugerida
1 → 2 → 3 → 4 (rápidas, bugs que você sente todo dia) → 5 → 6 → 7 → 8 → 9. A etapa 7 vem antes da 8 e da 5, porque o painel novo define onde ficam as opções; se preferir, a 5 pode ir antes da 7 usando o painel atual.

## Critérios de pronto
- Cada etapa: `tsc`, `vite build`, `npm test` e roteiro visual no Chromium; testes novos quando o código for puro (hit-test, multiseleção, alinhamento, lista de funções da ajuda).
- Desempenho: nenhuma alocação por quadro nos loops de render; menus e painéis só renderizam quando abertos.
