import { defineConfig } from 'vite'
import { resolve } from 'path'

export default defineConfig({
  base: './',
  server: {
    proxy: {
          '/api': 'http://localhost:3000',
          '/ws': { target: 'ws://localhost:3000', ws: true },
    },
  },
  build: {
    outDir: 'dist',
    modulePreload: { polyfill: false },
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        'reader-export': resolve(__dirname, 'reader-export.html'),
      },
    },
  },
})