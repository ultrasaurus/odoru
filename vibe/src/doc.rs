//! `vibe synthesize doc` — orchestrates a whole-document run: export the
//! doc's text from Odoru, segment it, synthesize all segments as one batch,
//! then import the result back into Odoru.
//!
//! Shells out to the `dl` CLI (in `../cli`) for export/import, via `cargo
//! run` rather than an installed binary, since `dl` doesn't yet expose a
//! library to depend on directly.

use anyhow::{Context, Result};
use tracing::info;
use util::segment_types::Sidecar;

use crate::{runpod, segment, synth, voice};

const DL_MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cli/Cargo.toml");

/// Called from `synth::run`'s `SynthInput::Doc` arm, after voice/param
/// resolution has already happened there — so this takes resolved values
/// (not raw `--voice`/`--cfg-scale`/etc.) and calls
/// [`synth::run_segments_batch`] directly rather than `synth::run`, to
/// avoid recursing back through `run`'s `SynthInput` dispatch.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &runpod::Client,
    basename: String,
    doc_id: String,
    voice_def: Option<&voice::VibeVoiceDef>,
    pod_id: Option<String>,
    url: Option<String>,
    speaker: String,
    cfg_scale: f64,
    temp: Option<f64>,
    speed: Option<f64>,
    seed: u64,
    gpu_price: Option<f64>,
    port: u16,
    basedir: Option<String>,
) -> Result<()> {
    let data_dir = segment::resolve_basedir(basedir.as_deref());
    std::fs::create_dir_all(&data_dir).with_context(|| format!("creating {data_dir}"))?;

    let txt_path = format!("{data_dir}/{basename}.txt");
    if std::path::Path::new(&txt_path).exists() {
        anyhow::bail!(
            "{txt_path} already exists — `doc` expects a fresh basedir; \
             remove it or pass a different --basedir to re-run"
        );
    }

    info!("exporting doc {doc_id} -> {txt_path}");
    dl(&["export", &doc_id, &txt_path])?;

    segment::run(&basename, Some(data_dir.as_str()))?;

    let sidecar_path = format!("{data_dir}/{basename}.segments.json");
    let sidecar: Sidecar = serde_json::from_str(
        &std::fs::read_to_string(&sidecar_path).with_context(|| format!("reading {sidecar_path}"))?,
    )
    .with_context(|| format!("parsing {sidecar_path}"))?;
    let segment_count = sidecar.segments.len();
    anyhow::ensure!(segment_count > 0, "segmenting {basename} produced no segments");
    let spec = format!("{basename}_seg01-{segment_count:02}");

    synth::run_segments_batch(
        client, voice_def, spec, pod_id, url, speaker, cfg_scale, temp, speed, seed, gpu_price,
        port, basedir,
    )
    .await?;

    info!("importing {data_dir} into odoru doc {doc_id}");
    dl(&["import", "vibe", &data_dir])?;

    Ok(())
}

/// Run a `dl` subcommand via `cargo run` against the sibling `cli` crate.
fn dl(args: &[&str]) -> Result<()> {
    let mut argv: Vec<String> = vec![
        "cargo".to_string(),
        "run".to_string(),
        "--quiet".to_string(),
        "--manifest-path".to_string(),
        DL_MANIFEST_PATH.to_string(),
        "--bin".to_string(),
        "dl".to_string(),
        "--".to_string(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    crate::run(&argv)
}
