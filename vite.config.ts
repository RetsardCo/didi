import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  build: {
    target: ["es2022", "chrome105"],
    sourcemap: false,
    outDir: "dist",
    emptyOutDir: true,
  },
});
