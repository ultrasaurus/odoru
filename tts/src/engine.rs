use std::sync::mpsc;
use std::thread;

use futures::stream::Stream;
use pyo3::prelude::*;
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
/// ```no_run
/// use std::sync::Arc;
/// use tts::G2pEngine;
/// let engine = Arc::new(G2pEngine::new()?);
/// # Ok::<(), tts::G2pError>(())
/// ```
#[derive(Debug)]
pub struct G2pEngine {
    /// Sender to the dedicated Python worker thread.
    tx: mpsc::SyncSender<G2pRequest>,
}

impl G2pEngine {
    /// Initialise the engine.
    ///
    /// Reads `$VIRTUAL_ENV` (set automatically by `source .venv/bin/activate`)
    /// to locate the venv's site-packages. If not set, system Python is used.
    ///
    /// This call blocks briefly (~100–300 ms on first run) while Python
    /// starts and Misaki is imported. All subsequent calls are non-blocking.
    pub fn new() -> Result<Self, G2pError> {
        // Add venv site-packages to sys.path and verify misaki.en is importable.
        // Fail fast with a clear error if not.
        Python::attach(|py| -> Result<(), G2pError> {
            setup_python(py)?;
            py.import("misaki.en").map_err(|e| {
                G2pError::PythonInit(format!(
                    "Could not import misaki.en — is the venv active and misaki[en] installed?\n  {e}"
                ))
            })?;
            py.import("misaki.espeak").map_err(|e| {
                G2pError::PythonInit(format!(
                    "Could not import misaki.espeak — is espeak-ng installed? (brew install espeak-ng)\n  {e}"
                ))
            })?;
            Ok(())
        })?;

        // ----------------------------------------------------------------
        // Step 3 — spawn the dedicated Python worker thread.
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
        let sentences: Vec<String> = crate::splitter::split(&text)
            .into_iter()
            .map(|s| s.text)
            .collect();

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

/// Construct `misaki.en.G2P(fallback=EspeakFallback(False))` and return it as an owned `Py<PyAny>`.
fn make_g2p_object() -> Result<Py<PyAny>, String> {
    Python::attach(|py| {
        // Misaki parses sys.argv at G2P() construction time.
        // Clear it so cargo test's flags don't cause a SystemExit.
        py.run(c"import sys; sys.argv = ['']", None, None)
            .map_err(|e| format!("Failed to reset sys.argv: {e}"))?;

        let fallback_module = py
            .import("misaki.espeak")
            .map_err(|e| format!("import misaki.espeak: {e}"))?;
        let fallback_class = fallback_module
            .getattr("EspeakFallback")
            .map_err(|e| format!("EspeakFallback class not found: {e}"))?;
        let fallback = fallback_class
            .call1((false,))
            .map_err(|e| format!("EspeakFallback(False) failed: {e}"))?;

        let module = py
            .import("misaki.en")
            .map_err(|e| format!("import misaki.en: {e}"))?;
        let class = module
            .getattr("G2P")
            .map_err(|e| format!("G2P class not found: {e}"))?;
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs.set_item("fallback", fallback)
            .map_err(|e| format!("kwargs.set_item: {e}"))?;
        let instance = class
            .call((), Some(&kwargs))
            .map_err(|e| format!("G2P() constructor failed: {e}"))?;
        Ok(instance.unbind())
    })
}

/// Call `g2p(sentence)` and extract the phoneme string from the result tuple.
///
/// Misaki returns `(phonemes: str, tokens: list)`.  We only need index 0.
fn call_g2p(g2p: &Py<PyAny>, req: &G2pRequest) -> Result<String, G2pError> {
    Python::attach(|py| {
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

/// Add the active venv's site-packages to `sys.path`.
///
/// Reads `$VIRTUAL_ENV` (set automatically by `source .venv/bin/activate`)
/// and derives the site-packages path from the running Python version.
/// If `$VIRTUAL_ENV` is not set, this is a no-op — system Python is used.
fn setup_python(py: Python) -> Result<(), G2pError> {
    let venv = match std::env::var("VIRTUAL_ENV") {
        Ok(v) => v,
        Err(_) => return Ok(()), // no venv active — use system Python
    };

    let version = py
        .eval(c"__import__('sys').version_info[:2]", None, None)
        .map_err(|e| G2pError::PythonInit(format!("Failed to get Python version: {e}")))?;

    let (major, minor): (u8, u8) = version
        .extract()
        .map_err(|e| G2pError::PythonInit(format!("Failed to extract Python version: {e}")))?;

    let site_packages = format!("{}/lib/python{}.{}/site-packages", venv, major, minor);

    let sys = py
        .import("sys")
        .map_err(|e| G2pError::PythonInit(format!("import sys: {e}")))?;

    // Reset sys.argv to prevent misaki from parsing cargo test's flags
    // (some versions call an argument parser at G2P() construction time).
    sys.getattr("argv")
        .map_err(|e| G2pError::PythonInit(format!("sys.argv: {e}")))?
        .call_method1("clear", ())
        .map_err(|e| G2pError::PythonInit(format!("sys.argv.clear: {e}")))?;
    sys.getattr("argv")
        .map_err(|e| G2pError::PythonInit(format!("sys.argv: {e}")))?
        .call_method1("append", ("",))
        .map_err(|e| G2pError::PythonInit(format!("sys.argv.append: {e}")))?;

    sys.getattr("path")
        .map_err(|e| G2pError::PythonInit(format!("sys.path: {e}")))?
        .call_method1("insert", (0, &site_packages))
        .map_err(|e| G2pError::PythonInit(format!("sys.path.insert: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Unit tests for engine internals are minimal since most behavior
    // requires a live Python interpreter, which is covered by the
    // integration tests in tests/integration.rs.
    //
    // Run integration tests with:
    //   source .venv/bin/activate && cargo test --test integration -- --ignored
}


