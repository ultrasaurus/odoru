# odoru

The name means dance, leap or take a journey in Japanese depending on context and how it is written. All of those words feel applicable to this experiment in the transformation of text and voice.


## Setup

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


## App server

Always run from the workspace root, with the frontend built at `app/frontend/dist`:

```bash
cargo run -p app
```

for now, you need to specify which backend to build
```bash
ODORU_BACKEND=f5 cargo run -p app
```

## Known Issues

In general, if anything works, consider it a happy surprise.

### CLI
- Segfault on exit when `--audio` is used. This is a PyO3/tokio shutdown
  ordering issue. All output is written successfully before the crash occurs.
