// Gera src/engine/motionTokens.gen.ts a partir de ../../docs/motion.tokens.json.
// `--check`: não escreve; falha se o .gen estiver defasado ou se a paleta do CSS/Tailwind divergir.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const srcPath = resolve(root, "../../docs/motion.tokens.json");
const outPath = resolve(root, "src/engine/motionTokens.gen.ts");
const check = process.argv.includes("--check");

const { $comment, ...tokens } = JSON.parse(readFileSync(srcPath, "utf8"));
const out =
  "// GERADO por scripts/gen-motion.mjs a partir de docs/motion.tokens.json. Não editar.\n" +
  `export const TOKENS = ${JSON.stringify(tokens, null, 2)} as const;\n`;

const errors = [];
if (check) {
  let cur = "";
  try { cur = readFileSync(outPath, "utf8"); } catch {}
  if (cur !== out) errors.push("motionTokens.gen.ts defasado: rode `npm run gen:motion`.");
  const css = readFileSync(resolve(root, "src/styles/prism.css"), "utf8").toLowerCase();
  const tw = readFileSync(resolve(root, "tailwind.config.js"), "utf8").toLowerCase();
  const c = tokens.color;
  const cssVars = { pureBlack: "--prism-pure-black", carbon: "--prism-carbon", mutedZinc: "--prism-muted-zinc", trackZinc: "--prism-track-zinc" };
  for (const [k, v] of Object.entries(cssVars)) {
    if (!css.includes(`${v}: ${c[k].toLowerCase()};`)) errors.push(`prism.css: ${v} != ${c[k]}`);
  }
  for (const k of ["pureBlack", "carbon", "surface", "mutedZinc", "trackZinc"]) {
    const name = k === "carbon" ? "carbonBlack" : k === "surface" ? "surfaceElevated" : k;
    if (!tw.includes(`${name.toLowerCase()}: "${c[k].toLowerCase()}"`)) errors.push(`tailwind.config.js: ${name} != ${c[k]}`);
  }
  if (errors.length) { console.error(errors.join("\n")); process.exit(1); }
  console.log("motion tokens ok");
} else {
  writeFileSync(outPath, out);
  console.log("gerado", outPath);
}
