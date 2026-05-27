# ko-odoru

Rust library that converts text to phonemes using [Misaki G2P](https://github.com/hexgrad/misaki), exposed as an async Tokio stream. Each sentence yields a `PhonemeChunk` as it completes.

The name means "dancing like this" in Japanese — a nod to the [Kokoro](https://pypi.org/project/kokoro/) TTS ecosystem this is designed to feed into.

## Setup

Requires Python 3.10–3.12 (arm64 on M1 Mac) and a venv with `misaki[en]` installed.

```bash
./setup.sh
cp .env.example .env   # fill in the paths setup.sh printed
```

## Build & run

```bash
source .env
cargo build
echo "Hello world." | cargo run --example basic
```

Or use the wrapper script which sources `.env` for you:

```bash
echo "Hello world." | ./run.sh
```

## Testing

Unit tests have no dependencies and run anywhere:

```bash
cargo test
```

Integration tests require a real venv with Misaki installed, so they are
marked `#[ignore]` by default to keep CI fast on machines without Python set
up. Run them locally after `source .env`:

```bash
cargo test -- --include-ignored
```

Or integration tests only:

```bash
cargo test --test integration -- --ignored
```
