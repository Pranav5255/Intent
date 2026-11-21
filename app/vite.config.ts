import { defineConfig, type PluginOption } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri expects a fixed port and must not have Vite clear its terminal output.
const host = process.env.TAURI_DEV_HOST;

// RETICLE=1 (dev/lima/scenarios-app.sh) adds the Reticle SDK to the dev server so the
// scenario run can drive the real app. The plugin is installed only for that run and is
// never part of a release build.
async function reticlePlugin(): Promise<PluginOption> {
  if (process.env.RETICLE !== '1') return null;
  const { reticle } = await import('@reticlehq/vite-plugin' as string);
  return reticle({ desktop: true, captureNetworkBodies: true });
}

export default defineConfig(async () => ({
  plugins: [react(), await reticlePlugin()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || '127.0.0.1',
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'es2022',
    sourcemap: false,
  },
}));
