/**
 * Documentação do motor de fórmulas `ƒx` (`$...$`) — fonte única da Ajuda do app.
 *
 * Fica em arquivo próprio para NÃO entrar no bundle leve do desktop (`desktop.tsx` só importa o
 * avaliador). A lista de funções vem de `FX_FUNCTION_NAMES` (`fxEngine.ts`): como cada verbete
 * declara `names: FxFunctionName[]`, o `tsc` e `tests/fxDocs.test.ts` impedem que o parser (TS
 * e `crates/prism-core/src/fx.rs`) ganhe uma função sem verbete, e cada exemplo é AVALIADO pelo
 * motor real nos testes — o resultado mostrado na Ajuda nunca diverge do código.
 *
 * Todos os resultados (`expected`) foram calculados com `FX_SAMPLE_CONTEXT`, o mesmo contexto de
 * `crates/prism-core/src/fx_parity_cases.json` (sábado 21/11/2026 14:05:09).
 */
import type { FxFunctionName } from "./fxEngine";

export interface FxExample {
  /** Texto exato a digitar no campo (já com `$...$`). */
  input: string;
  /** Resultado com o contexto de exemplo (`FX_SAMPLE_CONTEXT_NOTE`). */
  expected: string;
  note?: string;
}

export interface FxArgDoc {
  name: string;
  desc: string;
  optional?: boolean;
}

export interface FxOptionDoc {
  /** Valor do 1º argumento (`rm(cpu)`). Apelidos separados por " · ". */
  key: string;
  desc: string;
}

export type FxGroup = "Tempo" | "Sistema" | "Mídia" | "Clima e sol" | "Lógica" | "Texto" | "Matemática" | "Cor" | "Variáveis";

export const FX_GROUPS: readonly FxGroup[] = [
  "Tempo",
  "Sistema",
  "Mídia",
  "Clima e sol",
  "Lógica",
  "Texto",
  "Matemática",
  "Cor",
  "Variáveis",
];

export interface FxFunctionDoc {
  /** Âncora estável (slug) para navegação e busca. */
  id: string;
  /** Primeiro = nome principal; os demais são apelidos aceitos pelo parser. */
  names: readonly FxFunctionName[];
  group: FxGroup;
  /** Assinatura curta, exatamente como se digita. */
  signature: string;
  summary: string;
  args: readonly FxArgDoc[];
  optionsTitle?: string;
  options?: readonly FxOptionDoc[];
  notes?: readonly string[];
  examples: readonly FxExample[];
}

/** Dados fixos dos exemplos (idênticos ao `context` de `fx_parity_cases.json`). */
export const FX_SAMPLE_CONTEXT_NOTE =
  "Resultados calculados com: sábado 21/11/2026 14:05:09 · CPU 42,5% · RAM 63,25% (10,25 de 16 GB) · PC “PRISM-PC” · bateria 77% (na bateria) · tocando “Song” de “AC/DC”, 1:05 de 3:20 · sem dados de clima · globais show_stats = 1, accent = #FF00D2FF, fmt = dd MMMM.";

