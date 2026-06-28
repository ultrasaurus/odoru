import { defineConfig } from 'vite'
import { resolve } from 'path'

// Separate build target for reader-export.html so it gets its own,
// fully self-contained JS bundle with no chunks shared with the main
// app. `dl spa` inlines this output into a single-file static export
// and needs every <script> to have zero `import`/`export` statements
// left to resolve — splitting this into its own build, rather than
// relying on Rollup not to share-chunk it with `main`, guarantees that.
export default defineConfig({
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: false,
    modulePreload: { polyfill: false },
    rollupOptions: {
      input: {
        'reader-export': resolve(__dirname, 'reader-export.html'),
      },
      output: {
        codeSplitting: false,
      },
    },
  },
})
