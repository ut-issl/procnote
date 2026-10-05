import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite-plus";

export default defineConfig({
  fmt: {},
  lint: {},
  plugins: [sveltekit({ adapter: adapter({ fallback: "index.html" }) })],
  server: { port: 5180, strictPort: true },
});