export const FX_FUNCTION_DOCS: readonly FxFunctionDoc[] = [
  // ───────────── Tempo ─────────────
  {
    id: "df",
    names: ["df"],
    group: "Tempo",
    signature: "df(padrão)",
    summary:
      "Data e hora atuais formatadas por um padrão. Atualiza sozinha (a cada minuto, ou a cada segundo se o padrão tiver segundos).",
    args: [
      {
        name: "padrão",
        desc: "Texto com tokens (tabela abaixo). Sem aspas, vírgulas e espaços fazem parte do padrão. Vazio = HH:mm.",
        optional: true,
      },
    ],
    notes: [
      "Só vira token uma PALAVRA inteira que seja um token válido; “de”, “às”, “em” saem como texto. Para forçar texto literal use aspas simples: 'h'.",
      "Tokens colados (HHmm) não valem: separe com espaço, “:” ou texto literal (H'h'mm).",
      "O padrão S devolve o horário Unix em segundos (número).",
      "Para padrão vindo de outra fórmula use aspas ou chamada dentro: df(gv(fmt)).",
    ],
    examples: [
      { input: "$df(HH:mm)$", expected: "14:05" },
      { input: "$df(EEEE, dd de MMMM de yyyy)$", expected: "Sábado, 21 de Novembro de 2026" },
      { input: "$df(hh:mm:ss a)$", expected: "02:05:09 PM" },
      { input: "$df(dd/MM/yyyy)$", expected: "21/11/2026" },
      { input: "$df(H'h'mm)$", expected: "14h05" },
      { input: "$tc(up, df(EEE · dd MMM))$", expected: "SÁB · 21 NOV", note: "tc(up, …) deixa em maiúsculas" },
      { input: "$df(gv(fmt))$", expected: "21 Novembro", note: "O padrão vem da variável global fmt" },
    ],
  },
  {
    id: "tf",
    names: ["tf"],
    group: "Tempo",
    signature: "tf(segundos, padrão)",
    summary: "Formata uma duração em segundos (tempo de música, contagem regressiva, uptime).",
    args: [
      { name: "segundos", desc: "Número de segundos (negativo vira 0)." },
      {
        name: "padrão",
        desc: "Tokens D (dias), H (horas), m (minutos), s (segundos); repita a letra para 2 dígitos. Padrão: H:mm:ss se ≥ 1 h, senão mm:ss.",
        optional: true,
      },
    ],
    notes: ["Cada unidade absorve as que faltam: sem H no padrão, as horas somam nos minutos."],
    examples: [
      { input: "$tf(90)$", expected: "01:30" },
      { input: "$tf(3725)$", expected: "1:02:05" },
      { input: "$tf(90061, D dias HH:mm)$", expected: "1 dias 01:01" },
    ],
  },

  // ───────────── Variáveis ─────────────
  {
    id: "gv",
    names: ["gv"],
    group: "Variáveis",
    signature: "gv(nome, padrão)",
    summary:
      "Valor de uma variável global do projeto (aba Globais). Ignora maiúsculas/minúsculas no nome.",
    args: [
      { name: "nome", desc: "Nome da global." },
      { name: "padrão", desc: "Texto devolvido se a global não existir.", optional: true },
    ],
    notes: [
      "Tipos de global: SWITCH (1/0), NUMBER, TEXT e COLOR (#AARRGGBB).",
      "Globais marcadas como expostas podem ser trocadas pelo clique de uma camada (alternar/definir/próximo da lista).",
    ],
    examples: [
      { input: "$gv(fmt)$", expected: "dd MMMM" },
      { input: "$gv(missing, padrão)$", expected: "padrão" },
      { input: "$if(gv(show_stats), 'on', 'off')$", expected: "on" },
    ],
  },

  // ───────────── Sistema ─────────────
  {
    id: "rm",
    names: ["rm", "sys", "si"],
    group: "Sistema",
    signature: "rm(chave)",
    summary:
      "Telemetria do computador. rm aceita só chaves do sistema; sys e si aceitam também as de bateria (bi).",
    args: [{ name: "chave", desc: "O que ler (tabela). Padrão: cpu.", optional: true }],
    optionsTitle: "Chaves",
    options: [
      { key: "cpu · cpuutil", desc: "Uso de CPU em %, inteiro." },
      { key: "ram · mem · mused", desc: "Uso de RAM em %, inteiro." },
      { key: "ram_gb · mused_gb", desc: "RAM usada em GB, 1 casa decimal." },
      { key: "ram_total_gb · mtot_gb", desc: "RAM total em GB, 1 casa decimal." },
      { key: "host · model", desc: "Nome do computador." },
    ],
    notes: ["Chave desconhecida devolve texto vazio. A telemetria atualiza a cada ~1,5 s."],
    examples: [
      { input: "$rm(cpu)$%", expected: "43%" },
      { input: "$sys(ram)$", expected: "63" },
      { input: "$rm(ram_gb)$/$rm(ram_total_gb)$ GB", expected: "10.3/16.0 GB" },
      { input: "$rm(host)$", expected: "PRISM-PC" },
      { input: "$sys(bat)$", expected: "77", note: "sys também lê a bateria" },
    ],
  },
  {
    id: "bi",
    names: ["bi"],
    group: "Sistema",
    signature: "bi(chave)",
    summary: "Estado da bateria.",
    args: [{ name: "chave", desc: "O que ler (tabela). Padrão: level.", optional: true }],
    optionsTitle: "Chaves",
    options: [
      { key: "level · bat", desc: "Carga em %." },
      { key: "charging", desc: "1 se está carregando, 0 se não." },
      { key: "status", desc: "Texto: “Carregando” ou “Na Bateria”." },
    ],
    examples: [
      { input: "$bi(level)$%", expected: "77%" },
      { input: "$bi(charging)$", expected: "0" },
      { input: "$bi(status)$", expected: "Na Bateria" },
    ],
  },

  // ───────────── Mídia ─────────────
  {
    id: "mi",
    names: ["mi"],
    group: "Mídia",
    signature: "mi(chave)",
    summary: "Mídia em reprodução no Windows (Spotify, navegador, players).",
    args: [{ name: "chave", desc: "O que ler (tabela). Padrão: title.", optional: true }],
    optionsTitle: "Chaves",
    options: [
      { key: "title", desc: "Título da faixa (“Sem mídia” se não houver)." },
      { key: "artist", desc: "Artista." },
      { key: "album", desc: "Álbum." },
      { key: "cover", desc: "Caminho da capa em disco (vazio se não houver)." },
      { key: "state", desc: "PLAYING ou PAUSED." },
      { key: "playing", desc: "1 se está tocando, 0 se não." },
      { key: "pos", desc: "Posição atual, mm:ss (avança sozinha enquanto toca)." },
      { key: "dur", desc: "Duração total, mm:ss." },
      { key: "pct", desc: "Progresso de 0 a 100, inteiro." },
    ],
    examples: [
      { input: "$mi(title)$ — $mi(artist)$", expected: "Song — AC/DC" },
      { input: "$mi(pos)$/$mi(dur)$", expected: "01:10/03:20" },
      { input: "$mi(pct)$", expected: "35" },
      { input: "$mi(state)$", expected: "PLAYING" },
    ],
  },

  // ───────────── Clima e sol ─────────────
  {
    id: "wi",
    names: ["wi"],
    group: "Clima e sol",
    signature: "wi(chave)",
    summary: "Clima local.",
    args: [{ name: "chave", desc: "O que ler (tabela). Padrão: temp.", optional: true }],
    optionsTitle: "Chaves",
    options: [
      { key: "temp", desc: "Temperatura em °C, inteira; “--” enquanto não há dados." },
      { key: "cond", desc: "Condição em texto (ex.: Céu limpo)." },
      { key: "code", desc: "Código numérico da condição." },
      { key: "city", desc: "Cidade (“Local” se desconhecida)." },
      { key: "isday", desc: "1 de dia, 0 de noite." },
    ],
    examples: [
      { input: "$wi(temp)$°C", expected: "--°C", note: "No exemplo ainda não há dados de clima" },
      { input: "$wi(cond)$", expected: "Céu limpo" },
      { input: "$wi(city)$", expected: "Local" },
    ],
  },
  {
    id: "ai",
    names: ["ai"],
    group: "Clima e sol",
    signature: "ai(chave)",
    summary: "Astronomia simplificada: nascer/pôr do sol e estação do ano.",
    args: [{ name: "chave", desc: "O que ler (tabela). Padrão e chave desconhecida: isday.", optional: true }],
    optionsTitle: "Chaves",
    options: [
      { key: "sunrise", desc: "Nascer do sol (fixo: 06:00)." },
      { key: "sunset", desc: "Pôr do sol (fixo: 18:00)." },
      { key: "season", desc: "Estação (hemisfério sul): Verão, Outono, Inverno, Primavera." },
      { key: "isday", desc: "1 de dia, 0 de noite (mesmo dado de wi(isday))." },
    ],
    examples: [
      { input: "$ai(season)$", expected: "Primavera" },
      { input: "$ai(sunrise)$ - $ai(sunset)$", expected: "06:00 - 18:00" },
      { input: "$ai(isday)$", expected: "1" },
    ],
  },

  // ───────────── Lógica ─────────────
  {
    id: "if",
    names: ["if"],
    group: "Lógica",
    signature: "if(cond, então, cond2, então2, …, senão)",
    summary:
      "Escolhe um valor. Testa as condições em ordem e devolve o resultado da primeira verdadeira; o último argumento sem par é o “senão”.",
    args: [
      { name: "cond", desc: "Condição. Verdadeira se for número ≠ 0 ou texto que não seja vazio nem “false”." },
      { name: "então", desc: "Valor se a condição for verdadeira." },
      { name: "senão", desc: "Valor final se nenhuma condição valer.", optional: true },
    ],
    notes: [
      "Comparações: = == != <> < > <= >=. Números comparam como números; texto ignora maiúsculas em igualdade.",
      "Combine com && (e), || (ou) e ! (não).",
    ],
    examples: [
      { input: '$if(rm(cpu) > 30, gv(accent), "#FFFFFFFF")$', expected: "#FF00D2FF" },
      { input: "$if(bi(level) < 20, baixa, bi(level) < 50, média, ok)$", expected: "ok" },
      { input: "$if(mi(state) = playing, ▶, ⏸)$", expected: "▶" },
    ],
  },

  // ───────────── Cor ─────────────
  {
    id: "ce",
    names: ["ce"],
    group: "Cor",
    signature: "ce(cor, operação, …)",
    summary:
      "Edita uma cor. Aceita #RRGGBB ou #AARRGGBB e sempre devolve #AARRGGBB (alfa primeiro).",
    args: [
      { name: "cor", desc: "Cor de entrada (padrão: branco opaco)." },
      { name: "operação", desc: "alpha, mix, contrast ou lum (tabela). Sem operação: só normaliza a cor.", optional: true },
    ],
    optionsTitle: "Operações",
    options: [
      { key: "alpha, %", desc: "Troca a opacidade (0–100 %)." },
      { key: "mix, cor2, %", desc: "Mistura com cor2; % = quanto de cor2 (padrão 50)." },
      { key: "contrast", desc: "Preto ou branco, o que contrasta melhor com a cor." },
      { key: "lum", desc: "Luminosidade de 0 a 100." },
    ],
    examples: [
      { input: "$ce(#FF0000)$", expected: "#FFFF0000", note: "Normaliza para #AARRGGBB" },
      { input: "$ce(#FF0000, alpha, 50)$", expected: "#80FF0000" },
      { input: "$ce(#000000, mix, #FFFFFF, 50)$", expected: "#FF808080" },
      { input: "$ce(#FFFFFF, contrast)$", expected: "#FF000000" },
      { input: "$ce(#FFFFFF, lum)$", expected: "100" },
    ],
  },

  // ───────────── Texto ─────────────
  {
    id: "tc",
    names: ["tc"],
    group: "Texto",
    signature: "tc(modo, texto, …)",
    summary: "Transformações de texto.",
    args: [
      { name: "modo", desc: "Qual transformação (tabela)." },
      { name: "texto", desc: "Texto de entrada (pode ser outra fórmula)." },
    ],
    optionsTitle: "Modos",
    options: [
      { key: "up · upper", desc: "MAIÚSCULAS." },
      { key: "low · lower", desc: "minúsculas." },
      { key: "cap · capitalize", desc: "Primeira Letra De Cada Palavra." },
      { key: "len", desc: "Número de caracteres." },
      { key: "cut, n", desc: "Primeiros n caracteres." },
      { key: "ell, n", desc: "Corta em n caracteres e termina com “…” se passar." },
      { key: "split, sep, i", desc: "Parte i (começa em 0) do texto separado por sep." },
      { key: "lpad · rpad, largura, char", desc: "Completa à esquerda/direita até a largura (máx. 64); padrão 0 / espaço." },
    ],
    notes: ["Modo desconhecido devolve o texto sem alterar."],
    examples: [
      { input: "$tc(up, df(EEEE, dd MMMM))$", expected: "SÁBADO, 21 NOVEMBRO" },
      { input: "$tc(cap, 'olá mundo')$", expected: "Olá Mundo" },
      { input: "$tc(len, 'ação')$", expected: "4" },
      { input: "$tc(ell, 'abcdefgh', 5)$", expected: "abcd…" },
      { input: "$tc(split, 'a-b-c', '-', 1)$", expected: "b" },
      { input: "$tc(lpad, 7, 3)$", expected: "007" },
      { input: "$tc(cut, 'abcdef', 3)$", expected: "abc" },
    ],
  },

  // ───────────── Matemática ─────────────
  {
    id: "mu",
    names: ["mu"],
    group: "Matemática",
    signature: "mu(operação, números…)",
    summary: "Funções matemáticas. Argumentos que não são número são ignorados.",
    args: [
      { name: "operação", desc: "Qual cálculo (tabela)." },
      { name: "números", desc: "Um ou mais valores." },
    ],
    optionsTitle: "Operações",
    options: [
      { key: "min · max, a, b, …", desc: "Menor / maior valor." },
      { key: "abs · round · floor · ceil, x", desc: "Valor absoluto, arredonda (0,5 sobe), para baixo, para cima." },
      { key: "pow, x, y", desc: "x elevado a y." },
      { key: "sqrt, x", desc: "Raiz quadrada (negativo vira 0)." },
      { key: "sin · cos, graus", desc: "Seno / cosseno com o ângulo em graus." },
      { key: "clamp, x, mín, máx", desc: "Limita x entre mín e máx (padrão 0 e 100)." },
    ],
    notes: ["Resultados saem com até 2 casas decimais, sem zeros sobrando."],
    examples: [
      { input: "$mu(round, 2.5)$", expected: "3" },
      { input: "$mu(max, 3, 9, 4)$", expected: "9" },
      { input: "$mu(clamp, 150, 0, 100)$", expected: "100" },
      { input: "$mu(pow, 2, 10)$", expected: "1024" },
      { input: "$mu(sqrt, 2)$", expected: "1.41" },
      { input: "$mu(sin, 90)$", expected: "1" },
    ],
  },
  {
    id: "fl",
    names: ["fl"],
    group: "Matemática",
    signature: "fl(início, fim, passo, \"corpo\", separador)",
    summary:
      "Repete um corpo de fórmula. Dentro do corpo a variável i vale o contador atual (até 64 voltas, no máximo 2 níveis de fl aninhados).",
    args: [
      { name: "início", desc: "Primeiro valor de i (padrão 1)." },
      { name: "fim", desc: "Último valor de i; se for menor que o início, conta para trás." },
      { name: "passo", desc: "Quanto i anda a cada volta (sempre ≥ 1)." },
      { name: "corpo", desc: "Fórmula entre aspas, avaliada a cada volta." },
      { name: "separador", desc: "Texto entre as voltas.", optional: true },
    ],
    examples: [
      { input: '$fl(1, 3, 1, "i * 2", ",")$', expected: "2,4,6" },
      { input: '$fl(1, 5, 2, "i")$', expected: "135", note: "Sem separador" },
    ],
  },
];

