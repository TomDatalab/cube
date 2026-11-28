import path from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig(({ mode }) => ({
  base: './',
  build: {
    outDir: 'build',
    target: 'es2020',
  },
  server: {
    port: 3080,
    proxy: {
      '^/playground/*': 'http://localhost:4000',
      '^/cubejs-api/*': 'http://localhost:4000',
    },
  },
  plugins: [
    react(),
    // shadcn/ui: Tailwind utilities only, without its global reset (see
    // src/shadcn.css), so the antd screens keep their look.
    tailwindcss(),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
  css: {
    preprocessorOptions: {
      less: {
        javascriptEnabled: true,
        additionalData: '@root-entry-name: default;',
      },
    },
  },
  define: {
    'process.env.SC_DISABLE_SPEEDY': JSON.stringify('false'),
    ...(mode === 'development' ? { global: {} } : {}),
  },
}));
