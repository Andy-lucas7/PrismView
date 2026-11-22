// Uso: node tools/autorun/run.mjs [--from E2-L1] [--only E1-L3] [--list] [--max-turns 60] [--max-fix 3]
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const auto = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(auto, "../..");
const app = path.join(root, "apps", "prismview-studio");
const logs = path.join(auto, "logs");
const doneFile = path.join(auto, "done.txt");
const handoff = path.join(auto, "HANDOFF.md");
process.chdir(root);
fs.mkdirSync(logs, { recursive: true });
if (!fs.existsSync(doneFile)) fs.writeFileSync(doneFile, "");

const argv = process.argv.slice(2);
const opt = (n, d) => { const i = argv.indexOf(`--${n}`); return i >= 0 ? argv[i + 1] : d; };
const FROM = opt("from", ""), ONLY = opt("only", "");
const MAX_TURNS = Number(opt("max-turns", 60)), MAX_FIX = Number(opt("max-fix", 3));
const LIST = argv.includes("--list");

const ALLOWED = "Read,Edit,Write,Glob,Grep,Skill,Bash(node tools/autorun/gate.mjs:*),Bash(git status:*),Bash(git diff:*)";
const DENIED = "Read(./tools/autorun/logs/**),Read(./target/**),Read(./**/node_modules/**),Read(./**/dist/**),Read(./.git/**)";
const EXCLUDE = ["tools/autorun/logs", "tools/autorun/done.txt", "tools/autorun/gate-errors.txt", "tools/autorun/baseline-ts.txt", "tools/autorun/baseline-rust.txt"];

const q = (s) => `"${String(s).replace(/"/g, '\\"')}"`;
const sh = (cmd, input) => {
  const r = spawnSync(cmd, { shell: true, cwd: root, input, encoding: "utf8", maxBuffer: 1 << 28 });
  return { code: r.status ?? 1, out: `${r.stdout ?? ""}`, err: `${r.stderr ?? ""}` };
};
const tail = (s, n) => s.split(/\r?\n/).filter(Boolean).slice(-n).join("\n");
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
const c = { d: (s) => `\x1b[2m${s}\x1b[0m`, r: (s) => `\x1b[31m${s}\x1b[0m`, g: (s) => `\x1b[32m${s}\x1b[0m`, y: (s) => `\x1b[33m${s}\x1b[0m`, c: (s) => `\x1b[36m${s}\x1b[0m` };

const rows = [];
let total = 0;
const writeReport = () =>
  fs.writeFileSync(path.join(logs, "REPORT.md"),
    ["# Autorun report", "", "| lote | status | correcoes | custo acum. |", "|---|---|---|---|", ...rows, "", `total ~$${total.toFixed(2)}`].join("\n"));
const stopRun = (msg) => { console.log(c.r(`PARADO: ${msg}`)); writeReport(); process.exit(1); };

function getMeta(file) {
  const m = {};
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (line.trim() === "---") break;
    const x = line.match(/^(\w+):\s*(.+)$/);
    if (x) m[x[1]] = x[2].trim();
  }
  return m;
}

