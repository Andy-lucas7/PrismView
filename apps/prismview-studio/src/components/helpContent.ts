/**
 * Conteúdo da Ajuda (`HelpPanel`): dados puros, sem React — fácil de testar e de buscar.
 *
 * - Tudo sobre fórmulas ƒx vem de `engine/fx/fxDocs.ts` (conferido contra o motor nos testes).
 * - A tabela de atalhos carrega, em cada linha, o trecho de `useKeyboardShortcuts.ts` que a
 *   implementa (`hook`); `tests/helpContent.test.ts` falha se o atalho deixar de existir.
 */
import {
  FX_DATE_TOKENS,
  FX_DURATION_TOKENS,
  FX_FUNCTION_DOCS,
  FX_PROPERTY_DOCS,
  FX_RECIPES,
  FX_SAMPLE_CONTEXT_NOTE,
  FX_SYNTAX_SECTIONS,
  type FxExample,
} from "../engine/fx/fxDocs.ts";

export type HelpBlock =
  | { t: "p"; text: string }
  | { t: "h"; text: string }
  | { t: "ul"; items: readonly string[] }
  | { t: "ol"; items: readonly string[] }
  | { t: "keys"; rows: readonly (readonly [keys: string, desc: string])[] }
  | { t: "table"; head: readonly string[]; rows: readonly (readonly string[])[]; mono?: readonly number[] }
  | { t: "example"; ex: FxExample }
  | { t: "note"; text: string };

export const HELP_GROUPS = ["Primeiros passos", "Editor", "Fórmulas ƒx", "Referência"] as const;
export type HelpGroup = (typeof HELP_GROUPS)[number];

export interface HelpSection {
  id: string;
  group: HelpGroup;
  title: string;
  /** Rótulo pequeno ao lado do título na navegação (ex.: grupo da função). */
  tag?: string;
  /** Uma frase: aparece no resultado da busca. */
  summary: string;
  blocks: readonly HelpBlock[];
  /** Termos extras que levam a esta seção na busca. */
  keywords?: readonly string[];
}

// ───────────── Atalhos (com o contrato do hook) ─────────────

export interface ShortcutRow {
  keys: string;
  desc: string;
  /** Trecho que precisa existir no arquivo indicado por `src` (padrão: `useKeyboardShortcuts.ts`). */
  hook?: string;
  src?: "hook" | "app" | "editor";
}

