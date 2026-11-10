import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri 开发时固定 127.0.0.1:5281（5173 常被别的项目占着，而且 localhost 会先走 IPv6 连到别人那里）；
// src-tauri 的改动不触发前端热更新。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5281,
    strictPort: true,
    host: '127.0.0.1',
    watch: { ignored: ['**/src-tauri/**', '**/legacy/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'chrome105',
    sourcemap: false,
    outDir: 'dist',
  },
});