const bar = (done, all, w = 20) => `[${"█".repeat(Math.round((w * done) / all))}${"░".repeat(w - Math.round((w * done) / all))}] ${done}/${all}`;
const fmtTime = (ms) => { const s = Math.floor(ms / 1000); return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`; };
const describeTool = (p) => {
  const i = p.input ?? {};
  const target = i.file_path ? path.basename(String(i.file_path)) : i.pattern ? String(i.pattern) : i.command ? String(i.command) : "";
  return `${p.name} ${target}`.trim().slice(0, 70);
};

// Executa o claude em streaming: linha de status viva (turno, tempo, ferramenta atual) + log .jsonl.
function runStream(cmd, input, label, logPath) {
  return new Promise((resolve) => {
    const p = spawn(cmd, { shell: true, cwd: root });
    const logStream = fs.createWriteStream(logPath);
    let buf = "", err = "", result = null, turns = 0, last = "iniciando";
    const t0 = Date.now();
    const render = () => {
      const line = `${label} · turno ${turns}/${MAX_TURNS} · ${fmtTime(Date.now() - t0)} · ${last}`;
      process.stdout.write(`\r\x1b[2K${c.d(line.slice(0, (process.stdout.columns || 120) - 1))}`);
    };
    const tick = setInterval(render, 1000);
    const onLine = (line) => {
      if (!line) return;
      logStream.write(line + "\n");
      let e; try { e = JSON.parse(line); } catch { return; }
      if (e.type === "assistant") {
        turns++;
        const parts = e.message?.content ?? [];
        const tool = parts.filter((x) => x.type === "tool_use").pop();
        last = tool ? describeTool(tool) : "pensando...";
        render();
      } else if (e.type === "result") result = e;
    };
    p.stdout.on("data", (d) => { buf += d; let i; while ((i = buf.indexOf("\n")) >= 0) { onLine(buf.slice(0, i).trim()); buf = buf.slice(i + 1); } });
    p.stderr.on("data", (d) => { err += d; });
    p.on("close", (code) => {
      clearInterval(tick);
      if (buf.trim()) onLine(buf.trim());
      logStream.end();
      process.stdout.write("\r\x1b[2K");
      resolve({ code: code ?? 1, err, result, turns, ms: Date.now() - t0 });
    });
    p.stdin.write(input);
    p.stdin.end();
  });
}

// Sessão claude com retry em erro transitório. Todo prompt começa com /caveman.
async function invokeClaude(model, effort, prompt, logName, label = logName) {
  const full = "/caveman " + prompt;
  const cmd = ["claude", "-p", "--model", model, "--effort", effort, "--max-turns", MAX_TURNS,
    "--permission-mode", "acceptEdits", "--allowedTools", q(ALLOWED), "--disallowedTools", q(DENIED),
    "--output-format", "stream-json", "--verbose"].join(" ");
  for (let t = 1; t <= 3; t++) {
    const r = await runStream(cmd, full, label, path.join(logs, `${logName}.try${t}.jsonl`));
    const j = r.result;
    const cost = Number(j?.total_cost_usd ?? 0) || 0;
    const sub = String(j?.subtype ?? "");
    const isErr = r.code !== 0 || !j || j.is_error === true;
    const transient = isErr && /rate.?limit|overloaded|\b429\b|\b529\b|ECONNRESET|ETIMEDOUT|timed? ?out|temporarily/i.test(`${j?.result ?? ""} ${r.err}`);
    if (transient && t < 3) {
      console.log(c.y(`erro transitorio; aguardando ${60 * t}s (tentativa ${t}/3)`));
      sleep(60000 * t);
      continue;
    }
    console.log(c.d(`  sessao: ${r.turns} turnos, ${fmtTime(r.ms)}, $${cost.toFixed(2)}`));
    return { ok: !isErr, cost, sub, why: tail(r.err || String(j?.result ?? ""), 3) };
  }
}

function runGate(gate) {
  const r = sh(`node tools/autorun/gate.mjs ${gate}`);
  let count = 0;
  const ef = path.join(auto, "gate-errors.txt");
  if (fs.existsSync(ef)) count = Number(fs.readFileSync(ef, "utf8").match(/NEW_ERRORS=(\d+)/)?.[1] ?? 0);
  return { ok: r.code === 0, count };
}

const saveFailed = (id) => {
  sh(`git stash push -u -m autorun-fail-${id} -- . ${q(":!tools/autorun")}`);
  console.log(c.y(`trabalho parcial no 'git stash' (autorun-fail-${id}); arvore limpa. Retome com --from ${id}`));
};

const batches = fs.readdirSync(path.join(auto, "batches")).filter((f) => f.endsWith(".md")).sort();

if (LIST) {
  for (const f of batches) {
    const m = getMeta(path.join(auto, "batches", f));
    console.log(f.replace(/\.md$/, "").padEnd(30), (m.model ?? "").padEnd(7), (m.effort ?? "").padEnd(7), m.gate ?? "");
  }
  process.exit(0);
}

// --- preparação ---
if (!fs.existsSync(path.join(app, "node_modules"))) stopRun("rode 'npm install' em apps/prismview-studio antes (sem isso o baseline seria falso)");
const branch = "autorun/sala3d";
if (sh("git rev-parse --abbrev-ref HEAD").out.trim() !== branch) {
  if (sh(`git rev-parse --verify ${branch}`).code === 0) sh(`git checkout ${branch}`); else sh(`git checkout -b ${branch}`);
}
// Árvore suja (trabalho anterior não commitado): checkpoint antes de começar. Sem isso o commit do
// 1º lote misturaria tudo e um 'git stash' de falha guardaria o trabalho antigo junto.
const pathspec = `. ${EXCLUDE.map((e) => q(":!" + e)).join(" ")}`;
if (sh(`git status --porcelain -- ${pathspec}`).out.trim()) {
  console.log(c.y("arvore suja: criando commit de checkpoint antes de comecar"));
  sh(`git add -A -- ${pathspec}`);
  const cp = sh(`git commit -qm ${q("autorun: checkpoint antes de iniciar")}`);
  if (cp.code !== 0) stopRun(`nao consegui commitar o checkpoint (configure git user.name/user.email): ${tail(cp.err || cp.out, 3)}`);
}
for (const k of ["ts", "rust"]) {
  if (!fs.existsSync(path.join(auto, `baseline-${k}.txt`))) {
    console.log(c.c(`gerando baseline de erros pre-existentes (${k})...`));
    const r = sh(`node tools/autorun/gate.mjs ${k} --baseline`);
    console.log(r.out.trim());
  }
}

let started = FROM === "";
for (const file of batches) {
  const name = file.replace(/\.md$/, "");
  const id = name.split("-").slice(0, 2).join("-");
  if (!started) { if (id === FROM) started = true; else continue; }
  if (ONLY && id !== ONLY) continue;
  if (fs.readFileSync(doneFile, "utf8").split(/\r?\n/).includes(id)) { console.log(`pulando ${id} (feito)`); continue; }

  const m = getMeta(path.join(auto, "batches", file));
  const gate = m.gate;
  const gateCmd = `node tools/autorun/gate.mjs ${gate}`;
  const doneN = fs.readFileSync(doneFile, "utf8").split(/\r?\n/).filter(Boolean).length;
  const loteT0 = Date.now(), costBefore = total;
  console.log(c.c(`\n${bar(doneN, batches.length)}  ${name} | ${m.model}/${m.effort} | gate=${gate}`));
  const handoffBefore = fs.statSync(handoff).mtimeMs;

  const prompt = `Execute SOMENTE o lote ${id}. Leia, nesta ordem: CLAUDE.md, tools/autorun/CONTEXT.md, tools/autorun/HANDOFF.md, tools/autorun/batches/${file}.
Regras:
- Implemente tudo que o lote pede, de ponta a ponta. Proibido: TODO, placeholder, mock, stub, adiar, gambiarra no lugar da solucao correta.
- Leia so o necessario (grep por simbolo e leia janelas curtas). Patches pontuais (Edit); nunca reescreva arquivo existente inteiro.
- Nao implemente nada de outros lotes. Se faltar pre-requisito, escreva 'BLOCKED: motivo' no HANDOFF.md e termine.
- Validacao: rode SOMENTE '${gateCmd}' (saida ja condensada, so erros novos). Nunca rode npm run build / cargo check diretos e nunca leia logs brutos ou tools/autorun/logs.
- Se o gate acusar erro, corrija a causa raiz e rode o gate de novo, ate zerar. Proibido silenciar erro com @ts-ignore, 'as any' em massa, #[allow] ou removendo funcionalidade.
- Por ultimo, SOBRESCREVA tools/autorun/HANDOFF.md com no maximo 10 linhas: o que mudou (arquivos/simbolos), decisoes, o que o proximo lote precisa saber.`;

  let r = await invokeClaude(m.model, m.effort, prompt, name, `${id} ${m.model}/${m.effort}`);
  total += r.cost;

  let cont = 0;
  while (r.sub === "error_max_turns" && cont < 2) {
    cont++;
    console.log(c.y(`limite de turnos em ${id}; continuacao ${cont}/2`));
    const stat = tail(sh("git diff --stat").out, 15);
    r = await invokeClaude(m.model, m.effort,
      `Continue o lote ${id} (a sessao anterior atingiu o limite de turnos). Estado (git diff --stat):\n${stat}\nLeia CLAUDE.md, tools/autorun/CONTEXT.md e tools/autorun/batches/${file}. Confira o que falta com grep/janelas curtas, termine o lote, valide com '${gateCmd}' e atualize HANDOFF.md (max 10 linhas).`,
      `${name}.cont${cont}`, `${id} continuacao ${cont}`);
    total += r.cost;
  }
  if (!r.ok) { rows.push(`| ${id} | FALHOU (claude) | - | - |`); saveFailed(id); stopRun(`sessao claude falhou em ${id}: ${r.why} (veja logs/${name}.try*.jsonl)`); }
  if (/BLOCKED/.test(fs.readFileSync(handoff, "utf8"))) { rows.push(`| ${id} | BLOQUEADO | - | - |`); saveFailed(id); stopRun(`lote ${id} bloqueado (veja tools/autorun/HANDOFF.md)`); }

  console.log(c.d(`  validando gate '${gate}' (build/cargo pode levar alguns minutos)...`));
  let g = runGate(gate);
  console.log(c.d(`  gate: ${g.ok ? "ok" : g.count + " erros novos"}`));
  let k = 0, prev = g.count;
  while (!g.ok && k < MAX_FIX) {
    k++;
    const [fm, fe] = k === 1 ? ["sonnet", "high"] : ["opus", "medium"];
    console.log(c.y(`gate falhou em ${id} (${g.count} erros novos); correcao ${k}/${MAX_FIX} com ${fm}/${fe}`));
    const stat = tail(sh("git diff --stat").out, 12);
    const fix = `O gate '${gate}' do lote ${id} falhou com ${g.count} erros NOVOS (correcao ${k}/${MAX_FIX}).
Leia SOMENTE tools/autorun/gate-errors.txt (ja condensado) e, para cada erro, uma janela de ~40 linhas ao redor (Read com offset/limit). Arquivos do lote (git diff --stat):
${stat}
Corrija a causa raiz com patches pontuais, sem mudar o escopo. Proibido silenciar com @ts-ignore, 'as any' em massa, #[allow], stub, ou apagar funcionalidade. Se o erro vier de tipo/contrato desalinhado entre arquivos, alinhe pelo CONTRATO FINAL em tools/autorun/CONTEXT.md.
Revalide com '${gateCmd}' (unico comando de build permitido). Nunca leia logs brutos.`;
    const r2 = await invokeClaude(fm, fe, fix, `${name}.fix${k}`, `${id} correcao ${k}/${MAX_FIX} ${fm}/${fe}`);
    total += r2.cost;
    g = runGate(gate);
    if (!g.ok && k >= 2 && g.count >= prev) { console.log(c.y(`sem progresso nas correcoes de ${id}`)); break; }
    prev = g.count;
  }
  if (!g.ok) {
    rows.push(`| ${id} | GATE FALHOU (${g.count} erros) | ${k} | ~$${total.toFixed(2)} |`);
    saveFailed(id);
    stopRun(`gate '${gate}' ainda falha em ${id}; veja tools/autorun/gate-errors.txt`);
  }

  if (fs.statSync(handoff).mtimeMs === handoffBefore) {
    fs.writeFileSync(handoff, `AUTO: lote ${id} concluido sem handoff do modelo. Arquivos:\n${tail(sh("git diff --stat").out, 8)}\n`);
  }

  sh(`git add -A -- . ${EXCLUDE.map((e) => q(":!" + e)).join(" ")}`);
  sh(`git commit -qm ${q(`autorun ${id}: ${name}`)}`);
  fs.appendFileSync(doneFile, `${id}\n`);
  rows.push(`| ${id} | ok | ${k} | ~$${total.toFixed(2)} |`);
  console.log(c.g(`ok ${id} | ${fmtTime(Date.now() - loteT0)} | lote $${(total - costBefore).toFixed(2)} | acumulado $${total.toFixed(2)} | correcoes: ${k}`));
  console.log(c.d("  HANDOFF:\n" + fs.readFileSync(handoff, "utf8").trim().split(/\r?\n/).map((l) => "    " + l).join("\n")));
}
writeReport();
console.log(c.g(`FIM. Relatorio em tools/autorun/logs/REPORT.md (custo total ~$${total.toFixed(2)})`));
