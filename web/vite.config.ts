import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      // React Router v8 includes the browser bindings formerly provided by
      // react-router-dom. Keep existing imports while avoiding its vulnerable
      // React Router v7 dependency chain.
      "react-router-dom": "react-router",
    },
  },
  server: {
    host: "127.0.0.1",
    port: 3001,
    strictPort: false, // Allow fallback to next available port
    proxy: {
      "/api": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
      "/auth": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
      "/ws": {
        target: "http://localhost:8081",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
