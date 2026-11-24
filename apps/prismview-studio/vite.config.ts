import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { createReadStream, existsSync } from "node:fs";
import { resolve } from "node:path";
import type { Plugin } from "vite";

/**
 * Só no `npm run dev` fora do Tauri: serve os TTF embutidos (`src-tauri/fonts`) em `/__dev-fonts/`
 * para o texto alongado vetorial funcionar no navegador (no app, o comando `read_font_bytes` faz
 * isso). Não entra no build de produção.
 */
function devFonts(): Plugin {
  const dir = resolve(__dirname, "src-tauri/fonts");
  return {
    name: "prism-dev-fonts",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__dev-fonts/", (req, res, next) => {
        const name = decodeURIComponent((req.url ?? "").split("?")[0]).replace(/^\/+/, "");
        // Só o nome de um arquivo .ttf da pasta (sem subpastas nem "..").
        if (!/^[A-Za-z0-9_-]+\.ttf$/.test(name)) return next();
        const file = resolve(dir, name);
        if (!existsSync(file)) return next();
        res.setHeader("Content-Type", "font/ttf");
        createReadStream(file).pipe(res);
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), devFonts()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: ["es2021", "chrome105", "safari13"],
    minify: "esbuild",
    sourcemap: false,
    chunkSizeWarningLimit: 850,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        desktop: resolve(__dirname, "desktop.html"),
      },
      output: {
        manualChunks(id) {
          // Studio-only Sala 3D gizmos: never downloaded/parsed by Desktop wallpaper windows
          if (id.includes("node_modules/three/examples/jsm/controls")) {
            return "three-editor";
          }
          if (id.includes("node_modules/three")) {
            return "three-core";
          }
          return undefined;
        },
      },
    },
  },
});