export interface ShortcutGroup {
  title: string;
  rows: readonly ShortcutRow[];
}

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  {
    title: "Edição",
    rows: [
      { keys: "Ctrl+Z", desc: "Desfazer", hook: 'key === "z"' },
      { keys: "Ctrl+Y · Ctrl+Shift+Z", desc: "Refazer", hook: 'key === "y"' },
      { keys: "Ctrl+S", desc: "Salvar agora (funciona até dentro de um campo de texto)", hook: 'key === "s"' },
      { keys: "Ctrl+C · Ctrl+X · Ctrl+V", desc: "Copiar · recortar · colar camadas", hook: 'key === "v"' },
      { keys: "Ctrl+D", desc: "Duplicar", hook: 'key === "d"' },
      { keys: "Delete · Backspace", desc: "Excluir as camadas selecionadas", hook: '"Delete"' },
    ],
  },
  {
    title: "Seleção",
    rows: [
      { keys: "Clique", desc: "Seleciona a camada (ou o grupo que a contém)" },
      { keys: "Ctrl+Clique", desc: "Adiciona ou tira uma camada da seleção" },
      { keys: "Alt+Clique", desc: "Entra no grupo e seleciona o item de dentro; em um item já selecionado, desmarca" },
      { keys: "Duplo clique", desc: "Seleciona o item dentro de um grupo" },
      { keys: "Ctrl+A", desc: "Selecionar tudo", hook: 'key === "a"' },
      { keys: "Esc", desc: "Limpar a seleção", hook: 'e.key === "Escape"' },
    ],
  },
  {
    title: "Ordem, grupos e visibilidade",
    rows: [
      { keys: "Ctrl+]", desc: "Trazer para a frente", hook: 'e.key === "]"' },
      { keys: "Ctrl+[", desc: "Enviar para trás", hook: 'e.key === "["' },
      { keys: "Ctrl+G", desc: "Agrupar os selecionados", hook: 'key === "g"' },
      { keys: "Ctrl+Shift+G", desc: "Desagrupar", hook: "onUngroup" },
      { keys: "Ctrl+Shift+H", desc: "Ocultar / mostrar", hook: 'key === "h"' },
      { keys: "Ctrl+L", desc: "Travar / destravar", hook: 'key === "l"' },
    ],
  },
  {
    title: "Movimento",
    rows: [
      { keys: "Setas", desc: "Mover 1 px", hook: '"ArrowLeft"' },
      { keys: "Shift+Setas", desc: "Mover 10 px", hook: "e.shiftKey ? 10 : 1" },
      { keys: "Arrastar", desc: "Mover com encaixe magnético nas bordas e no centro" },
      { keys: "Alt+Arrastar", desc: "Mover livre, sem encaixe" },
      { keys: "Arrastar na lista", desc: "Reordenar a camada ou soltar dentro de um grupo" },
    ],
  },
  {
    title: "Sala 3D",
    rows: [
      { keys: "Botão direito + W A S D Q E", desc: "Voar pela sala (Shift acelera)" },
      { keys: "Botão do meio · Alt+Arrastar", desc: "Deslocar a câmera (pan)" },
      { keys: "Scroll", desc: "Aproximar / afastar" },
      { keys: "G · R · S", desc: "Mover · girar · escalar o objeto", hook: 'room.onGizmo("translate")' },
      { keys: "F", desc: "Focar o objeto selecionado", hook: "room.onFocus()" },
      { keys: "Ctrl+D · Delete", desc: "Duplicar · remover o objeto", hook: "room.onDuplicate" },
      { keys: "Esc", desc: "Sair da Sala 3D", hook: "room.onExit" },
    ],
  },
  {
    title: "Geral",
    rows: [
      { keys: "F1", desc: "Abrir esta Ajuda", hook: '"F1"', src: "app" },
      { keys: "Ctrl+\\", desc: "Recolher / mostrar o painel de propriedades e camadas (canvas em tela cheia)", hook: 'e.code === "Backslash"', src: "editor" },
      { keys: "Ctrl+Shift+P", desc: "Mostrar / ocultar o HUD de desempenho", hook: 'toLowerCase() === "p"', src: "editor" },
    ],
  },
];

// ───────────── Construção das seções ─────────────

const ex = (input: string, expected: string, note?: string): FxExample => ({ input, expected, note });

function fnSection(doc: (typeof FX_FUNCTION_DOCS)[number]): HelpSection {
  const blocks: HelpBlock[] = [
    { t: "p", text: doc.summary },
    { t: "h", text: "Como escrever" },
    { t: "example", ex: { input: `$${doc.signature}$`, expected: "" } },
  ];
  if (doc.names.length > 1) {
    blocks.push({
      t: "note",
      text: `Nomes aceitos: ${doc.names.join(", ")} (${doc.names.slice(1).join(", ")} ${doc.names.length > 2 ? "são apelidos" : "é apelido"} de ${doc.names[0]}).`,
    });
  }
  if (doc.args.length > 0) {
    blocks.push({
      t: "table",
      head: ["Argumento", "O que é"],
      rows: doc.args.map((a) => [a.optional ? `${a.name} (opcional)` : a.name, a.desc]),
      mono: [0],
    });
  }
  if (doc.options && doc.optionsTitle) {
    blocks.push({ t: "h", text: doc.optionsTitle });
    blocks.push({ t: "table", head: ["Valor", "Resultado"], rows: doc.options.map((o) => [o.key, o.desc]), mono: [0] });
  }
  if (doc.notes?.length) blocks.push({ t: "ul", items: doc.notes });
  blocks.push({ t: "h", text: "Exemplos" });
  for (const e of doc.examples) blocks.push({ t: "example", ex: e });
  blocks.push({ t: "note", text: FX_SAMPLE_CONTEXT_NOTE });
  return {
    id: `fx-${doc.id}`,
    group: "Fórmulas ƒx",
    title: doc.names.length > 1 ? `${doc.names[0]}() · ${doc.names.slice(1).join(", ")}` : `${doc.names[0]}()`,
    tag: doc.group,
    summary: doc.summary,
    keywords: [...doc.names, doc.group, doc.signature],
    blocks,
  };
}

