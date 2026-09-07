import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri serves the built assets from `dist`, and expects a fixed dev port so
// the Rust side can point a webview at it.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { port: 7188, strictPort: true },
  build: { target: "safari15", sourcemap: true },
  test: { environment: "jsdom" },
});
