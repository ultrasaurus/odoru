use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use futures::stream::Stream;
use pyo3::prelude::*;
use pyo3::types::PyList;
use tokio::sync::oneshot;

use crate::error::G2pError;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A phoneme result for one sentence.
#[derive(Debug, Clone)]
pub struct PhonemeChunk {
    /// 0-based position of this sentence in the original text.
    pub index: usize,
    /// The original sentence as it was split from the input.
    pub sentence: String,
    /// IPA-like phoneme string produced by Misaki, e.g. `"hɛloʊ wɜːld"`.
    pub phonemes: String,
}

// ---------------------------------------------------------------------------
// Internal channel message
// ---------------------------------------------------------------------------

/// One unit of work sent to the Python thread.
struct G2pRequest {
    index: usize,
    sentence: String,
    /// The Python thread sends its result back through here.
    reply: oneshot::Sender<Result<String, G2pError>>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// One-time initialised handle to the Misaki G2P engine.
///
/// Construct with [`G2pEngine::new`], then call [`G2pEngine::phonemize`] as
/// many times as you like. The underlying Python thread lives for the
/// lifetime of the engine.
///
/// Wrap in `Arc` if you need to share across tasks:
/// ```rust
/// let engine = Arc::new(G2pEngine::new(None)?);
/// ```
pub struct G2pEngine {
    /// Sender to the dedicated Python worker thread.
    tx: mpsc::SyncSender<G2pRequest>,
}

impl G2pEngine {
    /// Initialise the engine.
    ///
    /// `venv_path` — path to a Python venv that has `misaki-en` installed.
    /// If `None`, falls back to the `MISAKI_VENV` environment variable.
    ///
    /// This call blocks briefly (~100–300 ms on first run) while Python
    /// starts and Misaki is imported. All subsequent calls are non-blocking.
    pub fn new(venv_path: Option<&Path>) -> Result<Self, G2pError> {
        let venv = resolve_venv(venv_path)?;
        let site_packages = find_site_packages(&venv)?;

        // ----------------------------------------------------------------
        // Step 1 — tell Python where to find packages BEFORE the
        // interpreter starts.  PYTHONPATH is read at interpreter init time.
        // ----------------------------------------------------------------
        // Safety: we set this once, before pyo3::prepare_freethreaded_python.
        // Calling code is expected to call G2pEngine::new before any async
        // work spawns threads that might also read env vars.
        std::env::set_var("PYTHONPATH", &site_packages);

        // ----------------------------------------------------------------
        // Step 2 — start the Python interpreter (idempotent after first call).
        // ----------------------------------------------------------------
        pyo3::prepare_freethreaded_python();

        // ----------------------------------------------------------------
        // Step 3 — belt-and-suspenders: also insert site-packages at the
        // front of sys.path in case PYTHONPATH was already set to something
        // else before we got here.
        // ----------------------------------------------------------------
        Python::with_gil(|py| -> Result<(), G2pError> {
            prepend_site_packages(py, &site_packages)?;
            // Fail fast: if misaki isn't importable we want a clear error now,
            // not a confusing panic inside the worker thread later.
             py.import_bound("misaki.en").map_err(|e| {
                G2pError::PythonInit(format!(
                    "Could not import misaki.en from {}: {}",
                    site_packages.display(),
                    e
                ))
            })?;
            Ok(())
        })?;

        // ----------------------------------------------------------------
        // Step 4 — spawn the dedicated Python worker thread.
        //
        // Why a plain std::thread and not a Tokio task?
        // PyO3's Python::with_gil blocks the OS thread until the GIL is
        // available.  If we ran this on a Tokio worker thread we'd stall the
        // entire executor.  A dedicated std::thread sidesteps that entirely.
        // ----------------------------------------------------------------
        let (tx, rx) = mpsc::sync_channel::<G2pRequest>(64);

        thread::Builder::new()
            .name("misaki-g2p-worker".into())
            .spawn(move || python_worker(rx))
            .map_err(|e| G2pError::PythonInit(format!("Failed to spawn worker thread: {e}")))?;

        Ok(Self { tx })
    }

