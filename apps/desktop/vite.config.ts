import { svelte } from '@sveltejs/vite-plugin-svelte';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { xtermImePatch } from './xterm-ime-patch';

const tauriConfig = JSON.parse(
  readFileSync(fileURLToPath(new URL('./src-tauri/tauri.conf.json', import.meta.url)), 'utf8'),
) as { version: string };

export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  plugins: [xtermImePatch(), svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(tauriConfig.version),
  },
  clearScreen: false,
  // The IME backport must see xterm's source instead of Vite's pre-bundled copy.
  optimizeDeps: {
    exclude: ['@xterm/xterm'],
  },
  server: {
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'es2021',
    outDir: '../../dist/desktop',
    emptyOutDir: true,
  },
});
