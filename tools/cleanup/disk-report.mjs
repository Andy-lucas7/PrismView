// Uso:
//   node tools/cleanup/disk-report.mjs                  -> SÓ RELATÓRIO (nada é apagado)
//   node tools/cleanup/disk-report.mjs --clean          -> apaga só a lista "LIXO SEGURO" (pede confirmação)
//   node tools/cleanup/disk-report.mjs --clean --dev-caches -> inclui caches de dev (npm/pip/cargo), regeneráveis
// Nunca apaga pasta de aplicativo, documento, AppData inteiro nem arquivo do sistema.
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import readline from "node:readline";

const env = process.env;
const LOCAL = env.LOCALAPPDATA ?? path.join(os.homedir(), "AppData", "Local");
const ROAM = env.APPDATA ?? path.join(os.homedir(), "AppData", "Roaming");
const PD = env.ProgramData ?? "C:\\ProgramData";
const USER = env.USERPROFILE ?? os.homedir();
const WIN = env.SystemRoot ?? "C:\\Windows";
const TEMP = env.TEMP ?? os.tmpdir();
const args = new Set(process.argv.slice(2));
const CLEAN = args.has("--clean"), DEV = args.has("--dev-caches");
const DAY = 24 * 3600 * 1000, BIG = 500 * 1024 * 1024;