// ───────────── Sintaxe geral ─────────────

export interface FxSyntaxSection {
  id: string;
  title: string;
  paragraphs: readonly string[];
  examples?: readonly FxExample[];
}

export const FX_SYNTAX_SECTIONS: readonly FxSyntaxSection[] = [
  {
    id: "sintaxe-basica",
    title: "Como escrever uma fórmula",
    paragraphs: [
      "Uma fórmula é um texto comum com trechos entre cifrões: $…$. O que está fora dos cifrões fica como está; o que está dentro é calculado e substituído pelo resultado.",
      "$$ (dois cifrões seguidos) escreve um cifrão literal. Um $ sem par também fica como texto.",
      "Funções têm 2 letras minúsculas e parênteses: $df(HH:mm)$. Funções podem ficar dentro de outras: $tc(up, df(EEEE))$.",
      "Dentro dos cifrões, palavras soltas viram texto: $Olá mundo$ escreve “Olá mundo”. Use aspas simples ou duplas para texto com símbolos: 'a, b'.",
    ],
    examples: [
      { input: "Hoje é $df(EEEE)$, $df(dd)$ de $df(MMMM)$.", expected: "Hoje é Sábado, 21 de Novembro." },
      { input: "Preço: $$5", expected: "Preço: $5" },
      { input: "$Olá mundo$", expected: "Olá mundo" },
      { input: "$tc(up, df(EEEE, dd MMMM))$ · $rm(cpu)$%", expected: "SÁBADO, 21 NOVEMBRO · 43%" },
    ],
  },
  {
    id: "operadores",
    title: "Operadores e comparações",
    paragraphs: [
      "Aritmética: + − * / %, com a precedência de sempre; parênteses agrupam. Divisão por zero dá 0.",
      "Lógica: && (e), || (ou), ! (não). Comparação: = == != <> < > <= >=.",
      "Somar texto concatena. Itens lado a lado também se juntam, preservando o espaço original.",
      "Resultados numéricos têm até 2 casas decimais e nunca terminam com zeros inúteis (2.5, não 2.50).",
    ],
    examples: [
      { input: "$2 + 3 * 4$", expected: "14" },
      { input: "$(2 + 3) * 4$", expected: "20" },
      { input: "$10 / 3$", expected: "3.33" },
      { input: "$7 % 4$", expected: "3" },
      { input: "$1 / 0$", expected: "0" },
      { input: "$5 > 3$ $'abc' < 'abd'$", expected: "1 1" },
      { input: "$!0$ $1 && 0$ $1 || 0$", expected: "1 0 1" },
      { input: '$"a" + "b"$', expected: "ab" },
      { input: "$'Olá, ' + mi(artist)$", expected: "Olá, AC/DC" },
    ],
  },
];

