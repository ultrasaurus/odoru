# Performing a listen test

For iterative development, we routinely generate audio from text segments
to validate normalizer fixes, overrides, or how the TTS engine handles
different documents. This doc describes the steps — whether you're doing
it yourself or asking Claude Code to help.

## Quick single-segment test

Use this to validate a fix or test specific sections of a new document 
before running a full document.

Typical fixes in `util/src/normalizer.rs` - fixes for patterns that can't 
be addressed by overriding specific strings (see `tts_overrides.txt` at
the repo root)

1. If not already done in current shell, set up environment variables:

    ```
    source vibe/.env
    ```

2. Check whether a pod is already running:
   ```
   cargo run -- list-pods
   ```
   
   Look for vibe pods with `"desiredStatus": "RUNNING"`. If so, check with the
   team if anyone is running a job, otherwise there may be an error to 
   investigate. (Idle pods should stop on their own within 3 mins.)
   
   Once RunPod state is confirmed, start a new pod:
   ```
   cargo run -- new-pod gpu e6qma5uqam
   ```
   Note the pod ID and GPU price printed.

3. Synthesize the segment:
   ```
   cargo run -- synthesize <pod_id> --seed 71463 --gpu-price <price> segment <segment_name>
   ```
   This normalizes `vibe/data/<segment_name>.txt`, sends it to the pod,
   and saves `vibe/data/<segment_name>_generated.wav`. No manual wait
   needed — it polls until the pod is ready.

   Pass `--basedir <path>` to read/write somewhere other than
   `vibe/data/` — useful if you're keeping more than one run of the
   same segment around (e.g. comparing before/after a normalizer fix).
   If you do have more than one, there's no marker for which is
   "current" — say so explicitly each time.

4. Check the AlignReport verdict before you even listen. Synthesize also
   writes `<segment_name>_transcript.json` and `<segment_name>_report.json`,
   and logs a one-line verdict:
   - `QA <name>: clean` — no issues detected.
   - `QA <name>: N filtered word(s)` — low-confidence alignment matches,
     usually just numbers/IDs; check `_report.json`'s `filtered` list, but
     this is typically benign noise, not an audible problem.
   - `QA <name>: ⚠ TRUNCATED — ...` — `suspect` words with `"reason":
     "Truncated"` in `_report.json`; in practice this has correlated with a
     real, audible skip both times we've checked it against the wav.
   `_transcript.json` is minified (no trailing newline) — some editors flag
   it as "invalid JSON" purely on that formatting quirk; verify with
   `python3 -m json.tool` before assuming it's actually broken.

5. Listen to `vibe/data/<segment_name>_generated.wav`.

6. Note any mispronunciations — segment number, what was said vs. expected.
   Add findings to `dev/normalize-future.md` and/or `dev/quirks.md` 
   and fix as needed.

The idle watchdog stops the pod after 3 minutes of inactivity — no manual
cleanup needed.

## Full document test

Use this to validate changes across an entire document.

### 1. Generate segment files

If segment files don't exist yet or the document/segmenting has changed,
regenerate segment files. Run from `vibe/`:

```bash
cargo run -- segment authorship
```

Output: `vibe/data/authorship_seg01.txt` … `authorship_segNN.txt`

Segments are 50–250 words each, split at paragraph boundaries. Long inline
quotes (≥12 words) and parenthetical asides (≥12 words) are broken out as
their own speaker turns, giving the model cleaner synthesis units.

For other documents in `odoru/data/`, pass the stem name:
```bash
cargo run -- segment augment
```

Pass `--basedir <path>` to write segments somewhere other than
`vibe/data/` — e.g. to keep a previous run's segments/audio/transcripts
intact while testing a new normalizer change on a fresh copy. If you
have more than one run, there's no marker for which is "current" —
always say which one you mean.

### 2. Start a pod

```bash
cargo run -- new-pod gpu e6qma5uqam
```

Note the pod ID and price. Requires ≥24GB VRAM — enforced automatically.

### 3. Synthesize all segments

```bash
for seg in seg01 seg02 seg03 ...; do
  cargo run -- synthesize <pod_id> --seed 71463 --gpu-price <price> segment authorship_$seg
done
```

Each segment takes ~1–2 minutes. The pod stays alive between segments
as long as requests arrive within 3 minutes of each other.

If a segment fails with a timeout error (HTTP 524), the inference ran
longer than the proxy allows. Note which segment failed and retry it on
a fresh pod later.

### 4. Stitch segments into one file

```bash
cd vibe/data
printf "file '%s'\n" authorship_seg01_generated.wav authorship_seg02_generated.wav ... > authorship_concat_list.txt
ffmpeg -y -f concat -safe 0 -i authorship_concat_list.txt -acodec copy authorship_stitched.wav
```

### 5. Listen and record findings

Listen to `vibe/data/authorship_stitched.wav` or individual segment wavs.
For each problem: note the segment, what the text says, and what was heard.
Add findings to `dev/normalize-future.md`.

### 6. Verify logs and terminate the pod

Check that all expected `.log` and `_generated.wav` files are present in
`vibe/data/`, then terminate the pod:

```bash
cargo run -- terminate-pod <pod_id>
```

Or let the 3-minute idle watchdog do it automatically.


## When to rebuild the Docker image

The normalizer runs locally — normalizer and override changes take effect
immediately with no rebuild. Only rebuild when `vibe-service` itself changes
(e.g. watchdog fix, new endpoint). See [setup.md § Docker image build](setup.md#docker-image-build).

## Tips

- **Seed**: always pass `--seed 71463` for Sarah's voice. Omit to discover
  new seeds — check `vibe/runs.jsonl` for the seed used in each run. See
  `voices.md` for qualitative notes on other seeds.
- **Per-document overrides**: add entries to `tts_overrides.txt` (repo root) for
  document-specific pronunciation fixes. Take effect immediately, no rebuild.
- **GPU cost**: RTX A5000 (24GB) at ~$0.16/hr is cheapest and fastest.
  RTX 3090 (24GB) at ~$0.22/hr is a common fallback. `new-pod` picks the
  cheapest available automatically.
- **Don't restart stopped pods** — terminate and create a new one instead.
  Under some conditions, it's probably ok -- just good practice in testing.
  In practice, restarting a stopped pod fails outright once the host
  reallocates the GPU (`HTTP 500: not enough free GPUs`) — see
  [setup.md § Starting a pod](setup.md#starting-a-pod).
- **A single clean re-run does not confirm a fix.** Truncation/skip behavior
  has reproduced inconsistently even with identical text, seed, and GPU
  model across runs — VibeVoice's output isn't fully deterministic given a
  fixed seed. Before crediting a normalizer/override change with fixing a
  skip, do several repeat runs with and without the change; one before/after
  pair isn't a reliable signal.
