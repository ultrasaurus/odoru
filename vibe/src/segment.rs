use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

const SCHEMA_VERSION: &str = "0.1";

/// `<basedir>/<name>.segments.json` — see `vibe/dev/odoru-import.md` for the
/// full design. Vibe writes the `sentences`/`files.original` parts of this at
/// split time; `synthesize` fills in `files.normalized`/`audio`/`transcript`/
/// `report` and `voice_id` per segment as each one is rendered.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Sidecar {
    pub schema_version: String,
    pub source_document: String,
    pub source_sha256: String,
    pub voice_id: Option<String>,
    pub segments: Vec<SidecarSegment>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SidecarSegment {
    pub index: u32,
    pub sentences: Vec<SidecarSentence>,
    pub files: SidecarFiles,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SidecarSentence {
    pub text: String,
    pub paragraph_end: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SidecarFiles {
    pub original: Option<String>,
    pub normalized: Option<String>,
    pub audio: Option<String>,
    pub transcript: Option<String>,
    pub report: Option<String>,
}

fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// Build the sentence list for one segment from its constituent
/// paragraph-unit strings (as returned by `segmenter::segment_with_paragraphs`
/// for a single segment).
///
/// Runs `splitter::split` per paragraph-unit, not on the whole segment —
/// `splitter::split`'s paragraph detection needs a blank line, which doesn't
/// survive joining paragraph-units with a single `\n`. Splitting each unit
/// individually makes `splitter::split` correctly treat it as one paragraph,
/// marking only its own last sentence `paragraph_end: true`.
fn sentences_for_segment(paragraphs: &[String]) -> Vec<SidecarSentence> {
    paragraphs
        .iter()
        .flat_map(|p| util::splitter::split(p))
        .map(|s| SidecarSentence { text: s.text, paragraph_end: s.paragraph_end })
        .collect()
}

/// Build the sidecar for a document from its source text and the
/// per-segment paragraph groups (`segmenter::segment_with_paragraphs`'s
/// output). `name` is the document stem (e.g. "authorship").
fn build_sidecar(name: &str, source_text: &str, segments: &[Vec<String>]) -> Sidecar {
    Sidecar {
        schema_version: SCHEMA_VERSION.to_string(),
        source_document: format!("{name}.txt"),
        source_sha256: sha256_hex(source_text),
        voice_id: None,
        segments: segments
            .iter()
            .enumerate()
            .map(|(i, paragraphs)| {
                let seg_name = format!("{name}_seg{:02}", i + 1);
                SidecarSegment {
                    index: (i + 1) as u32,
                    sentences: sentences_for_segment(paragraphs),
                    files: SidecarFiles {
                        original: Some(format!("{seg_name}.txt")),
                        ..Default::default()
                    },
                }
            })
            .collect(),
    }
}

/// Split a document into TTS segments and write them as numbered files.
/// Reads `<basedir>/<name>.txt`, writes `<basedir>/<name>_seg01.txt` etc.
/// with `Speaker 1: ` prefix per paragraph, plus `<basedir>/<name>.segments.json`.
pub fn run(name: &str, basedir: Option<&str>) -> Result<()> {
    // Source documents live in the workspace data/ dir (odoru/data/).
    // Segment files are written to --basedir (default: vibe/data).
    let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../data");
    let default_seg_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
    let seg_dir = basedir.unwrap_or(default_seg_dir);
    let input_path = format!("{src_dir}/{name}.txt");
    let text = std::fs::read_to_string(&input_path)
        .with_context(|| format!("reading {input_path}"))?;

    let segments = util::segmenter::segment_with_paragraphs(&text);
    info!("{} → {} segments", name, segments.len());
    for (i, paragraphs) in segments.iter().enumerate() {
        let seg_name = format!("{name}_seg{:02}", i + 1);
        let seg_path = format!("{seg_dir}/{seg_name}.txt");
        let content: String = paragraphs.iter()
            .map(|p| format!("Speaker 1: {p}\n"))
            .collect();
        std::fs::write(&seg_path, &content)
            .with_context(|| format!("writing {seg_path}"))?;
        let wc: usize = paragraphs.iter().map(|p| p.split_whitespace().count()).sum();
        info!("  {seg_name}: {} paragraphs, {wc} words", paragraphs.len());
    }

    let sidecar = build_sidecar(name, &text, &segments);
    let sidecar_path = format!("{seg_dir}/{name}.segments.json");
    let sidecar_json = serde_json::to_string_pretty(&sidecar)
        .context("serializing sidecar")?;
    std::fs::write(&sidecar_path, sidecar_json + "\n")
        .with_context(|| format!("writing {sidecar_path}"))?;
    info!("wrote {sidecar_path}");

    Ok(())
}

/// Split a segment stem like "authorship_seg01" into its document name and
/// 1-based segment index. Returns `None` if it doesn't match that pattern
/// (e.g. a name not produced by `run` above).
fn parse_segment_name(name: &str) -> Option<(&str, u32)> {
    let pos = name.rfind("_seg")?;
    let (doc_name, rest) = (&name[..pos], &name[pos + 4..]);
    let index: u32 = rest.parse().ok()?;
    Some((doc_name, index))
}

/// Update the sidecar after segment `name` (e.g. "authorship_seg01") has been
/// synthesized: fills in that segment's `files.normalized`/`audio`/
/// `transcript`/`report`, and sets the document's `voice_id` if not already
/// set. Logs a warning and does nothing if the sidecar doesn't exist or
/// doesn't contain a matching segment — synthesis output is already on disk
/// regardless, so this is not fatal to the synthesize command.
pub fn record_synthesis(basedir: &str, name: &str, voice_id: &str) {
    let Some((doc_name, index)) = parse_segment_name(name) else {
        warn!("{name} doesn't look like <doc>_segNN — skipping sidecar update");
        return;
    };

    let sidecar_path = format!("{basedir}/{doc_name}.segments.json");
    let sidecar_json = match std::fs::read_to_string(&sidecar_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("reading {sidecar_path} for sidecar update: {e} — skipping");
            return;
        }
    };
    let mut sidecar: Sidecar = match serde_json::from_str(&sidecar_json) {
        Ok(s) => s,
        Err(e) => {
            warn!("parsing {sidecar_path}: {e} — skipping sidecar update");
            return;
        }
    };

    let Some(seg) = sidecar.segments.iter_mut().find(|s| s.index == index) else {
        warn!("{sidecar_path} has no segment with index {index} — skipping sidecar update");
        return;
    };
    seg.files.normalized = Some(format!("{name}_normalized.txt"));
    seg.files.audio = Some(format!("{name}_generated.wav"));
    seg.files.transcript = Some(format!("{name}_transcript.json"));
    seg.files.report = Some(format!("{name}_report.json"));

    match &sidecar.voice_id {
        None => sidecar.voice_id = Some(voice_id.to_string()),
        Some(existing) if existing != voice_id => {
            warn!(
                "{sidecar_path}: voice_id was {existing:?}, this segment used {voice_id:?} — \
                 leaving recorded voice_id unchanged. Document may be using more than one voice."
            );
        }
        Some(_) => {}
    }

    match serde_json::to_string_pretty(&sidecar) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&sidecar_path, json + "\n") {
                warn!("writing {sidecar_path}: {e}");
            } else {
                info!("updated {sidecar_path} for segment {index}");
            }
        }
        Err(e) => warn!("serializing {sidecar_path}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_segment_name_splits_doc_and_index() {
        assert_eq!(parse_segment_name("authorship_seg01"), Some(("authorship", 1)));
        assert_eq!(parse_segment_name("authorship_seg35"), Some(("authorship", 35)));
        // doc name itself contains "_seg" earlier — rfind picks the last one.
        assert_eq!(parse_segment_name("my_segment_doc_seg02"), Some(("my_segment_doc", 2)));
    }

    #[test]
    fn parse_segment_name_rejects_non_matching_names() {
        assert_eq!(parse_segment_name("authorship"), None);
        assert_eq!(parse_segment_name("authorship_segXX"), None);
    }

    fn write_sidecar(dir: &std::path::Path, doc_name: &str, sidecar: &Sidecar) {
        let path = dir.join(format!("{doc_name}.segments.json"));
        std::fs::write(path, serde_json::to_string(sidecar).unwrap()).unwrap();
    }

    fn read_sidecar(dir: &std::path::Path, doc_name: &str) -> Sidecar {
        let path = dir.join(format!("{doc_name}.segments.json"));
        let json = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    /// Two paragraphs, each long enough (>MAX words) to land in its own
    /// segment, so tests can verify one segment is updated without
    /// disturbing the other.
    fn sample_source() -> String {
        let long_para = "word ".repeat(260).trim().to_string() + ".";
        format!("{long_para}\n\n{long_para}")
    }

    fn sample_sidecar() -> Sidecar {
        let source = sample_source();
        build_sidecar("doc", &source, &util::segmenter::segment_with_paragraphs(&source))
    }

    #[test]
    fn record_synthesis_fills_in_files_and_voice_id() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(dir.path(), "doc", &sample_sidecar());

        record_synthesis(dir.path().to_str().unwrap(), "doc_seg01", "vibevoice:default");

        let sidecar = read_sidecar(dir.path(), "doc");
        assert_eq!(sidecar.voice_id, Some("vibevoice:default".to_string()));
        let seg = sidecar.segments.iter().find(|s| s.index == 1).unwrap();
        assert_eq!(seg.files.normalized, Some("doc_seg01_normalized.txt".to_string()));
        assert_eq!(seg.files.audio, Some("doc_seg01_generated.wav".to_string()));
        assert_eq!(seg.files.transcript, Some("doc_seg01_transcript.json".to_string()));
        assert_eq!(seg.files.report, Some("doc_seg01_report.json".to_string()));
        // Other segment untouched.
        let other = sidecar.segments.iter().find(|s| s.index != 1).unwrap();
        assert_eq!(other.files.audio, None);
    }

    #[test]
    fn record_synthesis_does_not_overwrite_existing_voice_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = sample_sidecar();
        sidecar.voice_id = Some("vibevoice:default".to_string());
        write_sidecar(dir.path(), "doc", &sidecar);

        record_synthesis(dir.path().to_str().unwrap(), "doc_seg01", "some-other-voice");

        let sidecar = read_sidecar(dir.path(), "doc");
        assert_eq!(sidecar.voice_id, Some("vibevoice:default".to_string()));
    }

    #[test]
    fn record_synthesis_missing_sidecar_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // No sidecar written — should warn and return, not panic or create one.
        record_synthesis(dir.path().to_str().unwrap(), "doc_seg01", "vibevoice:default");
        assert!(!dir.path().join("doc.segments.json").exists());
    }

    #[test]
    fn sentences_for_segment_marks_paragraph_end_per_unit() {
        let paragraphs = vec![
            "First sentence. Second sentence.".to_string(),
            "Third sentence.".to_string(),
        ];
        let sentences = sentences_for_segment(&paragraphs);
        let flags: Vec<bool> = sentences.iter().map(|s| s.paragraph_end).collect();
        // Each paragraph-unit's own last sentence is paragraph_end, not just
        // the segment's overall last sentence.
        assert_eq!(flags, vec![false, true, true]);
        assert_eq!(sentences[0].text, "First sentence.");
        assert_eq!(sentences[1].text, "Second sentence.");
        assert_eq!(sentences[2].text, "Third sentence.");
    }

    #[test]
    fn build_sidecar_indexes_segments_from_one() {
        let source = "First paragraph.\n\nSecond paragraph.";
        let segments = util::segmenter::segment_with_paragraphs(source);
        let sidecar = build_sidecar("doc", source, &segments);

        assert_eq!(sidecar.schema_version, "0.1");
        assert_eq!(sidecar.source_document, "doc.txt");
        assert_eq!(sidecar.voice_id, None);
        assert_eq!(sidecar.segments[0].index, 1);
        assert_eq!(sidecar.segments[0].files.original, Some("doc_seg01.txt".to_string()));
        assert_eq!(sidecar.segments[0].files.audio, None);
    }

    #[test]
    fn build_sidecar_source_sha256_matches_source_text() {
        let source = "Some text.";
        let segments = util::segmenter::segment_with_paragraphs(source);
        let sidecar = build_sidecar("doc", source, &segments);
        assert_eq!(sidecar.source_sha256, sha256_hex(source));
    }

    #[test]
    fn sidecar_roundtrips_through_json() {
        let source = "First paragraph.\n\nSecond paragraph, with more words in it.";
        let segments = util::segmenter::segment_with_paragraphs(source);
        let sidecar = build_sidecar("doc", source, &segments);
        let json = serde_json::to_string(&sidecar).unwrap();
        let parsed: Sidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, sidecar);
    }
}
