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

always run `cargo run -p app` from the workspace root, with the frontend built at `app/frontend/dist`

## Install CLI locally
```
cargo install --path cli
```

# F5 (slow — ~7 min for a short article)
```
dl --audio --backend f5 https://ultrasaurus.com/2015/10/software-isnt-real/
```
# Kokoro (needs KOKORO_MODEL_DIR set)
```
dl --audio --backend kokoro https://ultrasaurus.com/2015/10/software-isnt-real/.com
```

## Known Issues

In general, if anything works, consider it a happy surprise.

### CLI
- Segfault on exit when `--audio` is used. This is a PyO3/tokio shutdown 
  ordering issue. All output is written successfully before the crash occurs.
