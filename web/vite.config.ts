import { defineConfig } from "vite";

// `base: "./"` keeps asset paths relative, so the build works from a
// GitHub Pages project path without knowing its name.
export default defineConfig({
  base: "./",
  server: { port: 5173, strictPort: true },
});
