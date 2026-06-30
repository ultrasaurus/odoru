use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use util::segment_types::{Sidecar, SidecarFiles, SidecarSegment, SidecarSentence};

const SCHEMA_VERSION: &str = "0.1";

/// Resolves a `--basedir` argument: if `None`, defaults to `vibe/data`. If
/// relative, joins it under `vibe/data` (so `--basedir augment/foo` means
/// `vibe/data/augment/foo`, matching every documented use case). Absolute
/// paths are used as-is.
pub fn resolve_basedir(basedir: Option<&str>) -> String {
    let default_data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
    match basedir {
        None => default_data_dir.to_string(),
        Some(dir) if std::path::Path::new(dir).is_absolute() => dir.to_string(),
        Some(dir) => format!("{default_data_dir}/{dir}"),
    }
}

/// `<segment_name>_report.json`, as written by `synthesize` after each
/// segment's forced-alignment QA pass.
#[derive(Debug, Deserialize)]
pub struct AlignReport {
    pub filtered: Vec<serde_json::Value>,
    pub suspect: Vec<AlignSuspect>,
}

#[derive(Debug, Deserialize)]
pub struct AlignSuspect {
    pub word: String,
    pub score: f64,
    pub reason: String,
}

impl AlignReport {
    pub fn truncated(&self) -> Vec<&AlignSuspect> {
        self.suspect.iter().filter(|s| s.reason == "Truncated").collect()
    }

    pub fn low_score(&self) -> Vec<&AlignSuspect> {
        self.suspect.iter().filter(|s| s.reason == "LowScore").collect()
    }

    /// Same classification `synthesize` logs as separate lines, collapsed
    /// into one line for `summary`'s per-segment table.
    pub fn one_line(&self) -> String {
        if self.suspect.is_empty() && self.filtered.is_empty() {
            return "clean".to_string();
        }
        let mut parts = Vec::new();
        let truncated = self.truncated();
        if !truncated.is_empty() {
            let words: Vec<_> = truncated.iter().map(|s| format!("{}({:.2})", s.word, s.score)).collect();
            parts.push(format!("⚠ TRUNCATED — {}", words.join(" ")));
        }
        let low = self.low_score();
        if !low.is_empty() {
            let words: Vec<_> = low.iter().map(|s| format!("{}({:.2})", s.word, s.score)).collect();
            parts.push(format!("low-score — {}", words.join(" ")));
        }
        if !self.filtered.is_empty() {
            parts.push(format!("{} filtered word(s)", self.filtered.len()));
        }
        parts.join("; ")
    }
}

fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// Assign sentences split from the *original, unmodified* document text to
/// each segment, instead of re-splitting each segment's own paragraph-unit
/// strings.
///
/// `merge_fragments` (see `util::segmenter`) glues a heading like "Abstract"
/// onto the following paragraph with a single space, so segments can be
/// chunked without orphan heading-only segments. But that merged string is
/// also what segment-local splitting used to run `splitter::split` on — and
/// once a heading loses its blank-line boundary, the splitter has nothing to
/// stop on after it, collapsing the heading into the next sentence. That
/// shifts every later sentence's index by one relative to what
/// `tts::splitter::split` computes from the real document text at replay
/// time (see `dev/tts-backends/vibe-playback.md`), which uses a per-sentence
/// index to key its audio cache lookups — any drift here means cached audio
/// silently stops being found partway through a document.
///
/// Splitting `source_text` directly (once, before chunking into segments)
/// can't have this problem, since it's the same input replay uses. The only
/// remaining work is figuring out which of those sentences belong to which
/// segment: a segment's full text (all its paragraph-units joined) is,
/// modulo whitespace, exactly the concatenation of some contiguous run of
/// canonical sentences — so each segment claims canonical sentences off a
/// shared cursor until the running join's length catches up with its own
/// text's length. Comparing on normalized whitespace (not exact bytes)
/// is what makes this robust to `merge_fragments` joining with a single
/// space where the source had a newline/blank line.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assign_canonical_sentences(
    canonical: &[util::splitter::Sentence],
    segments: &[Vec<String>],
) -> Vec<Vec<SidecarSentence>> {
    let mut cursor = 0usize;
    segments
        .iter()
        .map(|paragraphs| {
            if canonical.is_empty() || cursor >= canonical.len() {
                return Vec::new();
            }
            let target = normalize_ws(&paragraphs.join(" "));
            let mut end = cursor;
            let mut joined = normalize_ws(&canonical[end].text);
            while joined.len() < target.len() && end + 1 < canonical.len() {
                end += 1;
                joined.push(' ');
                joined.push_str(&normalize_ws(&canonical[end].text));
            }
            if joined != target {
                warn!(
                    "segment text didn't match canonical sentences {cursor}..={end} exactly — \
                     sentence indices for this and later segments may be misaligned with the \
                     original document. segment={target:?} canonical={joined:?}"
                );
            }

            let slice = &canonical[cursor..=end];
            cursor = end + 1;
            slice
                .iter()
                .map(|s| SidecarSentence { text: s.text.clone(), paragraph_end: s.paragraph_end })
                .collect()
        })
        .collect()
}

