//! G2P engine — Misaki phonemizer bridge, used by the Kokoro backend.

use std::sync::mpsc;
use std::thread;

use futures::stream::Stream;
use pyo3::prelude::*;
use thiserror::Error;
use tokio::sync::oneshot;

use crate::splitter;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PhonemeChunk {
    pub index: usize,
    pub sentence: String,
    pub phonemes: String,
}

#[derive(Debug, Error)]
pub enum G2pError {
    #[error("Python initialisation failed: {0}")]
    PythonInit(String),

    #[error("G2P failed on sentence {index}: {cause}\n  sentence: {sentence:?}")]
    G2pFailed {
        index: usize,
        sentence: String,
        cause: String,
    },

    #[error("G2P engine stream closed unexpectedly")]
    StreamClosed,
}

struct G2pRequest {
    index: usize,
    sentence: String,
    reply: oneshot::Sender<Result<String, G2pError>>,
}

// ---------------------------------------------------------------------------
// G2pEngine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct G2pEngine {
    tx: mpsc::SyncSender<G2pRequest>,
}

impl G2pEngine {
    pub fn new() -> Result<Self, G2pError> {
        Python::attach(|py| -> Result<(), G2pError> {
            crate::python::setup::setup(py)
                .map_err(|e| G2pError::PythonInit(e.to_string()))?;
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

        let (tx, rx) = mpsc::sync_channel::<G2pRequest>(64);

        thread::Builder::new()
            .name("misaki-g2p-worker".into())
            .spawn(move || python_worker(rx))
            .map_err(|e| G2pError::PythonInit(format!("Failed to spawn worker thread: {e}")))?;

        Ok(Self { tx })
    }

    pub fn phonemize(
        &self,
        text: impl Into<String>,
    ) -> impl Stream<Item = Result<PhonemeChunk, G2pError>> {
        let text = text.into();
        let sentences: Vec<String> = splitter::split(&text)
            .into_iter()
            .map(|s| s.text)
            .collect();

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
                async move {
                    send_result.map_err(|_| G2pError::StreamClosed)?;
                    let phonemes = reply_rx
                        .await
                        .map_err(|_| G2pError::StreamClosed)??;
                    Ok(PhonemeChunk { index, sentence, phonemes })
                }
            })
            .collect();

        futures
    }
}

fn python_worker(rx: mpsc::Receiver<G2pRequest>) {
    let g2p = match make_g2p_object() {
        Ok(obj) => obj,
        Err(e) => {
            for req in rx {
                let _ = req.reply.send(Err(G2pError::PythonInit(e.clone())));
            }
            return;
        }
    };

    for req in rx {
        let result = call_g2p(&g2p, &req);
        let _ = req.reply.send(result);
    }
}

fn make_g2p_object() -> Result<Py<PyAny>, String> {
    Python::attach(|py| {
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

fn call_g2p(g2p: &Py<PyAny>, req: &G2pRequest) -> Result<String, G2pError> {
    Python::attach(|py| {
        let result = g2p
            .call1(py, (&req.sentence,))
            .map_err(|e| G2pError::G2pFailed {
                index: req.index,
                sentence: req.sentence.clone(),
                cause: e.to_string(),
            })?;

        let phonemes: String = result
            .bind(py)
            .get_item(0)
            .and_then(|item| item.extract::<String>())
            .map_err(|e| G2pError::G2pFailed {
                index: req.index,
                sentence: req.sentence.clone(),
                cause: format!("could not extract phoneme string: {e}"),
            })?;

        Ok(phonemes)
    })
}

#[cfg(test)]
mod tests {
    // Integration tests in tests/integration.rs cover G2P behavior.
}