// ───────────── Tokens de df() e tf() ─────────────

export interface FxTokenDoc {
  token: string;
  meaning: string;
  /** Resultado com `FX_SAMPLE_CONTEXT_NOTE` (o teste avalia `$df(token)$` / `$tf(…)$`). */
  sample: string;
}

export const FX_DATE_TOKENS: readonly FxTokenDoc[] = [
  { token: "yyyy", meaning: "Ano com 4 dígitos", sample: "2026" },
  { token: "yy", meaning: "Ano com 2 dígitos", sample: "26" },
  { token: "MMMM", meaning: "Mês por extenso", sample: "Novembro" },
  { token: "MMM", meaning: "Mês abreviado", sample: "Nov" },
  { token: "MM", meaning: "Mês com 2 dígitos", sample: "11" },
  { token: "M", meaning: "Mês sem zero à esquerda", sample: "11" },
  { token: "EEEE", meaning: "Dia da semana por extenso", sample: "Sábado" },
  { token: "EEE", meaning: "Dia da semana abreviado", sample: "Sáb" },
  { token: "dd", meaning: "Dia do mês com 2 dígitos", sample: "21" },
  { token: "d", meaning: "Dia do mês sem zero à esquerda", sample: "21" },
  { token: "HH", meaning: "Hora 00–23 com 2 dígitos", sample: "14" },
  { token: "H", meaning: "Hora 0–23", sample: "14" },
  { token: "hh", meaning: "Hora 01–12 com 2 dígitos", sample: "02" },
  { token: "h", meaning: "Hora 1–12", sample: "2" },
  { token: "mm", meaning: "Minutos com 2 dígitos", sample: "05" },
  { token: "m", meaning: "Minutos sem zero à esquerda", sample: "5" },
  { token: "ss", meaning: "Segundos com 2 dígitos", sample: "09" },
  { token: "s", meaning: "Segundos sem zero à esquerda", sample: "9" },
  { token: "a", meaning: "AM / PM", sample: "PM" },
];

