import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 2 + React 18 + Vite：配置对齐 cc-switch 的风格（base ./、outDir ../dist、@ alias）
export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
});
