//! Integration tests for the G2P bridge and TtsEngine.
//!
//! These tests require a real Python venv with `misaki-en` installed.
//! They are marked `#[ignore]` so they don't run in CI without setup.
//!
//! # Running locally
//!
//! ```bash
//! source .venv/bin/activate
//! cargo test --test integration -- --ignored
//! ```

use futures::StreamExt;
use tts::{G2pEngine, TtsEngine, Backend};
use std::sync::Mutex;

// Serialize all tests — Python state and VIRTUAL_ENV are process-global.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    // Use unwrap_or_else so a poisoned mutex (from a previous test panic)
    // doesn't cascade failures to all subsequent tests.
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ── Helpers ────────────────────────────────────────────────────────────────

async fn collect_ok(engine: &G2pEngine, text: &str) -> Vec<tts::PhonemeChunk> {
    engine
        .phonemize(text)
        .map(|r| r.expect("unexpected G2pError in stream"))
        .collect()
        .await
}

fn try_build_engine() -> Option<TtsEngine> {
    if std::env::var("VIRTUAL_ENV").is_err() {
        eprintln!("Skipping: venv not active (source .venv/bin/activate)");
        return None;
    }
    Some(TtsEngine::builder().backend(Backend::Mock).build().expect("build failed"))
}

// ── G2pEngine initialisation ───────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn engine_new_with_env_var_succeeds() {
    let _lock = lock();
    G2pEngine::new().expect(
        "G2pEngine::new failed — is the venv active and misaki[en] installed?",
    );
}

#[tokio::test]
#[ignore]
async fn engine_new_with_active_venv_succeeds() {
    let _lock = lock();
    let venv = std::env::var("VIRTUAL_ENV")
        .expect("VIRTUAL_ENV must be set — run: source .venv/bin/activate");
    assert!(!venv.is_empty());
    G2pEngine::new().expect("G2pEngine::new failed — is misaki[en] installed in the venv?");
}

// ── G2P phonemize ──────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn phonemize_single_sentence_returns_one_chunk() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "Hello world.").await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].index, 0);
    assert_eq!(chunks[0].sentence, "Hello world.");
    assert!(!chunks[0].phonemes.is_empty());
}

#[tokio::test]
#[ignore]
async fn phonemize_empty_input_returns_empty_stream() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    assert!(collect_ok(&engine, "").await.is_empty());
}

#[tokio::test]
#[ignore]
async fn phonemize_whitespace_only_returns_empty_stream() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    assert!(collect_ok(&engine, "   \n   \n   ").await.is_empty());
}

#[tokio::test]
#[ignore]
async fn phonemize_multi_sentence_preserves_order() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    let input = "The cat sat on the mat.\nThe dog ran in the fog.\nThe hen laid an egg.";
    let chunks = collect_ok(&engine, input).await;
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].sentence, "The cat sat on the mat.");
    assert_eq!(chunks[1].sentence, "The dog ran in the fog.");
    assert_eq!(chunks[2].sentence, "The hen laid an egg.");
}

#[tokio::test]
#[ignore]
async fn phonemize_chunk_indices_are_zero_based_and_contiguous() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "One.\nTwo.\nThree.\nFour.\nFive.").await;
    assert_eq!(chunks.len(), 5);
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i);
    }
}

#[tokio::test]
#[ignore]
async fn engine_can_be_reused_across_multiple_phonemize_calls() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    for i in 0..5 {
        let text = format!("Call number {i}.");
        let chunks = collect_ok(&engine, &text).await;
        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].phonemes.is_empty());
    }
}

#[tokio::test]
#[ignore]
async fn engine_shared_via_arc_across_tasks() {
    let _lock = lock();
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

// ── espeak fallback ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn phonemize_unknown_proper_noun_uses_espeak_fallback() {
    let _lock = lock();
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "Contreras is the clearest voice.").await;
    assert_eq!(chunks.len(), 1);
    let phonemes = &chunks[0].phonemes;
    assert!(!phonemes.is_empty());
    assert!(
        phonemes.chars().count() >= 20,
        "phoneme string suspiciously short ({} chars): {:?}",
        phonemes.chars().count(), phonemes
    );
}

#[tokio::test]
#[ignore]
async fn tokenizer_preserves_espeak_phonemes() {
    let _lock = lock();
    use tts::kokoro::{build_vocab, tokenize};
    use std::path::PathBuf;

    let model_dir = std::env::var("KOKORO_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join(".kokoro"));

    let vocab = build_vocab(&model_dir).expect("build_vocab failed");
    let engine = G2pEngine::new().unwrap();
    let chunks = collect_ok(&engine, "Contreras is the clearest voice.").await;
    let phonemes = &chunks[0].phonemes;
    let token_ids = tokenize(phonemes, &vocab);

    let total_chars = phonemes.chars().count();
    let missing: Vec<char> = phonemes.chars().filter(|c| !vocab.contains_key(c)).collect();
    assert!(!token_ids.is_empty(), "all phonemes were dropped");
    let missing_pct = (missing.len() as f64 / total_chars as f64) * 100.0;
    assert!(missing_pct < 20.0, "{:.0}% missing: {:?}", missing_pct, missing);
}

// ── TtsEngine tests ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn tts_engine_single_sentence_yields_one_segment() {
    let _lock = lock();
    let Some(engine) = try_build_engine() else { return; };
    let mut stream = engine.synthesize("Hello world.");
    let seg = stream.next().await.unwrap().expect("segment failed");
    assert!(!seg.samples.is_empty());
    assert_eq!(seg.sample_rate, 24_000);
    assert_eq!(seg.index, 0);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
#[ignore]
async fn tts_engine_segment_timestamps_are_monotonic() {
    let _lock = lock();
    let Some(engine) = try_build_engine() else { return; };
    let mut stream = engine.synthesize("Hello world. The cat sat on the mat. How are you?");
    let mut segments = vec![];
    while let Some(result) = stream.next().await {
        segments.push(result.expect("segment failed"));
    }
    assert_eq!(segments.len(), 3);
    for w in segments.windows(2) {
        assert!(w[1].transcript.start >= w[0].transcript.end);
    }
}

/// Word-level timestamps — ignored until word alignment is implemented.
#[tokio::test]
#[ignore]
async fn single_segment_start_end_match_words() {
    let _lock = lock();
    let Some(engine) = try_build_engine() else { return; };
    let mut stream = engine.synthesize("Hello world.");
    let seg = stream.next().await.unwrap().expect("segment failed");
    assert!(stream.next().await.is_none());
    assert!(!seg.samples.is_empty());
    assert_eq!(seg.index, 0);
}
