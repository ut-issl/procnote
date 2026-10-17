import react from "@vitejs/plugin-react";
import { defineConfig } from "vite-plus";

export default defineConfig({
  fmt: {},
  lint: {},
  plugins: [react()],
  publicDir: "static",
  build: {
    // Tauri (frontendDist) and procnote-server (--static-dir) both serve build/.
    outDir: "build",
    emptyOutDir: true,
  },
  server: {
    port: 5180,
    strictPort: true,
    // Browser mode talks to procnote-server (`just dev-server`).
    proxy: { "/api": { target: "http://127.0.0.1:5181", ws: true } },
  },
});
