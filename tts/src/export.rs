//! Cache-based audio export — assembles per-sentence MP3s from the audio
//! disk cache without running TTS synthesis.

use crate::audio_cache;
use crate::backend::Voice;
use crate::splitter;

/// One sentence's worth of export data.
pub struct SentenceAudio {
    pub index: usize,
    pub text: String,
    /// Raw MP3 bytes from the audio cache.  `None` on a cache miss.
    pub mp3: Option<Vec<u8>>,
    /// Duration in seconds (from cache metadata).  `0.0` on a cache miss.
    pub duration: f64,
    pub paragraph_end: bool,
}

/// Split `text` into sentences and look each one up in the audio cache for
/// `voice`.  Always returns one entry per sentence — callers inspect `mp3` to
/// detect misses and decide whether to fall back to text-only export.
pub fn export_audio(text: &str, voice: &Voice) -> Vec<SentenceAudio> {
    splitter::split(text)
        .into_iter()
        .enumerate()
        .map(|(index, sentence)| {
            let (mp3, duration) = lookup_sentence(&sentence.text, voice);
            SentenceAudio {
                index,
                text: sentence.text,
                mp3,
                duration,
                paragraph_end: sentence.paragraph_end,
            }
        })
        .collect()
}

fn lookup_sentence(text: &str, voice: &Voice) -> (Option<Vec<u8>>, f64) {
    let key = match voice {
        Voice::F5Tts { .. } => {
            let normalized = crate::f5::normalizer::normalize(text);
            audio_cache::cache_key(&normalized, &voice.cache_key())
        }
        Voice::Kokoro { .. } => {
            audio_cache::cache_key(text, &voice.cache_key())
        }
        Voice::Mock => return (None, 0.0),
    };

    match audio_cache::lookup(&key) {
        Some((mp3, duration)) => (Some(mp3), duration),
        None => (None, 0.0),
    }
}
