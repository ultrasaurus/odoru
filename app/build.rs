use std::path::Path;
use std::process::Command;

// Runs on every `cargo build`/`cargo run -p app` (no rerun-if-changed
// directives — directory-level mtimes don't reflect file edits inside, so
// the only reliable option is to always run this). `npm run build`'s own
// `prebuild` hook regenerates the wasm splitter too, so this single step
// keeps both build.rs and the frontend in sync with the server binary.
fn main() {
    let frontend_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend");

    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&frontend_dir)
        .status()
        .expect("failed to run `npm run build` in app/frontend — is npm installed?");

    if !status.success() {
        panic!("`npm run build` failed in app/frontend");
    }
}