const col = (n) => (s) => `\x1b[${n}m${s}\x1b[0m`;
const dim = col(2), red = col(31), green = col(32), yellow = col(33), cyan = col(36), bold = col(1);
const fmt = (b) => (b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(2)} GB` : `${(b / (1 << 20)).toFixed(0)} MB`);
const exists = (p) => { try { return fs.existsSync(p); } catch { return false; } };
const J = (...a) => path.join(...a);

// ---------- lista de LIXO SEGURO (whitelist; nada fora daqui é apagado) ----------
const safe = [];
const add = (label, dir, o = {}) => { if (dir && exists(dir)) safe.push({ label, dir, ...o }); };
add("Temp do usuario", TEMP, { minAge: DAY });
add("Temp do Windows (precisa admin)", J(WIN, "Temp"), { minAge: DAY });
add("NVIDIA DXCache", J(LOCAL, "NVIDIA", "DXCache"));
add("NVIDIA GLCache", J(LOCAL, "NVIDIA", "GLCache"));
add("NVIDIA NV_Cache", J(PD, "NVIDIA Corporation", "NV_Cache"));
add("NVIDIA ComputeCache", J(ROAM, "NVIDIA", "ComputeCache"));
add("Cache de shaders D3D", J(LOCAL, "D3DSCache"));
add("Cache de shaders AMD", J(LOCAL, "AMD", "DxCache"));
add("Cache de shaders Intel", J(LOCAL, "Intel", "ShaderCache"));
add("INetCache (Windows)", J(LOCAL, "Microsoft", "Windows", "INetCache"));
add("Miniaturas do Explorer", J(LOCAL, "Microsoft", "Windows", "Explorer"), { filter: /^(thumbcache|iconcache).*\.db$/i });
add("Relatorios de erro (WER)", J(LOCAL, "Microsoft", "Windows", "WER"));
add("Relatorios de erro (WER, sistema)", J(PD, "Microsoft", "Windows", "WER", "ReportQueue"));
add("Crash dumps", J(LOCAL, "CrashDumps"));
for (const [name, base] of [["Chrome", J(LOCAL, "Google", "Chrome", "User Data")], ["Edge", J(LOCAL, "Microsoft", "Edge", "User Data")], ["Brave", J(LOCAL, "BraveSoftware", "Brave-Browser", "User Data")]]) {
  if (!exists(base)) continue;
  for (const prof of fs.readdirSync(base).filter((n) => n === "Default" || /^Profile \d+$/.test(n)))
    for (const sub of ["Cache", "Code Cache", "GPUCache", "GrShaderCache", "ShaderCache"])
      add(`${name} ${prof}/${sub}`, J(base, prof, sub));
}
if (DEV) {
  add("npm cache", exists(J(ROAM, "npm-cache")) ? J(ROAM, "npm-cache") : J(LOCAL, "npm-cache"), { dev: true });
  add("pip cache", J(LOCAL, "pip", "Cache"), { dev: true });
  add("Yarn cache", J(LOCAL, "Yarn", "Cache"), { dev: true });
  add("cargo registry/cache", J(USER, ".cargo", "registry", "cache"), { dev: true });
  add("cargo registry/src", J(USER, ".cargo", "registry", "src"), { dev: true });
}

// ---------- varredura ----------
const bigFiles = [];
function walk(dir, depth, node, skip) {
  let total = 0, ents;
  try { ents = fs.readdirSync(dir, { withFileTypes: true }); } catch { return 0; }
  for (const e of ents) {
    if (skip && skip.has(e.name)) continue;
    if (e.isSymbolicLink()) continue;
    const p = J(dir, e.name);
    if (e.isDirectory()) {
      const child = depth < 2 ? { name: e.name, path: p, size: 0, children: [] } : null;
      const s = walk(p, depth + 1, child);
      if (child) { child.size = s; node.children.push(child); }
      total += s;
    } else if (e.isFile()) {
      let s = 0; try { s = fs.lstatSync(p).size; } catch { continue; }
      total += s;
      if (s >= BIG) bigFiles.push({ p, s });
    }
  }
  return total;
}
const sizeOf = (p) => walk(p, 99, null);

function eligible(item) {
  let ents; try { ents = fs.readdirSync(item.dir, { withFileTypes: true }); } catch { return []; }
  const now = Date.now(), out = [];
  for (const e of ents) {
    if (item.filter && !item.filter.test(e.name)) continue;
    const p = J(item.dir, e.name);
    if (item.minAge) { try { if (now - fs.lstatSync(p).mtimeMs < item.minAge) continue; } catch { continue; } }
    out.push({ p, dir: e.isDirectory() && !e.isSymbolicLink() });
  }
  return out;
}

// ---------- classificação do que é pesado ----------
function advise(p) {
  const l = p.toLowerCase();
  if (/\\(code cache|gpucache|shadercache|dxcache|glcache|d3dscache|crashdumps|inetcache)(\\|$)/.test(l) || /\\cache(\\|$)/.test(l)) return ["cache", "geralmente seguro apagar (o app recria)"];
  if (/\.vhdx$|\\packages\\.*(canonical|ubuntu|debian|kali)|\\wsl\\/.test(l)) return ["VM/WSL", "grande: use 'wsl --list -v' e compacte o disco; nao apague manualmente"];
  if (/docker/.test(l)) return ["Docker", "use 'docker system prune'; nao apague a pasta"];
  if (/\\(microsoft|windows)(\\|$)/.test(l)) return ["sistema", "NAO MEXER (use Limpeza de Disco do Windows)"];
  if (/\\\.(cargo|rustup|gradle|m2|nuget|android)(\\|$)|npm|pnpm|yarn|\\pip(\\|$)/.test(l)) return ["dev", "ferramentas: parte e cache regeneravel (veja --dev-caches); SDKs so se nao usa"];
  if (/\\downloads(\\|$)/.test(l)) return ["seus arquivos", "revise manualmente (instaladores, ISOs, zips antigos)"];
  if (/\\(documents|desktop|videos|pictures|music)(\\|$)/.test(l)) return ["seus arquivos", "so voce sabe; mova para o D:/E:"];
  if (/\\programs(\\|$)/.test(l)) return ["app instalado", "desinstale pelo Windows se nao usa mais"];
  return ["revisar", "app/dados: veja se ainda usa; procure subpasta Cache/Logs dentro"];
}
const tagColor = { cache: green, "VM/WSL": yellow, Docker: yellow, sistema: red, dev: cyan, "seus arquivos": yellow, "app instalado": yellow, revisar: (s) => s };

// ---------- execução ----------
console.log(bold("\n=== Espaco livre ==="));
for (const d of ["C:\\", "D:\\", "E:\\", "F:\\"]) {
  try { const s = fs.statfsSync(d); const free = s.bavail * s.bsize, tot = s.blocks * s.bsize; console.log(`  ${d}  livre ${fmt(free)} de ${fmt(tot)}`); } catch { /* unidade ausente */ }
}

console.log(bold("\n=== LIXO SEGURO ===") + dim(CLEAN ? "  (vai apagar apos confirmacao)" : "  (so listando; use --clean para apagar)"));
let safeTotal = 0;
const plan = [];
for (const it of safe) {
  const el = eligible(it);
  let sz = 0;
  for (const x of el) { try { sz += x.dir ? sizeOf(x.p) : fs.lstatSync(x.p).size; } catch { /* sumiu */ } }
  plan.push({ it, el, sz });
  safeTotal += sz;
}
for (const x of plan.sort((a, b) => b.sz - a.sz)) console.log(`  ${fmt(x.sz).padStart(10)}  ${x.it.label}${x.it.dev ? dim("  [dev cache]") : ""}`);
console.log(bold(`  ${fmt(safeTotal).padStart(10)}  TOTAL liberavel com seguranca${DEV ? "" : dim("  (+ caches de dev com --dev-caches)")}`));

console.log(bold("\n=== O que mais pesa (varrendo, pode levar alguns minutos) ==="));
const rows = [];
const roots = [["AppData\\Local", LOCAL, null], ["AppData\\Roaming", ROAM, null], ["ProgramData", PD, null], ["Perfil do usuario", USER, new Set(["AppData"])]];
for (const [label, dir, skip] of roots) {
  if (!exists(dir)) continue;
  process.stdout.write(dim(`  varrendo ${label}...\r`));
  const node = { children: [] };
  walk(dir, 0, node, skip);
  for (const ch of node.children) rows.push({ root: label, ...ch });
}
process.stdout.write("\x1b[2K");
rows.sort((a, b) => b.size - a.size);
for (const r of rows.slice(0, 20)) {
  const [tag, tip] = advise(r.path);
  console.log(`  ${fmt(r.size).padStart(10)}  ${r.root}\\${r.name}  ${(tagColor[tag] ?? ((s) => s))(`[${tag}]`)} ${dim(tip)}`);
}

console.log(bold("\n=== Dentro dos 5 maiores ==="));
for (const r of rows.slice(0, 5)) {
  console.log(`  ${bold(r.root + "\\" + r.name)} ${dim(fmt(r.size))}`);
  for (const ch of r.children.sort((a, b) => b.size - a.size).slice(0, 5)) {
    const [tag] = advise(ch.path);
    console.log(`      ${fmt(ch.size).padStart(10)}  ${ch.name}  ${(tagColor[tag] ?? ((s) => s))(`[${tag}]`)}`);
  }
}

if (bigFiles.length) {
  console.log(bold("\n=== Arquivos gigantes (>= 500 MB) ==="));
  for (const f of bigFiles.sort((a, b) => b.s - a.s).slice(0, 10)) console.log(`  ${fmt(f.s).padStart(10)}  ${f.p}`);
}

console.log(bold("\n=== O que eu removeria primeiro ==="));
console.log(`  1. ${green("--clean")}: ${fmt(safeTotal)} de lixo seguro (temp, caches de GPU/NVIDIA, WER, navegadores).`);
const heavy = rows.filter((r) => ["cache", "VM/WSL", "Docker", "app instalado", "seus arquivos", "dev"].includes(advise(r.path)[0])).slice(0, 4);
heavy.forEach((r, i) => console.log(`  ${i + 2}. ${r.root}\\${r.name} (${fmt(r.size)}): ${advise(r.path)[1]}`));
console.log(dim("  Nada alem do 'LIXO SEGURO' e apagado por este script. O resto e decisao sua.\n"));

if (CLEAN) {
  const ok = await new Promise((res) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    rl.question(yellow(`Apagar ${fmt(safeTotal)} de lixo seguro? Digite SIM: `), (a) => { rl.close(); res(a.trim() === "SIM"); });
  });
  if (!ok) { console.log("cancelado, nada apagado."); process.exit(0); }
  let freed = 0, fail = 0;
  for (const x of plan) {
    for (const e of x.el) {
      let s = 0; try { s = e.dir ? sizeOf(e.p) : fs.lstatSync(e.p).size; } catch { /* */ }
      try { fs.rmSync(e.p, { recursive: true, force: true }); freed += s; } catch { fail++; }   // em uso/sem permissao: ignora
    }
  }
  console.log(green(`liberado ~${fmt(freed)}`) + (fail ? dim(`  (${fail} itens em uso/sem permissao foram pulados)`) : ""));
}
