import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `npm run dev` proxies the API to the Rust server (OORT_ADDR, default 127.0.0.1:3000).
const backend = process.env.OORT_BACKEND ?? "http://127.0.0.1:3000";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": { target: backend, changeOrigin: true },
    },
  },
});
