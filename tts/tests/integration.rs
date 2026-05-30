//! Integration tests for the Misaki G2P bridge.
//!
//! These tests require a real Python venv with `misaki-en` installed.
//! They are marked `#[ignore]` so they don't run in CI without setup.
//!
//! # Running locally
//!
//! ```bash
//! source .venv/bin/activate
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

/// Smoke test: if this fails, all others will too — run: source .venv/bin/activate
#[tokio::test]
#[ignore]
async fn engine_new_with_env_var_succeeds() {
    G2pEngine::new().expect(
        "G2pEngine::new failed — is the venv active and misaki[en] installed?",
    );
}

#[tokio::test]
#[ignore]
async fn engine_new_with_active_venv_succeeds() {
    // VIRTUAL_ENV is set automatically when the venv is active
    let venv = std::env::var("VIRTUAL_ENV")
        .expect("VIRTUAL_ENV must be set — run: source .venv/bin/activate");
    assert!(!venv.is_empty());
    G2pEngine::new().expect("G2pEngine::new failed — is misaki[en] installed in the venv?");
}

#[tokio::test]
#[ignore]
async fn engine_new_with_bad_virtual_env_returns_python_init_error() {
    // Temporarily point VIRTUAL_ENV at a dir with no misaki installed
    std::env::set_var("VIRTUAL_ENV", "/tmp");
    let result = G2pEngine::new();
    std::env::remove_var("VIRTUAL_ENV");
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
    let engine = G2pEngine::new().unwrap();
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
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "").await;
    assert!(chunks.is_empty());
}

#[tokio::test]
#[ignore]
async fn phonemize_whitespace_only_returns_empty_stream() {
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "   \n   \n   ").await;
    assert!(chunks.is_empty());
}

// ── phonemize — ordering ───────────────────────────────────────────────────

/// Chunks must arrive in sentence order, not in arbitrary completion order.
#[tokio::test]
#[ignore]
async fn phonemize_multi_sentence_preserves_order() {
    let engine = G2pEngine::new().unwrap();
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
    let engine = G2pEngine::new().unwrap();
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
    let engine = G2pEngine::new().unwrap();

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

    let engine = Arc::new(G2pEngine::new().unwrap());
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

// ── espeak fallback for unknown proper nouns ───────────────────────────────

/// Verifies that proper nouns unknown to misaki's dictionary (like foreign
/// names) are phonemized via the espeak fallback rather than silently dropped.
///
/// "Contreras" is a Spanish surname that misaki won't have in its English
/// dictionary — it should fall back to espeak and produce non-empty phonemes.
#[tokio::test]
#[ignore]
async fn phonemize_unknown_proper_noun_uses_espeak_fallback() {
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "Contreras is the clearest voice.").await;

    assert_eq!(chunks.len(), 1);
    let phonemes = &chunks[0].phonemes;
    assert!(!phonemes.is_empty(), "phoneme string was empty for sentence containing 'Contreras'");

    // The phoneme string should be substantially long — if "Contreras" was
    // silently dropped we'd get far fewer phonemes than expected.
    // "is the clearest voice" alone would be ~15 chars; with "Contreras" ~25+.
    assert!(
        phonemes.chars().count() >= 20,
        "phoneme string suspiciously short ({} chars) — 'Contreras' may have been dropped: {:?}",
        phonemes.chars().count(),
        phonemes
    );
}

/// Verifies that the tokenizer doesn't silently drop phonemes for words
/// phonemized via espeak — espeak may produce characters not in our vocab.
#[tokio::test]
#[ignore]
async fn tokenizer_preserves_espeak_phonemes() {
    use ko_odoru::synth::{build_vocab, tokenize};
    use std::path::PathBuf;

    let model_dir = std::env::var("KOKORO_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap()).join(".kokoro")
        });

    let vocab = build_vocab(&model_dir).expect("build_vocab failed");
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "Contreras is the clearest voice.").await;

    assert_eq!(chunks.len(), 1);
    let phonemes = &chunks[0].phonemes;

    let token_ids = tokenize(phonemes, &vocab);

    // Count how many phoneme chars are actually in the vocab
    let total_chars = phonemes.chars().count();
    let mapped_chars = phonemes.chars().filter(|c| vocab.contains_key(c)).count();
    let missing: Vec<char> = phonemes.chars().filter(|c| !vocab.contains_key(c)).collect();

    println!("phonemes: {:?}", phonemes);
    println!("total chars: {total_chars}, mapped: {mapped_chars}, missing: {}", missing.len());
    println!("missing chars: {:?}", missing);
    println!("token_ids: {:?}", token_ids);

    assert!(
        !token_ids.is_empty(),
        "all phonemes for 'Contreras' sentence were dropped by tokenizer"
    );

    // Warn if more than 20% of chars are missing from vocab — suggests
    // espeak is producing phonemes we don't support
    let missing_pct = (missing.len() as f64 / total_chars as f64) * 100.0;
    assert!(
        missing_pct < 20.0,
        "{:.0}% of phoneme chars missing from vocab — espeak may be using unsupported symbols: {:?}",
        missing_pct,
        missing
    );
}