    /// Convert `text` to phonemes, yielding one [`PhonemeChunk`] per sentence.
    ///
    /// The stream is ordered — chunks arrive in the same order as sentences
    /// appear in the input, even though work is dispatched asynchronously.
    /// If Misaki fails on one sentence an `Err` is yielded for that sentence
    /// and the stream continues with the next.
    pub fn phonemize(
        &self,
        text: impl Into<String>,
    ) -> impl Stream<Item = Result<PhonemeChunk, G2pError>> {
        let text = text.into();

        // Sentence splitting lives in splitter.rs — wired up in the next step.
        // For now, a line-based stub so the bridge is testable end-to-end.
        let sentences: Vec<String> = crate::splitter::split(&text);

        // Build one future per sentence and collect into a FuturesOrdered so
        // results always emerge in sentence order regardless of processing time.
        let futures: futures::stream::FuturesOrdered<_> = sentences
            .into_iter()
            .enumerate()
            .map(|(index, sentence)| {
                let (reply_tx, reply_rx) = oneshot::channel();

                let send_result = self.tx.send(G2pRequest {
                    index,
                    sentence: sentence.clone(),
                    reply: reply_tx,
                });

                // Return a future that resolves once the Python thread replies.
                async move {
                    // If the channel was full or dropped, propagate immediately.
                    send_result.map_err(|_| G2pError::StreamClosed)?;

                    // Await the oneshot reply from the Python thread.
                    let phonemes = reply_rx
                        .await
                        .map_err(|_| G2pError::StreamClosed)??; // double ? unpacks oneshot then G2pError

                    Ok(PhonemeChunk {
                        index,
                        sentence,
                        phonemes,
                    })
                }
            })
            .collect();

        futures
    }
}

// ---------------------------------------------------------------------------
// Python worker — runs on its own std::thread for the lifetime of the engine.
// ---------------------------------------------------------------------------

fn python_worker(rx: mpsc::Receiver<G2pRequest>) {
    // Instantiate the G2P object once and reuse it for every sentence.
    // Constructing it is expensive (~50 ms); per-call cost is cheap.
    let g2p = match make_g2p_object() {
        Ok(obj) => obj,
        Err(e) => {
            // Drain the channel so callers don't hang forever.
            for req in rx {
                let _ = req.reply.send(Err(G2pError::PythonInit(e.clone())));
            }
            return;
        }
    };

    for req in rx {
        let result = call_g2p(&g2p, &req);
        // Receiver may have been dropped (caller gave up) — that's fine.
        let _ = req.reply.send(result);
    }
}

/// Construct `misaki.en.G2P()` and return it as an owned `PyObject`.
fn make_g2p_object() -> Result<PyObject, String> {
    Python::with_gil(|py| {
        let module = py
            .import_bound("misaki.en")
            .map_err(|e| format!("import misaki.en: {e}"))?;
        let class = module
            .getattr("G2P")
            .map_err(|e| format!("G2P class not found: {e}"))?;
        let instance = class
            .call0()
            .map_err(|e| format!("G2P() constructor failed: {e}"))?;
        Ok(instance.into())
    })
}

/// Call `g2p(sentence)` and extract the phoneme string from the result tuple.
///
/// Misaki returns `(phonemes: str, tokens: list)`.  We only need index 0.
fn call_g2p(g2p: &PyObject, req: &G2pRequest) -> Result<String, G2pError> {
    Python::with_gil(|py| {
        // Call the G2P object: result = g2p(sentence)
        let result = g2p
            .call1(py, (&req.sentence,))
            .map_err(|e| G2pError::G2pFailed {
                index: req.index,
                sentence: req.sentence.clone(),
                cause: e.to_string(),
            })?;

        // result is Py<PyAny> — bind it to the GIL to get a Bound reference,
        // then index into the (phonemes, tokens) tuple Misaki returns.
        let phonemes: String = result
            .bind(py)
            .get_item(0)
            .and_then(|item| item.extract::<String>())
            .map_err(|e| G2pError::G2pFailed {
                index: req.index,
                sentence: req.sentence.clone(),
                cause: format!("could not extract phoneme string from result: {e}"),
            })?;

        Ok(phonemes)
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the venv path from the explicit argument or `$MISAKI_VENV`.
fn resolve_venv(explicit: Option<&Path>) -> Result<PathBuf, G2pError> {
    if let Some(p) = explicit {
        if !p.is_dir() {
            return Err(G2pError::PythonInit(format!(
                "Provided venv path does not exist: {}",
                p.display()
            )));
        }
        return Ok(p.to_path_buf());
    }

    let from_env = std::env::var("MISAKI_VENV").map_err(|_| {
        G2pError::PythonInit(
            "No venv path given and $MISAKI_VENV is not set. \
             Run setup.sh first, then export MISAKI_VENV=<path>."
                .into(),
        )
    })?;

    let p = PathBuf::from(from_env);
    if !p.is_dir() {
        return Err(G2pError::PythonInit(format!(
            "$MISAKI_VENV points to a non-existent directory: {}",
            p.display()
        )));
    }
    Ok(p)
}

/// Walk `<venv>/lib/` looking for `python3.x/site-packages`.
/// On M1 Macs the layout is always `lib/python3.xx/site-packages`.
fn find_site_packages(venv: &Path) -> Result<PathBuf, G2pError> {
    let lib = venv.join("lib");

    let entries = std::fs::read_dir(&lib).map_err(|e| {
        G2pError::PythonInit(format!(
            "Cannot read venv lib dir {}: {e}",
            lib.display()
        ))
    })?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("python3.") {
            let sp = entry.path().join("site-packages");
            if sp.is_dir() {
                return Ok(sp);
            }
        }
    }

    Err(G2pError::PythonInit(format!(
        "No python3.x/site-packages directory found inside {}. \
         Is this a valid venv with misaki installed?",
        lib.display()
    )))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialise every test that touches env vars.
    // std::env::set_var / remove_var are not thread-safe across parallel tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── resolve_venv ──────────────────────────────────────────────────────

    #[test]
    fn resolve_venv_explicit_valid_dir_returns_path() {
        let dir = TempDir::new().unwrap();
        let result = resolve_venv(Some(dir.path()));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), dir.path());
    }

    #[test]
    fn resolve_venv_explicit_nonexistent_path_returns_error() {
        let result = resolve_venv(Some(Path::new("/this/does/not/exist")));
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "unexpected error: {err}");
    }

