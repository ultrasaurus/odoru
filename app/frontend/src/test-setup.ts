import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

// jsdom's fetch doesn't set a Content-Type header for wasm assets served by
// Vite's test server, which trips wasm-bindgen's instantiateStreaming MIME
// check. Removing it makes the generated init() glue fall back to
// arrayBuffer() + instantiate(), which doesn't care about Content-Type.
delete (WebAssembly as any).instantiateStreaming

// jsdom's fetch also can't fetch real bytes from Vite's test server for a
// binary asset like this (returns an empty buffer), which the arrayBuffer()
// fallback above needs. Read .wasm requests straight off disk instead —
// `import.meta.url`'s relative path component matches the source layout.
const realFetch = globalThis.fetch
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const url = input instanceof Request ? input.url : input.toString()
  if (url.endsWith('.wasm')) {
    const relPath = new URL(url).pathname.replace(/^\/src\//, '') // wasm/splitter_wasm_bg.wasm
    const bytes = readFileSync(fileURLToPath(new URL(relPath, import.meta.url)))
    return new Response(bytes, { headers: { 'Content-Type': 'application/wasm' } })
  }
  return realFetch(input, init)
}) as typeof fetch