function buildFxSections(): HelpSection[] {
  const out: HelpSection[] = [];
  out.push({
    id: "fx-visao",
    group: "Fórmulas ƒx",
    title: "O que é ƒx",
    summary: "Fórmulas entre cifrões que deixam textos, cores e opacidade vivos.",
    keywords: ["formula", "fx", "cifrao", "dinamico"],
    blocks: [
      {
        t: "p",
        text: "ƒx é o motor de fórmulas do PrismView. Em qualquer texto, ou nos campos FX de uma camada, trechos entre $…$ são calculados ao vivo: relógio, data, CPU, memória, bateria, música tocando, clima e variáveis suas.",
      },
      {
        t: "p",
        text: "É um motor próprio, mais simples que o do Prismlock: nomes de função de 2 ou 3 letras, argumentos separados por vírgula, sem declarar variáveis. O Estúdio e o desktop usam exatamente o mesmo avaliador (mesmos resultados nos dois).",
      },
      { t: "h", text: "Onde usar" },
      {
        t: "ul",
        items: [
          "No texto de uma camada Texto (botão ƒx abre o editor com atalhos para inserir funções e globais).",
          "Nos campos FX de opacidade, rotação, cor e visibilidade (aba Camada → seção de cada propriedade).",
        ],
      },
      { t: "example", ex: ex("$df(HH:mm)$ · CPU $rm(cpu)$%", "14:05 · CPU 43%") },
      { t: "note", text: FX_SAMPLE_CONTEXT_NOTE },
    ],
  });
  for (const s of FX_SYNTAX_SECTIONS) {
    const blocks: HelpBlock[] = s.paragraphs.map((text) => ({ t: "p", text }) as HelpBlock);
    if (s.examples?.length) {
      blocks.push({ t: "h", text: "Exemplos" });
      for (const e of s.examples) blocks.push({ t: "example", ex: e });
    }
    out.push({
      id: `fx-${s.id}`,
      group: "Fórmulas ƒx",
      title: s.title,
      summary: s.paragraphs[0],
      keywords: ["sintaxe", "operador", "comparacao", "aritmetica"],
      blocks,
    });
  }
  for (const doc of FX_FUNCTION_DOCS) out.push(fnSection(doc));
  out.push({
    id: "fx-tokens",
    group: "Fórmulas ƒx",
    title: "Tokens de data e duração",
    summary: "Letras que df() e tf() reconhecem nos padrões.",
    keywords: ["df", "tf", "padrao", "formato", "data", "hora"],
    blocks: [
      { t: "h", text: "df() — data e hora" },
      {
        t: "table",
        head: ["Token", "Significado", "Exemplo"],
        rows: FX_DATE_TOKENS.map((t) => [t.token, t.meaning, t.sample]),
        mono: [0, 2],
      },
      { t: "p", text: "Texto literal entre aspas simples ('h'). Duas aspas simples seguidas escrevem uma aspa." },
      { t: "h", text: "tf() — duração" },
      { t: "table", head: ["Token", "Significado"], rows: FX_DURATION_TOKENS.map((t) => [t.token, t.meaning]), mono: [0] },
      { t: "note", text: FX_SAMPLE_CONTEXT_NOTE },
    ],
  });
  out.push({
    id: "fx-propriedades",
    group: "Fórmulas ƒx",
    title: "FX por propriedade",
    summary: "Opacidade, rotação, cor e visibilidade de uma camada podem ser fórmulas.",
    keywords: ["opacidade", "rotacao", "cor", "visivel", "propriedade", "animar"],
    blocks: [
      {
        t: "p",
        text: "Cada propriedade da lista abaixo tem um campo FX na camada. O campo precisa de pelo menos um $…$; sem cifrões o texto é usado como está. O resultado é convertido para o tipo da propriedade.",
      },
      ...FX_PROPERTY_DOCS.flatMap<HelpBlock>((p) => [
        { t: "h", text: p.label },
        { t: "p", text: p.desc },
        { t: "example", ex: p.example },
      ]),
      { t: "note", text: FX_SAMPLE_CONTEXT_NOTE },
    ],
  });
  out.push({
    id: "fx-globais",
    group: "Fórmulas ƒx",
    title: "Variáveis globais",
    summary: "Valores do projeto que as fórmulas leem com gv() e que cliques podem alterar.",
    keywords: ["global", "gv", "variavel", "switch", "toggle"],
    blocks: [
      {
        t: "p",
        text: "As globais ficam na aba Globais do inspetor. Cada uma tem nome, tipo e valor; o nome não diferencia maiúsculas de minúsculas.",
      },
      {
        t: "table",
        head: ["Tipo", "Valor", "Uso comum"],
        rows: [
          ["SWITCH", "1 ou 0", "Mostrar/ocultar blocos: $if(gv(show_stats), …)$"],
          ["NUMBER", "número", "Limites e escalas: $if(rm(cpu) > gv(limite), …)$"],
          ["TEXT", "texto", "Padrões de data, nomes: $df(gv(fmt))$"],
          ["COLOR", "#AARRGGBB", "Cor de destaque única para o projeto: $gv(accent)$"],
        ],
        mono: [0],
      },
      {
        t: "p",
        text: "Uma camada pode, ao ser clicada, alternar uma global, definir um valor ou passar para o próximo item de uma lista — assim um widget troca de aparência com um clique.",
      },
      { t: "example", ex: ex("$if(gv(show_stats), 'on', 'off')$", "on") },
      { t: "example", ex: ex("$gv(accent)$", "#FF00D2FF") },
    ],
  });
  out.push({
    id: "fx-receitas",
    group: "Fórmulas ƒx",
    title: "Exemplos prontos",
    summary: "Fórmulas comuns para copiar e colar.",
    keywords: ["receita", "exemplo", "copiar", "relogio", "bateria", "saudacao"],
    blocks: [
      { t: "p", text: "Use o botão de copiar de cada exemplo e cole no campo de texto ou FX." },
      ...FX_RECIPES.flatMap<HelpBlock>((r) => [{ t: "h", text: r.title }, { t: "example", ex: r }]),
      { t: "note", text: FX_SAMPLE_CONTEXT_NOTE },
    ],
  });
  return out;
}