/// Build the sidecar for a document from its source text and the
/// per-segment paragraph groups (`segmenter::segment_with_paragraphs`'s
/// output). `name` is the document stem (e.g. "authorship").
fn build_sidecar(name: &str, source_text: &str, segments: &[Vec<String>]) -> Sidecar {
    let canonical = util::splitter::split(source_text);
    let mut sentences_by_segment = assign_canonical_sentences(&canonical, segments);
    Sidecar {
        schema_version: SCHEMA_VERSION.to_string(),
        source_document: format!("{name}.txt"),
        source_sha256: sha256_hex(source_text),
        voice_id: None,
        segments: segments
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let seg_name = format!("{name}_seg{:02}", i + 1);
                SidecarSegment {
                    index: (i + 1) as u32,
                    sentences: std::mem::take(&mut sentences_by_segment[i]),
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
    let seg_dir = resolve_basedir(basedir);
    let input_path = format!("{seg_dir}/{name}.txt");
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
pub(crate) fn parse_segment_name(name: &str) -> Option<(&str, u32)> {
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

/// Print a per-segment status table for `<basedir>/<name>.segments.json`:
/// which segments are missing audio (not yet synthesized, e.g. after a
/// crashed/interrupted run) and the QA verdict for those that are done.
/// Lets you resume a run without re-reading every log line.
pub fn summary(name: &str, basedir: Option<&str>) -> Result<()> {
    let seg_dir = resolve_basedir(basedir);
    println!("\nlooking for segments in {seg_dir}\n");
    let sidecar_path = format!("{seg_dir}/{name}.segments.json");
    let sidecar_json = std::fs::read_to_string(&sidecar_path)
        .with_context(|| format!("reading {sidecar_path}"))?;
    let sidecar: Sidecar = serde_json::from_str(&sidecar_json)
        .with_context(|| format!("parsing {sidecar_path}"))?;
    let missing = missing_segments(name, basedir)?;
    let missing_set: std::collections::HashSet<&str> = missing.iter().map(String::as_str).collect();

    for seg in &sidecar.segments {
        let seg_name = format!("{name}_seg{:02}", seg.index);
        if missing_set.contains(seg_name.as_str()) {
            println!("{seg_name}: MISSING (not yet synthesized)");
            continue;
        }
        let verdict = seg.files.report.as_deref()
            .and_then(|f| std::fs::read(format!("{seg_dir}/{f}")).ok())
            .and_then(|bytes| serde_json::from_slice::<AlignReport>(&bytes).ok())
            .map(|r| r.one_line())
            .unwrap_or_else(|| "(no report found)".to_string());
        println!("{seg_name}: {verdict}");
    }

    println!();
    if missing.is_empty() {
        println!("{} segments total, all synthesized", sidecar.segments.len());
    } else {
        println!(
            "{} segments total, {} missing: {}",
            sidecar.segments.len(),
            missing.len(),
            missing.join(", ")
        );
    }
    Ok(())
}

/// Returns segment names (e.g. "authorship_seg05") from
/// `<basedir>/<name>.segments.json` that don't yet have a generated audio
/// file on disk — either never synthesized, or synthesized then deleted.
pub fn missing_segments(name: &str, basedir: Option<&str>) -> Result<Vec<String>> {
    let seg_dir = resolve_basedir(basedir);
    let sidecar_path = format!("{seg_dir}/{name}.segments.json");
    let sidecar_json = std::fs::read_to_string(&sidecar_path)
        .with_context(|| format!("reading {sidecar_path}"))?;
    let sidecar: Sidecar = serde_json::from_str(&sidecar_json)
        .with_context(|| format!("parsing {sidecar_path}"))?;

    Ok(sidecar.segments.iter()
        .filter(|seg| {
            !seg.files.audio.as_deref()
                .is_some_and(|f| std::path::Path::new(&format!("{seg_dir}/{f}")).exists())
        })
        .map(|seg| format!("{name}_seg{:02}", seg.index))
        .collect())
}

/// Regenerate `<basedir>/<name>.segments.json` from whatever
/// `<name>_segNN.txt` files currently exist on disk, instead of from
/// `segment`'s original split. Lets you hand-edit segment files (e.g. to
/// test a segmenter fix by re-splitting paragraphs differently) and get a
/// sidecar that matches reality, without re-running `segment` — which would
/// also discard every segment's recorded synthesis output.
///
/// Preserves each segment's `files.audio`/`normalized`/`transcript`/
/// `report` from the existing sidecar by index — only the sentence
/// structure is recomputed. Segments you didn't touch keep their recorded
/// output; segments you changed will look "done" with a stale audio/report
/// pairing until you re-run `synthesize` for them (use `summary` to see
/// what's missing first, but staleness from edits isn't tracked).
///
/// If there's no existing sidecar (e.g. it was deleted), each segment's
/// files are instead recovered by checking disk for the standard
/// `synthesize`/`record_synthesis` output names
/// (`{seg_name}_normalized.txt`, `_generated.wav`, `_transcript.json`,
/// `_report.json`), `source_sha256` is computed from `odoru/data/<name>.txt`
/// (same source location and hash `segment`'s `run` uses), and `voice_id`
/// falls back to the `voice_id` argument.
pub fn segments_from_files(name: &str, basedir: Option<&str>, voice_id: Option<&str>) -> Result<()> {
    let seg_dir = resolve_basedir(basedir);

    let existing: Option<Sidecar> = std::fs::read_to_string(format!("{seg_dir}/{name}.segments.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let existing_files: std::collections::HashMap<u32, SidecarFiles> = existing
        .as_ref()
        .map(|s| {
            s.segments
                .iter()
                .map(|seg| {
                    (
                        seg.index,
                        SidecarFiles {
                            original: seg.files.original.clone(),
                            normalized: seg.files.normalized.clone(),
                            audio: seg.files.audio.clone(),
                            transcript: seg.files.transcript.clone(),
                            report: seg.files.report.clone(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let voice_id = existing
        .as_ref()
        .and_then(|s| s.voice_id.clone())
        .or_else(|| voice_id.map(str::to_string));

    // Needed for canonical sentence splitting below regardless of whether
    // source_sha256 can be reused from an existing sidecar.
    let input_path = format!("{seg_dir}/{name}.txt");
    let source_text = std::fs::read_to_string(&input_path)
        .with_context(|| format!("reading {input_path}"))?;
    let source_sha256 = existing
        .as_ref()
        .map(|s| s.source_sha256.clone())
        .unwrap_or_else(|| sha256_hex(&source_text));

    let prefix = format!("{name}_seg");
    let mut indices: Vec<u32> = std::fs::read_dir(&seg_dir)
        .with_context(|| format!("reading dir {seg_dir}"))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_str()?;
            let stem = file_name.strip_suffix(".txt")?;
            let rest = stem.strip_prefix(&prefix)?;
            rest.parse::<u32>().ok()
        })
        .collect();
    indices.sort_unstable();

    if indices.is_empty() {
        anyhow::bail!("no {prefix}NN.txt files found in {seg_dir}");
    }

    let indexed: Vec<(u32, Vec<String>, SidecarFiles)> = indices
        .into_iter()
        .map(|index| {
            let seg_name = format!("{name}_seg{index:02}");
            let seg_path = format!("{seg_dir}/{seg_name}.txt");
            let content = std::fs::read_to_string(&seg_path)
                .with_context(|| format!("reading {seg_path}"))?;
            let paragraphs: Vec<String> = content
                .lines()
                .filter_map(|line| line.strip_prefix("Speaker 1: "))
                .map(|s| s.to_string())
                .filter(|s| !s.trim().is_empty())
                .collect();
            let files = existing_files.get(&index).cloned().unwrap_or_else(|| {
                let probe = |suffix: &str| {
                    let path = format!("{seg_dir}/{seg_name}{suffix}");
                    std::path::Path::new(&path).is_file().then(|| format!("{seg_name}{suffix}"))
                };
                SidecarFiles {
                    original: Some(format!("{seg_name}.txt")),
                    normalized: probe("_normalized.txt"),
                    audio: probe("_generated.wav"),
                    transcript: probe("_transcript.json"),
                    report: probe("_report.json"),
                }
            });
            Ok((index, paragraphs, files))
        })
        .collect::<Result<_>>()?;

    let canonical = util::splitter::split(&source_text);
    let all_paragraphs: Vec<Vec<String>> = indexed.iter().map(|(_, p, _)| p.clone()).collect();
    let mut sentences_by_segment = assign_canonical_sentences(&canonical, &all_paragraphs);

    let segments: Vec<SidecarSegment> = indexed
        .into_iter()
        .enumerate()
        .map(|(i, (index, _, files))| SidecarSegment {
            index,
            sentences: std::mem::take(&mut sentences_by_segment[i]),
            files,
        })
        .collect();

    let sidecar = Sidecar {
        schema_version: SCHEMA_VERSION.to_string(),
        source_document: format!("{name}.txt"),
        source_sha256,
        voice_id,
        segments,
    };

    let sidecar_path = format!("{seg_dir}/{name}.segments.json");
    let count = sidecar.segments.len();
    let sidecar_json = serde_json::to_string_pretty(&sidecar).context("serializing sidecar")?;
    std::fs::write(&sidecar_path, sidecar_json + "\n")
        .with_context(|| format!("writing {sidecar_path}"))?;
    info!("wrote {sidecar_path} from {count} segment file(s) found in {seg_dir}");

    Ok(())
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
    fn assign_canonical_sentences_splits_across_segments() {
        let canonical = util::splitter::split(
            "First sentence. Second sentence.\n\nThird sentence."
        );
        let segments = vec![
            vec!["First sentence. Second sentence.".to_string()],
            vec!["Third sentence.".to_string()],
        ];
        let by_segment = assign_canonical_sentences(&canonical, &segments);
        let texts: Vec<Vec<&str>> = by_segment
            .iter()
            .map(|sents| sents.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(texts, vec![vec!["First sentence.", "Second sentence."], vec!["Third sentence."]]);
        // Paragraph-end flags come from the canonical (original-document)
        // split, so they reflect real paragraph boundaries, not segment
        // boundaries.
        assert_eq!(by_segment[0][0].paragraph_end, false);
        assert_eq!(by_segment[0][1].paragraph_end, true);
        assert_eq!(by_segment[1][0].paragraph_end, true);
    }

    /// Regression test for the bug this function exists to fix: a heading
    /// merged into the following paragraph by `merge_fragments` must still
    /// end up as its own sentence in the sidecar, matching what
    /// `tts::splitter::split` computes from the original document text at
    /// replay time — not collapsed into the next sentence just because
    /// `merge_fragments` joined them with a space for chunking purposes.
    #[test]
    fn build_sidecar_keeps_heading_as_its_own_sentence() {
        let source = "Heading\n\nFirst real sentence. Second real sentence.";
        let segments = util::segmenter::segment_with_paragraphs(source);
        // Confirm the premise: the heading does get merged for chunking.
        assert_eq!(segments[0].len(), 1);
        assert!(segments[0][0].starts_with("Heading First real sentence."));

        let sidecar = build_sidecar("doc", source, &segments);
        let all_texts: Vec<&str> =
            sidecar.segments.iter().flat_map(|s| &s.sentences).map(|s| s.text.as_str()).collect();
        assert_eq!(all_texts, vec!["Heading", "First real sentence.", "Second real sentence."]);

        // And this must match what replay's splitter computes directly from
        // the same source text, sentence-for-sentence.
        let canonical = util::splitter::split(source);
        let canonical_texts: Vec<&str> = canonical.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(all_texts, canonical_texts);
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
