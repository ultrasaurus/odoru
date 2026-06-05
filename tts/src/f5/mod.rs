//! F5-TTS backend — MLX inference via Python bridge.
//!
//! # Threading model
//!
//! MLX creates a GPU stream on the OS thread where the model is first used,
//! and that stream cannot be accessed from any other thread. Using
//! `spawn_blocking` is therefore unsafe for F5: tokio's blocking thread pool
//! may dispatch consecutive calls to different OS threads.
//!
//! Fix: each worker is a **dedicated `std::thread`** that owns its Python
//! module for its entire lifetime. Work items are sent to it over a
//! `std::sync::mpsc` channel. The worker thread loops, processes one sentence
//! at a time, and sends the result back over a one-shot reply channel.
//!
//! `TtsBackend::synthesize_sentence` sends a work item and blocks on the
//! reply — this call already happens inside `tokio::task::spawn_blocking`,
//! so the async executor is never blocked.

pub mod normalizer;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;

use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use tracing::{info, warn};
use crate::backend::Voice;
use crate::engine::TtsBackend;
use crate::error::TtsError;

// ---------------------------------------------------------------------------
// Work item sent to a worker thread
// ---------------------------------------------------------------------------

struct WorkItem {
    text:          String,
    voice_ref:     String,
    ref_text:      String,
    speed:         f32,
    cfg_strength:  f32,
    index:         usize,
    reply:         std_mpsc::SyncSender<Result<(Vec<f32>, u32, f64), TtsError>>,
}

// ---------------------------------------------------------------------------
// F5Backend
// ---------------------------------------------------------------------------

pub struct F5Backend {
    /// One sender per worker thread, round-robin dispatched.
    senders: Vec<std_mpsc::SyncSender<WorkItem>>,
    /// Round-robin counter.
    next_worker: AtomicUsize,
}

// The senders are Send; the Python state lives only inside the worker threads.
unsafe impl Send for F5Backend {}
unsafe impl Sync for F5Backend {}

impl F5Backend {
    pub fn init(worker_count: usize) -> Result<Self, anyhow::Error> {
        let worker_count = worker_count.max(1);
        info!("Loading F5-TTS ({worker_count} worker thread(s))…");

        let mut senders = Vec::with_capacity(worker_count);

        for i in 0..worker_count {
            // Bounded channel — backpressure if the worker is busy.
            let (tx, rx) = std_mpsc::sync_channel::<WorkItem>(4);

            // Load the Python module on the worker thread so the MLX GPU
            // stream is created there and stays there for the process lifetime.
            let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<(), String>>(1);

            std::thread::Builder::new()
                .name(format!("f5-worker-{i}"))
                .spawn(move || {
                    // Initialize the module on this thread.
                    let module = Python::attach(|py| -> PyResult<Py<PyAny>> {
                        crate::python::bridge::load_module(
                            py,
                            c_str!(include_str!("../tts.py")),
                            c"tts.py",
                            c"tts",
                        )
                    });

                    match module {
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                        Ok(module) => {
                            let _ = ready_tx.send(Ok(()));

                            // Process work items until the sender side is dropped.
                            while let Ok(item) = rx.recv() {
                                let result = Python::attach(|py| {
                                    call_python(py, &module, &item)
                                });
                                let _ = item.reply.send(result);
                            }
                            warn!("f5-worker-{i}: channel closed, exiting.");
                        }
                    }
                })
                .map_err(|e| anyhow::anyhow!("Worker {i}: failed to spawn thread: {e}"))?;

            // Wait for the worker to confirm the module loaded successfully.
            match ready_rx.recv() {
                Ok(Ok(())) => info!("f5-worker-{i}: ready."),
                Ok(Err(e)) => anyhow::bail!("Worker {i}: module load failed: {e}"),
                Err(_) => anyhow::bail!("Worker {i}: thread died before signalling ready"),
            }

            senders.push(tx);
        }

        info!("F5-TTS ready ({worker_count} worker(s)).");
        Ok(Self { senders, next_worker: AtomicUsize::new(0) })
    }

    fn pick_sender(&self) -> &std_mpsc::SyncSender<WorkItem> {
        let idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len();
        &self.senders[idx]
    }
}

impl TtsBackend for F5Backend {
    fn synthesize_sentence(
        &self,
        text: &str,
        voice: &Voice,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let text = normalizer::normalize(text);

        let (voice_ref, ref_text, speed, cfg_strength) = match voice {
            Voice::F5Tts { voice_ref, ref_text, speed, cfg_strength, .. } => (
                voice_ref.to_string_lossy().into_owned(),
                ref_text.clone(),
                *speed,
                *cfg_strength,
            ),
            _ => return Err(TtsError::UnknownVoice(
                format!("F5Backend received non-F5 voice: {:?}", voice.name())
            )),
        };

        // One-shot reply channel.
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);

        let item = WorkItem { text, voice_ref, ref_text, speed, cfg_strength, index, reply: reply_tx };

        self.pick_sender()
            .send(item)
            .map_err(|_| TtsError::StreamClosed)?;

        reply_rx
            .recv()
            .map_err(|_| TtsError::StreamClosed)?
    }
}

// ---------------------------------------------------------------------------
// Python call — runs on the worker thread
// ---------------------------------------------------------------------------

fn call_python(
    py: Python,
    module: &Py<PyAny>,
    item: &WorkItem,
) -> Result<(Vec<f32>, u32, f64), TtsError> {
    let text = &item.text;
    let index = item.index;

    let kwargs = PyDict::new(py);
    kwargs.set_item("backend", "f5_tts").map_err(|e| synth_err(index, text, e))?;
    kwargs.set_item("voice_ref", &item.voice_ref).map_err(|e| synth_err(index, text, e))?;
    kwargs.set_item("ref_text", &item.ref_text).map_err(|e| synth_err(index, text, e))?;
    kwargs.set_item("speed", item.speed).map_err(|e| synth_err(index, text, e))?;
    kwargs.set_item("cfg_strength", item.cfg_strength).map_err(|e| synth_err(index, text, e))?;

    let result = module.bind(py)
        .getattr("synthesize_sentence")
        .and_then(|f| f.call((text.as_str(),), Some(&kwargs)))
        .map_err(|e| synth_err(index, text, e))?;

    let samples = crate::python::bridge::extract_f32_list(&result, "samples")
        .map_err(|e| synth_err(index, text, e))?;
    let sample_rate = crate::python::bridge::extract_u32(&result, "sample_rate")
        .map_err(|e| synth_err(index, text, e))?;
    let duration = crate::python::bridge::extract_f64(&result, "duration")
        .map_err(|e| synth_err(index, text, e))?;

    Ok((samples, sample_rate, duration))
}

fn synth_err(index: usize, sentence: &str, e: impl std::fmt::Display) -> TtsError {
    TtsError::SynthesisFailed {
        index,
        sentence: sentence.to_owned(),
        cause: e.to_string(),
    }
}
