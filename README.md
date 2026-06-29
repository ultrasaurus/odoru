# odoru

The name means dance, leap or take a journey in Japanese depending on context and how it is written. All of those words feel applicable to this experiment in the transformation of text and voice. 踊って!

## What it is (& will be)
A hypertext audio reading (and authoring) app. Fetches web articles, synthesizes
them to speech, plays back with synchronized transcript highlighting. 

Features currently supported:
* Create/Edit document from URL or type/paste into new doc - [editing.md](editing.md)
* Listen to a documet, including seeking anywhere, even while synth is in progress
* Add highlights "annotations" to a doc, and copy to a new doc
* Publish a collection of documents and export to SPA - [export.md](export.md)


## Dev Setup

The app is written in 
* Front end: Typescript
* Backend: Rust and Python (via Py03) 


### Python environment

Create and activate a virtual environment, then install dependencies:

```bash
python3.12 -m venv .venv
source .venv/bin/activate
pip install "misaki[en]" click trafilatura f5-tts-mlx soundfile
```

### spaCy language model

The Kokoro backend uses [misaki](https://github.com/hexgrad/misaki) for
grapheme-to-phoneme (G2P) conversion. Misaki uses spaCy to tokenize and
part-of-speech tag English text before phonemizing it. The `en_core_web_sm`
model must be downloaded once — it won't be pulled in automatically by pip:

```bash
python -m spacy download en_core_web_sm
```

If you skip this step, the first call to `G2P()` will attempt to download it
automatically, which can cause confusing errors in some environments.

Always run from the workspace root, running the server automatically
builds and serves the front end: `app/frontend/dist`:

```bash
cargo run -p app
```

For now, you need to specify which backend to build. Typically you would build for `f5` and `kokoro`:
```bash
ODORU_BACKEND=both cargo run -p app
```

### Frontend

in `app/frontend` see `.nvmrc` for version of node
```
cd app/frontend
nvm use
npx tsc
npx vite build
```

Run `tsc` separately before `vite build` — Vite bundles the compiled `.js` files in `src/`, not the `.ts` sources directly. If `tsc` fails, Vite silently uses stale `.js` files and produces a build that looks successful but doesn't reflect your changes.


## CLI

Install the `dl` binary:

```bash
cargo install --path cli
```

### Usage

```
dl [OPTIONS] <INPUT>
```

`<INPUT>` is a URL, a local `.txt` file, or a local `.html` file.

```bash
# Fetch a URL and print as markdown
dl https://ultrasaurus.com/2015/10/software-isnt-real/

# Read a local text file
dl abstract.txt

# Print as plain text
dl --format text https://ultrasaurus.com/2015/10/software-isnt-real/

# Synthesize audio with Kokoro (fast, needs $KOKORO_MODEL_DIR)
dl --audio --backend kokoro abstract.txt

# Synthesize audio with F5-TTS (slow, ~7 min for a short article)
dl --audio --backend f5 abstract.txt

# Write audio to a specific path or directory
dl --audio --backend kokoro -o /tmp/out.wav abstract.txt
dl --audio --backend kokoro -o /tmp/ abstract.txt
```

Audio is written to `out/<name>.wav` by default (directory created if needed).
Override with `-o <path>` — if the path is an existing directory, the filename
is derived from the input as usual.

see `dl --help` for more options

### Backends

| Backend | Speed | Notes |
|---------|-------|-------|
| `kokoro` | Fast | ONNX inference. Requires `$KOKORO_MODEL_DIR`. |
| `f5` | Slow (~0.17x realtime on M1) | MLX inference. Requires `voices/sarah/`. |
| `mock` | Instant | Sine wave, no model weights needed. For testing. |

### Pronunciation overrides

Edit `tts_overrides.txt` in the workspace root to customize how words are
spoken by the F5 backend. Changes take effect on the next run — no recompile
needed. See the file for format and examples.

## Known Issues

In general, if anything works, consider it a happy surprise.

### CLI
- Segfault on exit when `--audio` is used. This is a PyO3/tokio shutdown
  ordering issue. All output is written successfully before the crash occurs.

## Code coverage (Rust)

Install `cargo-llvm-cov` (one-time setup):

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
```

Run coverage for the whole repo:

```bash
cargo llvm-cov
```

Run coverage for just the `normalize` function's unit tests (in `util/src/normalizer.rs`):

```bash
cargo llvm-cov --package util --lib -- normalizer::
```

Add `--html` to either command to generate a browsable report at
`target/llvm-cov/html/index.html`.
