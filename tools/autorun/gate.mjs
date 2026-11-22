// Uso: node tools/autorun/gate.mjs <ts|rust|both> [--baseline]
// Saída condensada: só erros NOVOS (vs baseline), sem warnings, sem log bruto.
// Também grava tools/autorun/gate-errors.txt. Exit 0 = sem erros novos.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const auto = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(auto, "../..");
const app = path.join(root, "apps", "prismview-studio");
const argv = process.argv.slice(2);
const gate = argv.find((a) => ["ts", "rust", "both"].includes(a)) ?? "ts";
const baselineMode = argv.includes("--baseline");
const MAX = 30;

const sh = (cmd, cwd) => {
  const r = spawnSync(cmd, { shell: true, cwd, encoding: "utf8", maxBuffer: 1 << 28 });
  const text = `${r.stdout ?? ""}\n${r.stderr ?? ""}`.replace(/\x1b\[[0-9;]*m/g, "");
  return { code: r.status ?? 1, lines: text.split(/\r?\n/) };
};

const normalize = (l) =>
  l.trim().replace(/:\d+:\d+(?=:)/, "").replace(/\(\d+,\d+\)/, "").slice(0, 300);

// null = toolchain ausente (não validado); senão [{sig, raw}]
function getErrors(kind) {
  let r, pat;
  if (kind === "ts") {
    r = sh("npm run build", app);
    pat = /error TS\d+|\[vite|error during build|Could not resolve|RollupError|^error/;
    if (r.code === 0) {
      r = sh("npm test", app);
      pat = /^\s*not ok|^✖|\bfailing\b|^# fail [1-9]|Error:/;
    }
  } else {
    if (sh("cargo --version", root).code !== 0) return null;
    r = sh("cargo check --workspace --message-format=short --color never", root);
    pat = /^\S.*: error|^error(\[|:)/;
  }
  if (r.code === 0) return [];
  let hits = r.lines.filter((l) => pat.test(l) && !/could not compile|aborting due to/.test(l));
  if (hits.length === 0) {
    hits = r.lines.filter((l) => l.trim()).slice(-15).map((l) => `UNPARSED: ${l}`);
  }
  return hits.map((h) => ({ sig: normalize(h), raw: h.trim() }));
}

const kinds = gate === "both" ? ["ts", "rust"] : [gate];
const out = [];
let totalNew = 0;

for (const k of kinds) {
  const baseFile = path.join(auto, `baseline-${k}.txt`);
  const items = getErrors(k);
  if (items === null) {
    out.push(`[${k}] NAO VALIDADO: toolchain ausente (cargo nao encontrado)`);
    continue;
  }
  if (baselineMode) {
    fs.writeFileSync(baseFile, [...new Set(items.map((i) => i.sig))].sort().join("\n"));
    console.log(`[${k}] baseline: ${items.length} erros pre-existentes registrados`);
    continue;
  }
  const known = new Set(fs.existsSync(baseFile) ? fs.readFileSync(baseFile, "utf8").split(/\r?\n/) : []);
  const seen = new Set();
  const fresh = [];
  for (const it of items) {
    if (known.has(it.sig) || seen.has(it.sig)) continue;
    seen.add(it.sig);
    fresh.push(it);
  }
  const ignored = items.length - fresh.length;
  totalNew += fresh.length;
  if (fresh.length === 0) {
    out.push(`[${k}] OK (${ignored} erros pre-existentes ignorados)`);
    continue;
  }
  out.push(`[${k}] ${fresh.length} erros NOVOS (${ignored} pre-existentes ignorados):`);
  fresh.sort((a, b) => a.raw.localeCompare(b.raw)).slice(0, MAX).forEach((i) => out.push("  " + i.raw));
  if (fresh.length > MAX) out.push(`  ... +${fresh.length - MAX} omitidos (corrija os primeiros e rode o gate de novo)`);
}

if (baselineMode) process.exit(0);
const text = [`NEW_ERRORS=${totalNew}`, ...out].join("\n");
fs.writeFileSync(path.join(auto, "gate-errors.txt"), text);
console.log(text);
process.exit(totalNew > 0 ? 1 : 0);
