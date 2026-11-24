// Compila `crates/prism-vstem-wasm` (alongamento vetorial por haste) para WebAssembly e copia o
// resultado para `public/wasm/prism_vstem.wasm`, de onde o Estúdio o carrega sob demanda.
//
// Requisitos (uma vez): `rustup target add wasm32-unknown-unknown`.
// Uso: `npm run build:vstem` (o .wasm fica versionado: só refaça ao mudar `vstem.rs` ou o crate).
// `--check` só confere se o arquivo existe e exporta a ABI esperada (usado pelos testes).
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, "..");
const crate = resolve(app, "../../crates/prism-vstem-wasm");
const built = resolve(crate, "target/wasm32-unknown-unknown/release/prism_vstem_wasm.wasm");
const dest = resolve(app, "public/wasm/prism_vstem.wasm");
const EXPECTED_ABI = 1;
const REQUIRED_EXPORTS = [
  "memory",
  "vs_abi_version",
  "vs_alloc",
  "vs_free",
  "vs_font_add",
  "vs_font_remove",
  "vs_font_clear_caches",
  "vs_layout",
  "vs_out_ptr",
  "vs_out_len",
];

async function verify(path) {
  if (!existsSync(path)) throw new Error(`não existe: ${path} (rode \`npm run build:vstem\`)`);
  const { instance } = await WebAssembly.instantiate(readFileSync(path), {});
  const missing = REQUIRED_EXPORTS.filter((n) => !(n in instance.exports));
  if (missing.length) throw new Error(`exports ausentes: ${missing.join(", ")}`);
  const abi = instance.exports.vs_abi_version();
  if (abi !== EXPECTED_ABI) throw new Error(`ABI ${abi}, esperado ${EXPECTED_ABI}: rebuild necessário`);
  return statSync(path).size;
}

if (process.argv.includes("--check")) {
  const size = await verify(dest);
  console.log(`prism_vstem.wasm ok (${(size / 1024).toFixed(0)} KB, ABI ${EXPECTED_ABI})`);
  process.exit(0);
}

const r = spawnSync("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown"], {
  cwd: crate,
  stdio: "inherit",
});
if (r.status !== 0) {
  console.error("cargo build falhou (alvo instalado? `rustup target add wasm32-unknown-unknown`)");
  process.exit(r.status ?? 1);
}
mkdirSync(dirname(dest), { recursive: true });
copyFileSync(built, dest);
const size = await verify(dest);
console.log(`prism_vstem.wasm atualizado (${(size / 1024).toFixed(0)} KB, ABI ${EXPECTED_ABI}) → ${dest}`);
