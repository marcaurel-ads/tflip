import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue()],
  server: {
    proxy: {
      // changeOrigin, so the backend sees its own Host, not Vite's, and its
      // DNS-rebinding check lets the request through. Vite's own Host check
      // (server.allowedHosts) stands in for it during development. For the
      // SQL socket, rewriteWsOrigin does the same for the Origin check.
      "/api": {
        target: "http://127.0.0.1:6263",
        changeOrigin: true,
        ws: true,
        rewriteWsOrigin: true,
      },
    },
  },
});