export const FX_DURATION_TOKENS: readonly { token: string; meaning: string }[] = [
  { token: "D · DD", meaning: "Dias (1 ou 2 dígitos)" },
  { token: "H · HH", meaning: "Horas" },
  { token: "m · mm", meaning: "Minutos" },
  { token: "s · ss", meaning: "Segundos" },
];

// ───────────── FX por propriedade ─────────────

export interface FxPropertyDoc {
  /** Nome do campo no inspetor. */
  label: string;
  /** Chave interna em `layer.fx`. */
  key: "opacity" | "rotation" | "color" | "visible";
  desc: string;
  example: FxExample;
}

export const FX_PROPERTY_DOCS: readonly FxPropertyDoc[] = [
  {
    label: "Opacidade",
    key: "opacity",
    desc: "Número de 0 a 1 (um “%” no fim é ignorado). Valores fora do intervalo são limitados.",
    example: { input: "$if(mi(playing), 1, 0.3)$", expected: "1", note: "Apagada quando nada toca" },
  },
  {
    label: "Rotação",
    key: "rotation",
    desc: "Ângulo em graus (pode passar de 360).",
    example: { input: "$mi(pct) * 3.6$", expected: "126", note: "Uma volta ao longo da música" },
  },
  {
    label: "Cor (texto e SVG)",
    key: "color",
    desc: "Cor #RRGGBB ou #AARRGGBB. Vazio mantém a cor fixa da camada.",
    example: { input: "$if(rm(cpu) > 70, #FFFF6B6B, #FFFFFFFF)$", expected: "#FFFFFFFF", note: "Vermelho se a CPU passar de 70%" },
  },
  {
    label: "Visível",
    key: "visible",
    desc: "A camada some se o resultado for 0, false ou vazio.",
    example: { input: "$mi(playing)$", expected: "1", note: "Só aparece enquanto toca" },
  },
];

