# odoru

The name means dance, leap or take a journey in Japanese depending on context and how it is written. All of those words feel applicable to this experiment in the transformation of text and voice.


```
python3.12 -m venv .venv
source .venv/bin/activate
pip install "misaki[en]" click trafilatura 
```

always run `cargo run -p app` from the workspace root, with the frontend built at `app/frontend/dist`


## Known Issues

In general, if anything work, consider it a happy surprise.

### CLI
- Segfault on exit when `--audio` is used. This is a PyO3/tokio shutdown 
  ordering issue. All output is written successfully before the crash occurs.
