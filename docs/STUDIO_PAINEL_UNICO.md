# Studio com um painel só (Lote D, etapa 7)

## Layout

- Painel único à **direita** (`components/studio/StudioSidePanel.tsx`, 352 px): **propriedades em cima** (`StudioInspector`), **camadas embaixo** (`layersPanel` em `StudioEditor.tsx`), com divisória arrastável entre os dois.
- Divisória: `role="separator"` focável. Arrastar mexe direto no `flex-basis` (sem re-render por movimento) e grava ao soltar; setas = 3 %, Shift+setas = 10 %, Home/End = limites, Enter ou duplo clique = padrão (58 %). Proporção em `localStorage` (`prismview.studioPanelSplit.v1`), limitada a 25–80 % e a 170 px (cima) / 150 px (baixo).
- Canvas: ocupa todo o resto (`padding-left` 16 px, `padding-right` = largura do painel + folgas via `stageRightInset`), mantendo a proporção do monitor.
- Recolher: botão ao lado de **Exportar** ou **Ctrl+\\**. Recolhido, o painel nem fica montado e o canvas usa a largura toda. Estado em `prismview.studioPanelOpen.v1`.
- Funções puras e testes: `utils/panelSplit.ts`, `tests/panelSplit.test.ts`.

## Auditoria de redundâncias (feita antes de mexer)

Conferido lendo `StudioEditor.tsx` (barra, árvore) e `StudioInspector.tsx` (abas, controles).

| Item | Decisão | Motivo |
|---|---|---|
| Voltar, nome, estado de salvamento, desfazer/refazer, Exportar, Aplicar | **mantém** | Sem equivalente em outro lugar. |
| Abas do inspetor **Camada / Fundo / Globais** | **mantém** | Escopos diferentes: a camada, o projeto, as variáveis ƒx. |
| Sub-abas **Conteúdo / Vidro & Cor / Posição** | **mantém** | Conferido lendo os controles de cada aba: nenhum se repete na mesma camada (rótulos iguais só aparecem em tipos diferentes, ex.: "Dispersão RGB" do SVG × vidro completo). Opacidade, ƒx de visibilidade e blend ficam em Vidro & Cor; posição, tamanho, rotação, ação de clique e parallax em Posição. |
| Ocultar e travar na linha da camada | **mantém, sempre visíveis** | São *estado* da camada; ganharam `aria-label` e `aria-pressed`. |
| Ordem Z, duplicar, excluir na linha | **mantém, só no hover/foco/seleção** | Têm atalho (Ctrl+], Ctrl+[, Ctrl+D, Del); em repouso poluíam 6 botões por linha. Ganharam `aria-label` com o nome da camada. |
| Barra de grupo (Agrupar/Desagrupar) | **mantém** | Descoberta do Ctrl+G; só aparece com seleção múltipla/grupo. |
| Duplicar/excluir objeto 3D no outliner **e** na barra da Sala 3D | **mantém** | Contextos diferentes: lista (qualquer objeto) × sobre o canvas (o selecionado). |
| Botão **Atalhos** + popover com tabela própria | **junta → Ajuda** | A tabela existia em dois lugares (popover e Ajuda). Agora só `SHORTCUT_GROUPS` (Ajuda, F1); um botão só: "Ajuda, fórmulas e atalhos". Popover e estado `shortcutsOpen` removidos. |
| Grade 2 × 4 de botões grandes para criar camadas + botão de widget | **compacta** | Virou uma fileira de 9 ícones (tooltip e `aria-label`), liberando altura para a lista. |
| Cabeçalho "Arraste p/ Z / Grupo" | **encurta** | "Arraste para reordenar" + dica completa no `title`. |
| "Alinhamento Rápido" do inspetor (posições fixas 12 %/88 %) | **mantém por ora** | Não é igual a alinhar à tela; será substituído pelas funções puras da etapa 8 (`utils/align.ts`). |

## Não mudou

- Lógica de seleção, arraste de linhas, histórico e salvamento.
- O inspetor continua com seu estado local (aba, otimizações 3D); ao recolher o painel ele é desmontado e volta com os padrões.