const GETTING_STARTED: HelpSection[] = [
  {
    id: "visao-geral",
    group: "Primeiros passos",
    title: "O que é o PrismView Studio",
    summary: "Editor de wallpapers e widgets animados para o Windows.",
    keywords: ["inicio", "sobre", "wallpaper", "widget", "prism"],
    blocks: [
      {
        t: "p",
        text: "O PrismView Studio cria wallpapers animados e widgets de área de trabalho para o Windows. Você monta a cena no Studio; o motor nativo (um programa leve em segundo plano) desenha o resultado atrás dos ícones, sem abrir navegador.",
      },
      {
        t: "ul",
        items: [
          "Wallpaper: ocupa o desktop inteiro, em todos os monitores. Pode ter vídeo, imagem, texto, vidro e cena 3D.",
          "Widget: janela transparente do tamanho que você definir, fixada no desktop (relógio, música, telemetria). Pode ser movida e travada.",
          "Pacote .prism: arquivo zip com o projeto e suas mídias. Serve para exportar, compartilhar e importar (widgets usam .widget.prism).",
        ],
      },
      { t: "note", text: "Só Windows. Os projetos ficam em %APPDATA%\\PrismView." },
    ],
  },
  {
    id: "fluxo",
    group: "Primeiros passos",
    title: "Fluxo de trabalho",
    summary: "Do projeto novo ao wallpaper aplicado, em seis passos.",
    keywords: ["criar", "aplicar", "exportar", "importar", "passo a passo", "tutorial"],
    blocks: [
      {
        t: "ol",
        items: [
          "Crie: no Hub, “Novo Projeto” ou escolha um modelo em “Modelos Prontos”. Também dá para importar um modelo 3D (.glb/.gltf), um vídeo ou imagem, ou um pacote .prism (seção “Importar do PC”).",
          "Edite: clique no card para abrir o Studio. Adicione camadas (Vídeo, Imagem, SVG, Texto, Vidro, Barra, Grupo, Cena 3D), arraste no canvas e ajuste no inspetor.",
          "Dê vida: use fórmulas ƒx em textos, cores e opacidade; ligue globais a cliques; no 3D, ações reativas a mouse, áudio e teclado.",
          "Salve: o projeto salva sozinho; a etiqueta ao lado do nome mostra Salvando, Salvo ou Falha (com “Tentar de novo”). Ctrl+S força.",
          "Aplique: “Aplicar no Desktop” (wallpaper) ou “Fixar Widget na Tela”. Um selo laranja no botão avisa o que o desktop ainda não desenha.",
          "Compartilhe: “Exportar” gera o .prism. Para abrir no outro PC use “Pacote .prism” no Hub; se o id já existir, escolha Substituir ou Manter os dois.",
        ],
      },
      {
        t: "note",
        text: "Importou um pacote com botão que abre programas? Ele chega marcado para revisão: confirme o destino antes de usar.",
      },
    ],
  },
  {
    id: "hub",
    group: "Primeiros passos",
    title: "O Hub",
    summary: "Biblioteca de projetos, modelos prontos e controles do motor.",
    keywords: ["carrossel", "grade", "pausar", "bandeja", "opcoes", "biblioteca"],
    blocks: [
      {
        t: "ul",
        items: [
          "Wallpapers e Widgets: seus projetos. Filtros por tipo (3D, Vídeo, 2D & Vidro) e busca por nome.",
          "Modelos Prontos: projetos de exemplo para começar a partir de algo pronto.",
          "Carrossel ou Grade: botões no topo da lista alternam a visualização; a escolha é lembrada.",
          "Pausar / Retomar: congela as animações do desktop sem remover o wallpaper.",
          "Engrenagem (Opções): ícone na bandeja, iniciar com o Windows e Modo de Energia.",
        ],
      },
    ],
  },
];

