import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "/static/gpt2api/",
  build: {
    outDir: "../static/gpt2api",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/api/gpt2api": "http://127.0.0.1:39180",
    },
  },
});