    #[test]
    fn resolve_venv_no_arg_no_env_var_returns_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MISAKI_VENV");

        let result = resolve_venv(None);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("MISAKI_VENV"),
            "error should mention MISAKI_VENV, got: {err}"
        );
    }

    #[test]
    fn resolve_venv_env_var_valid_dir_returns_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        std::env::set_var("MISAKI_VENV", dir.path());

        let result = resolve_venv(None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), dir.path());

        std::env::remove_var("MISAKI_VENV");
    }

    #[test]
    fn resolve_venv_env_var_nonexistent_dir_returns_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("MISAKI_VENV", "/no/such/dir");

        let result = resolve_venv(None);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("non-existent"),
            "unexpected error: {err}"
        );

        std::env::remove_var("MISAKI_VENV");
    }

    // Explicit path takes priority over env var even when both are set.
    #[test]
    fn resolve_venv_explicit_overrides_env_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let env_dir = TempDir::new().unwrap();
        let explicit_dir = TempDir::new().unwrap();
        std::env::set_var("MISAKI_VENV", env_dir.path());

        let result = resolve_venv(Some(explicit_dir.path())).unwrap();
        assert_eq!(result, explicit_dir.path());

        std::env::remove_var("MISAKI_VENV");
    }

    // ── find_site_packages ────────────────────────────────────────────────

    /// Build a minimal fake venv on disk: <root>/lib/python3.xx/site-packages/
    fn make_fake_venv(python_version: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        let sp = dir
            .path()
            .join("lib")
            .join(format!("python{python_version}"))
            .join("site-packages");
        fs::create_dir_all(&sp).unwrap();
        dir
    }

    #[test]
    fn find_site_packages_standard_layout_returns_path() {
        let venv = make_fake_venv("3.11");
        let result = find_site_packages(venv.path());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("site-packages"));
    }

    #[test]
    fn find_site_packages_works_for_any_minor_version() {
        for minor in ["3.10", "3.11", "3.12", "3.13"] {
            let venv = make_fake_venv(minor);
            let result = find_site_packages(venv.path());
            assert!(
                result.is_ok(),
                "expected Ok for python{minor}, got: {:?}",
                result
            );
        }
    }

    #[test]
    fn find_site_packages_missing_lib_dir_returns_error() {
        let dir = TempDir::new().unwrap(); // no lib/ created
        let err = find_site_packages(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("Cannot read venv lib dir"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_site_packages_no_python_subdir_returns_error() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("lib")).unwrap(); // lib/ exists but empty
        let err = find_site_packages(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("No python3.x/site-packages"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_site_packages_python_dir_without_site_packages_returns_error() {
        let dir = TempDir::new().unwrap();
        // lib/python3.11/ exists but site-packages/ does not
        fs::create_dir_all(dir.path().join("lib").join("python3.11")).unwrap();
        let err = find_site_packages(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("No python3.x/site-packages"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_site_packages_ignores_non_python_subdirs() {
        let dir = TempDir::new().unwrap();
        // Directories that should NOT match the python3.* pattern
        for name in &["bin", "include", "lib64", "python2.7"] {
            fs::create_dir_all(
                dir.path().join("lib").join(name).join("site-packages"),
            )
            .unwrap();
        }
        let err = find_site_packages(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("No python3.x/site-packages"),
            "should not match non-python3 dirs, got: {err}"
        );
    }
}

/// Insert `site_packages` at position 0 of `sys.path` if not already present.
fn prepend_site_packages(py: Python<'_>, site_packages: &Path) -> Result<(), G2pError> {
    let sp_str = site_packages.to_string_lossy().to_string();

    let sys = py
        .import_bound("sys")
        .map_err(|e| G2pError::PythonInit(format!("import sys: {e}")))?;

    let path_any = sys
        .getattr("path")
        .map_err(|e| G2pError::PythonInit(format!("sys.path not found: {e}")))?;

    let path = path_any
        .downcast::<PyList>()
        .map_err(|e| G2pError::PythonInit(format!("sys.path not a list: {e}")))?;

    let already_present = path.iter().any(|item| {
        item.extract::<String>()
            .map(|s| s == sp_str)
            .unwrap_or(false)
    });

    if !already_present {
        path.insert(0, &sp_str)
            .map_err(|e| G2pError::PythonInit(format!("sys.path.insert failed: {e}")))?;
    }

    Ok(())
}
