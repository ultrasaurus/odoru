//! Integration tests for the Misaki G2P bridge.
//!
//! These tests require a real Python venv with `misaki-en` installed.
//! They are marked `#[ignore]` so they don't run in CI without setup.
//!
//! # Running locally
//!
//! ```bash
//! ./setup.sh
//! export MISAKI_VENV=~/.misaki-g2p/venv
//! export PYO3_PYTHON=~/.misaki-g2p/venv/bin/python
//!
//! # Run only integration tests
//! cargo test --test integration -- --ignored
//!
//! # Run everything (unit + integration)
//! cargo test -- --include-ignored
//! ```

use futures::StreamExt;
use ko_odoru::{G2pEngine, G2pError};

// ── Helpers ────────────────────────────────────────────────────────────────

/// Collect all chunks from a phonemize stream, panicking on unexpected errors.
async fn collect_ok(engine: &G2pEngine, text: &str) -> Vec<ko_odoru::PhonemeChunk> {
    engine
        .phonemize(text)
        .map(|r| r.expect("unexpected G2pError in stream"))
        .collect()
        .await
}

// ── Engine initialisation ──────────────────────────────────────────────────

/// Smoke test: if this fails, all others will too — check your MISAKI_VENV.
#[tokio::test]
#[ignore]
async fn engine_new_with_env_var_succeeds() {
    G2pEngine::new(None).expect(
        "G2pEngine::new failed — is MISAKI_VENV set and does it contain misaki-en?",
    );
}

#[tokio::test]
#[ignore]
async fn engine_new_with_explicit_path_succeeds() {
    let venv = std::env::var("MISAKI_VENV")
        .expect("MISAKI_VENV must be set to run integration tests");
    G2pEngine::new(Some(std::path::Path::new(&venv)))
        .expect("G2pEngine::new failed with explicit path");
}

#[tokio::test]
#[ignore]
async fn engine_new_with_bad_venv_returns_python_init_error() {
    let result = G2pEngine::new(Some(std::path::Path::new("/tmp"))); // exists, but no misaki
    assert!(
        matches!(result, Err(G2pError::PythonInit(_))),
        "expected PythonInit error, got: {:?}",
        result
    );
}

// ── phonemize — basic correctness ─────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn phonemize_single_sentence_returns_one_chunk() {
    let engine = G2pEngine::new(None).unwrap();
    let chunks = collect_ok(&engine, "Hello world.").await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].index, 0);
    assert_eq!(chunks[0].sentence, "Hello world.");
    // Phoneme string must be non-empty; exact value is Misaki's business.
    assert!(!chunks[0].phonemes.is_empty(), "phoneme string was empty");
}

#[tokio::test]
#[ignore]
async fn phonemize_empty_input_returns_empty_stream() {
    let engine = G2pEngine::new(None).unwrap();
    let chunks = collect_ok(&engine, "").await;
    assert!(chunks.is_empty());
}

#[tokio::test]
#[ignore]
async fn phonemize_whitespace_only_returns_empty_stream() {
    let engine = G2pEngine::new(None).unwrap();
    let chunks = collect_ok(&engine, "   \n   \n   ").await;
    assert!(chunks.is_empty());
}

// ── phonemize — ordering ───────────────────────────────────────────────────

/// Chunks must arrive in sentence order, not in arbitrary completion order.
#[tokio::test]
#[ignore]
async fn phonemize_multi_sentence_preserves_order() {
    let engine = G2pEngine::new(None).unwrap();
    let input = "The cat sat on the mat.\nThe dog ran in the fog.\nThe hen laid an egg.";

    let chunks = collect_ok(&engine, input).await;

    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].index, 0);
    assert_eq!(chunks[1].index, 1);
    assert_eq!(chunks[2].index, 2);
    assert_eq!(chunks[0].sentence, "The cat sat on the mat.");
    assert_eq!(chunks[1].sentence, "The dog ran in the fog.");
    assert_eq!(chunks[2].sentence, "The hen laid an egg.");
}

/// Indices must be contiguous, zero-based, and match sentence position.
#[tokio::test]
#[ignore]
async fn phonemize_chunk_indices_are_zero_based_and_contiguous() {
    let engine = G2pEngine::new(None).unwrap();
    let input = "One.\nTwo.\nThree.\nFour.\nFive.";

    let chunks = collect_ok(&engine, input).await;

    assert_eq!(chunks.len(), 5);
    for (expected_idx, chunk) in chunks.iter().enumerate() {
        assert_eq!(
            chunk.index, expected_idx,
            "chunk at position {expected_idx} had index {}",
            chunk.index
        );
    }
}

// ── phonemize — error handling ─────────────────────────────────────────────

// G2pFailed error path tests are omitted: Misaki is intentionally robust and
// handles unusual input (null bytes, symbols, empty-ish strings) without raising
// Python exceptions. The stream-continues-after-error logic in engine.rs is
// correct, but there is no reliable way to trigger G2pFailed through the public
// API without access to Misaki internals. If a real failure mode is identified,
// add a targeted test here.

// ── phonemize — repeated use ───────────────────────────────────────────────

/// The same engine should be usable across multiple phonemize calls.
#[tokio::test]
#[ignore]
async fn engine_can_be_reused_across_multiple_phonemize_calls() {
    let engine = G2pEngine::new(None).unwrap();

    for i in 0..5 {
        let text = format!("Call number {i}.");
        let chunks = collect_ok(&engine, &text).await;
        assert_eq!(chunks.len(), 1, "call {i} returned wrong chunk count");
        assert!(!chunks[0].phonemes.is_empty(), "call {i} returned empty phonemes");
    }
}

/// Engine wrapped in Arc can be shared across concurrent tasks.
#[tokio::test]
#[ignore]
async fn engine_shared_via_arc_across_tasks() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let engine = Arc::new(G2pEngine::new(None).unwrap());
    let mut set = JoinSet::new();

    for i in 0..4 {
        let eng = Arc::clone(&engine);
        set.spawn(async move {
            let text = format!("Task {i} says hello.");
            let chunks: Vec<_> = eng
                .phonemize(text)
                .map(|r| r.expect("error in concurrent task"))
                .collect()
                .await;
            assert_eq!(chunks.len(), 1);
            assert!(!chunks[0].phonemes.is_empty());
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("task panicked");
    }
}
