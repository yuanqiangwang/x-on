import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";

// Tauri expects a fixed dev server port.
export default defineConfig({
  // Tailwind v4 走 Vite 插件，不需要 postcss.config。
  plugins: [tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // 与 tauri.conf.json 的 bundle.windows.minimumWebview2Version = 120.0.0.0 对齐。
    // Tailwind v4 的产物用 @property / color-mix() / oklch（需要 Chrome 111+），
    // 原来的 chrome105 只作用于 JS 转译、不影响 CSS，但两边对齐省得以后踩坑。
    target: "chrome120",
    minify: "esbuild",
    sourcemap: false,
  },
});