const EDITOR: HelpSection[] = [
  {
    id: "editor",
    group: "Editor",
    title: "A tela do editor",
    summary: "Lista de camadas, canvas e inspetor.",
    keywords: ["canvas", "inspetor", "camadas", "barra superior"],
    blocks: [
      {
        t: "ul",
        items: [
          "Canvas: a área onde a cena aparece, na proporção do seu monitor, ocupando todo o espaço à esquerda do painel. Arraste para mover; as alças dos cantos mudam o tamanho mantendo a proporção.",
          "Painel à direita: propriedades em cima e camadas embaixo. Arraste a divisória entre os dois (ou use as setas com ela focada; Enter restaura) para dar mais espaço a um deles; a proporção é lembrada.",
          "Propriedades: Camada (a selecionada), Fundo (fundo do projeto, escurecimento, mídia) e Globais (variáveis do ƒx).",
          "Camadas: lista com a ordem de empilhamento (a de cima fica na frente) e uma fileira de ícones para criar cada tipo. Ocultar e travar ficam sempre visíveis; ordem, duplicar e excluir aparecem ao passar o mouse ou selecionar.",
          "Ctrl+\\ recolhe o painel inteiro (o canvas ocupa a largura toda) e mostra de novo; o botão ao lado de Exportar faz o mesmo.",
          "Barra superior: voltar ao Hub, nome do projeto, estado de salvamento, desfazer/refazer, ajuda e atalhos, Exportar e Aplicar.",
        ],
      },
      {
        t: "p",
        text: "Se o projeto tem cena 3D, aparece também o seletor de modos: Layout 2D, Sala 3D e Testar Interações.",
      },
    ],
  },
  {
    id: "camadas",
    group: "Editor",
    title: "Tipos de camada",
    summary: "O que cada camada faz e o que ela aceita.",
    keywords: ["video", "imagem", "svg", "texto", "vidro", "barra", "grupo", "cena 3d", "png", "icone"],
    blocks: [
      {
        t: "table",
        head: ["Camada", "Para que serve"],
        rows: [
          ["Vídeo", "Vídeo de fundo ou em caixa; o desktop decodifica pela GPU. Um vídeo que cobre a tela vira o fundo do wallpaper."],
          ["Imagem PNG", "PNG/WebP com transparência, ou a capa da música que está tocando."],
          ["SVG / Ícone", "Ícones embutidos ou SVG próprio. Com ação de clique vira botão (tocar/pausar, próxima, anterior, abrir link ou app)."],
          ["Texto / ƒx", "Texto fixo ou fórmula viva (relógio, data, CPU, música)."],
          ["Vidro", "Placa de vidro óptico (desfoque, refração, dispersão, brilho) sobre o fundo."],
          ["Barra", "Barra de progresso ligada à música, CPU, RAM ou bateria."],
          ["Grupo", "Agrupa camadas para mover, escalar e aplicar vidro juntas. Widgets importados entram como grupo."],
          ["Cena 3D", "Modelos .glb/.gltf ou esculturas prontas, com luz, ambiente e reações a mouse, áudio e teclado."],
        ],
      },
      {
        t: "note",
        text: "No desktop, só a primeira Cena 3D é desenhada; fontes woff/woff2 também não carregam no motor. O selo no botão Aplicar e o painel de compatibilidade do inspetor listam esses casos.",
      },
    ],
  },
  {
    id: "selecao",
    group: "Editor",
    title: "Seleção, ordem e grupos",
    summary: "Como escolher, empilhar, agrupar e travar camadas.",
    keywords: ["multiselecao", "selecionar", "ctrl", "travar", "ocultar", "z", "ordem", "agrupar"],
    blocks: [
      {
        t: "ul",
        items: [
          "Clique no canvas seleciona a camada que está visualmente por cima, mesmo se estiver travada. Dentro de um grupo, o primeiro clique pega o grupo; duplo clique ou Alt+Clique pega o item de dentro.",
          "Ctrl+Clique soma ou tira camadas da seleção. Ao arrastar uma delas, todas se movem juntas mantendo a posição relativa; Ctrl+A seleciona tudo.",
          "Camada travada (Ctrl+L) pode ser selecionada, mas não se move nem muda de tamanho até ser destravada.",
          "Camada oculta (Ctrl+Shift+H) some do canvas e do desktop, mas continua na lista.",
          "Ordem Z: a lista mostra a camada da frente no topo. Ctrl+] e Ctrl+[ sobem e descem; também dá para arrastar na lista ou soltar sobre um grupo.",
          "Agrupar (Ctrl+G) junta as selecionadas em um grupo; Desagrupar (Ctrl+Shift+G) devolve cada uma ao nível acima.",
        ],
      },
    ],
  },
  {
    id: "texto",
    group: "Editor",
    title: "Texto, fontes e alongamento",
    summary: "Fontes, caixa de texto, espaçamento e Escala Vertical.",
    keywords: ["fonte", "tipografia", "alongamento", "escala vertical", "caixa", "largura", "espacamento"],
    blocks: [
      {
        t: "ul",
        items: [
          "Conteúdo: texto livre ou fórmula ƒx. Quebras de linha com Enter.",
          "Fonte: lista com as fontes do sistema e do projeto. Peso, tamanho, alinhamento e espaçamento entre letras ficam logo abaixo.",
          "Caixa de texto: sem largura definida, o texto fica em uma linha. Arraste as alças laterais para definir a largura; o texto quebra só entre palavras, e uma palavra maior que a caixa estende a caixa em vez de ser partida (isso vale também no desktop).",
          "Escala Vertical (1× a 2,4×): estica as letras só no sentido vertical. O desktop usa contornos vetoriais das fontes; o canvas do editor mostra uma aproximação, então confira o resultado depois de aplicar.",
          "Sombra: desfoque da sombra do texto. Vidro no texto: modo “glifo” (vidro dentro das letras) ou “pílula” (placa de vidro atrás do texto).",
        ],
      },
    ],
  },
  {
    id: "vidro",
    group: "Editor",
    title: "Vidro óptico",
    summary: "Como o vidro enxerga o fundo e o que cada controle muda.",
    keywords: ["vidro", "glass", "blur", "refracao", "dispersao", "cromatica", "fundo"],
    blocks: [
      {
        t: "p",
        text: "O vidro distorce o que está atrás dele: refração, desfoque, dispersão cromática, brilho especular e grão. O que ele enxerga é a mídia de fundo do projeto (vídeo ou imagem de fundo, ou uma cena 3D de tela cheia).",
      },
      {
        t: "ul",
        items: [
          "Sem mídia de fundo o vidro só mostra a cor de tinta, sem distorção.",
          "Desfoque, refração, aberração cromática, bisel e opacidade do gelo têm sliders próprios; cor da tinta e borda ajustam o acabamento.",
          "No modo econômico/ultra/congelado do motor o desfoque é reduzido ou desligado para poupar bateria.",
        ],
      },
    ],
  },
  {
    id: "cena3d",
    group: "Editor",
    title: "Cena 3D e Sala 3D",
    summary: "Modelos, câmera, luz e reações.",
    keywords: ["3d", "glb", "gltf", "sala", "camera", "gizmo", "reativo", "audio", "mouse"],
    blocks: [
      {
        t: "ul",
        items: [
          "Layout 2D: posiciona a camada 3D como as outras.",
          "Sala 3D: câmera livre (botão direito + W A S D Q E), mover/girar/escalar objetos com G, R e S, foco com F, e botão para salvar a câmera do wallpaper. Sensores ficam desligados aqui.",
          "Testar Interações: câmera travada na do wallpaper; mouse, clique, áudio e teclado funcionam ao vivo para você conferir as reações.",
          "Reações: a lista de ações liga uma fonte (cursor, hover/clique em uma peça, graves/médios/agudos, teclado, capa/cor da música) a um alvo (girar, mover, escalar, brilho, trocar animação).",
          "Alt+Clique em uma peça do modelo seleciona o osso ou a parte para usar como alvo.",
        ],
      },
    ],
  },
  {
    id: "aplicar",
    group: "Editor",
    title: "Aplicar, fixar e exportar",
    summary: "O que acontece quando o projeto vai para o desktop.",
    keywords: ["aplicar", "fixar", "desktop", "motor", "exportar", "pacote", "widget", "bandeja"],
    blocks: [
      {
        t: "ul",
        items: [
          "Aplicar no Desktop: converte o projeto e entrega ao motor nativo, que roda em segundo plano. Depois disso o Studio pode ser fechado e o wallpaper continua.",
          "Fixar Widget: cria uma janela transparente no desktop. Arraste para mover; travar o widget faz o clique passar por ele.",
          "Remover: no card do wallpaper ativo (ou pelo ícone da bandeja) tira o wallpaper do desktop; o projeto continua salvo.",
          "Exportar: grava o pacote .prism (ou .widget.prism) com tudo o que o projeto usa.",
          "Se o aplicado fica diferente do canvas, veja o painel de compatibilidade do inspetor: ele lista o que o desktop ainda não desenha.",
        ],
      },
    ],
  },
  {
    id: "energia",
    group: "Editor",
    title: "Energia e desempenho",
    summary: "Modos que limitam quadros e efeitos para poupar bateria.",
    keywords: ["energia", "bateria", "fps", "eco", "ultra", "congelado", "desempenho"],
    blocks: [
      {
        t: "table",
        head: ["Modo", "O que faz"],
        rows: [
          ["Automático", "Troca sozinho conforme a bateria: entra em Ultra economia com 20% ou menos e só volta acima de 25%."],
          ["Normal", "Usa o perfil escolhido em cada wallpaper."],
          ["Econômico", "Até 30 FPS, resolução 1×, sem desfoque de vidro."],
          ["Ultra economia", "Até 15 FPS, sem reatividade nem giro automático."],
          ["Congelado", "Um quadro só; a GPU descansa."],
        ],
        mono: [0],
      },
      {
        t: "p",
        text: "O motor também pausa sozinho um monitor quando uma janela maximizada ou um jogo em tela cheia cobre o desktop. Escolha o modo na engrenagem do Hub.",
      },
    ],
  },
];

