



```
python3.12 -m venv .venv
source .venv/bin/activate
pip install "misaki[en]" click trafilatura 
```

always run `cargo run -p app` from the `ko-odoru/` workspace root, with the frontend built at `app/frontend/dist`


## Known Issues
- Segfault on exit when `--audio` is used. This is a PyO3/tokio shutdown 
  ordering issue. All output is written successfully before the crash occurs.