/** Receitas prontas (seção “Exemplos prontos” da Ajuda); avaliadas pelos testes como o resto. */
export const FX_RECIPES: readonly (FxExample & { title: string })[] = [
  { title: "Relógio", input: "$df(HH:mm)$", expected: "14:05" },
  { title: "Data por extenso", input: "$tc(up, df(EEEE, dd MMMM))$", expected: "SÁBADO, 21 NOVEMBRO" },
  { title: "Música tocando", input: "$mi(title)$ — $mi(artist)$", expected: "Song — AC/DC" },
  { title: "Progresso da música", input: "$mi(pos)$ / $mi(dur)$", expected: "01:10 / 03:20" },
  { title: "CPU e RAM", input: "CPU $rm(cpu)$% · RAM $rm(ram)$%", expected: "CPU 43% · RAM 63%" },
  { title: "Bateria", input: "$bi(level)$% · $bi(status)$", expected: "77% · Na Bateria" },
  { title: "Saudação", input: "$if(df(H) < 12, Bom dia, df(H) < 18, Boa tarde, Boa noite)$", expected: "Boa tarde" },
  { title: "Cor de alerta", input: "$if(rm(cpu) > 70, #FFFF6B6B, #FFFFFFFF)$", expected: "#FFFFFFFF" },
];