const REFERENCE: HelpSection[] = [
  {
    id: "atalhos",
    group: "Referência",
    title: "Atalhos de teclado",
    summary: "Todos os atalhos do editor.",
    keywords: ["atalho", "teclado", "ctrl", "shortcut", "tecla"],
    blocks: [
      { t: "p", text: "Os atalhos de edição não disparam enquanto você digita em um campo de texto (exceto Ctrl+S)." },
      ...SHORTCUT_GROUPS.flatMap<HelpBlock>((g) => [
        { t: "h", text: g.title },
        { t: "keys", rows: g.rows.map((r) => [r.keys, r.desc] as const) },
      ]),
    ],
  },
];

/** Todas as seções, na ordem de navegação. Construído uma vez quando a Ajuda abre. */
export function buildHelpSections(): HelpSection[] {
  return [...GETTING_STARTED, ...EDITOR, ...buildFxSections(), ...REFERENCE];
}

// ───────────── Busca ─────────────

/** Minúsculas e sem acentos: “fórmula” e “formula” se encontram. */
export function normalizeSearch(s: string): string {
  return s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}

function blockText(b: HelpBlock): string {
  switch (b.t) {
    case "p":
    case "h":
    case "note":
      return b.text;
    case "ul":
    case "ol":
      return b.items.join(" ");
    case "keys":
      return b.rows.map((r) => `${r[0]} ${r[1]}`).join(" ");
    case "table":
      return `${b.head.join(" ")} ${b.rows.map((r) => r.join(" ")).join(" ")}`;
    case "example":
      return `${b.ex.input} ${b.ex.expected} ${b.ex.note ?? ""}`;
  }
}

