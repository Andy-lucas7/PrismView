import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [react()],
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
