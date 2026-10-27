import { defineConfig } from "vite";
export default defineConfig({
  build: {
    target: "es2022",
    rollupOptions: {
      output: {
        entryFileNames: "assets/editor.js",
        assetFileNames: "assets/editor.[ext]",
      },
    },
  },
});