export interface SearchIndexEntry {
  section: HelpSection;
  title: string;
  keywords: string;
  body: string;
  /** Palavras inteiras do título (“rm”, “vidro”): casam com a maior prioridade. */
  titleWords: ReadonlySet<string>;
  /** Palavras inteiras das palavras-chave (apelidos como “sys”, “si”). */
  keywordWords: ReadonlySet<string>;
}

const WORD_SPLIT = /[^a-z0-9ƒ_]+/;

export function buildSearchIndex(sections: readonly HelpSection[]): SearchIndexEntry[] {
  return sections.map((section) => {
    const title = normalizeSearch(section.title);
    const keywords = normalizeSearch((section.keywords ?? []).join(" "));
    return {
      section,
      title,
      keywords,
      titleWords: new Set(title.split(WORD_SPLIT).filter(Boolean)),
      keywordWords: new Set(keywords.split(WORD_SPLIT).filter(Boolean)),
      body: normalizeSearch(`${section.summary} ${section.blocks.map(blockText).join(" ")}`),
    };
  });
}

/**
 * Todos os termos precisam aparecer. Pontuação por termo: palavra inteira do título (400) > palavra
 * inteira das palavras-chave (300) > início de palavra do título (150) > trecho do título (100) >
 * trecho das palavras-chave (40) > texto (10).
 * Empate = ordem original. Assim “rm” abre rm() antes de “fórmula”.
 */
export function searchSections(index: readonly SearchIndexEntry[], query: string): HelpSection[] {
  const terms = normalizeSearch(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return index.map((e) => e.section);
  const scored: { section: HelpSection; score: number; order: number }[] = [];
  index.forEach((e, order) => {
    let score = 0;
    for (const t of terms) {
      if (e.titleWords.has(t)) score += 400;
      else if (e.keywordWords.has(t)) score += 300;
      else if ([...e.titleWords].some((w) => w.startsWith(t))) score += 150;
      else if (e.title.includes(t)) score += 100;
      else if (e.keywords.includes(t)) score += 40;
      else if (e.body.includes(t)) score += 10;
      else return;
    }
    scored.push({ section: e.section, score, order });
  });
  scored.sort((a, b) => b.score - a.score || a.order - b.order);
  return scored.map((s) => s.section);
}
