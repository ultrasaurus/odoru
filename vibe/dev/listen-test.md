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
   cargo run -- synthesize <segment_name> <pod_id> --seed 71463 --gpu-price <price>
   ```
   This normalizes `vibe/data/<segment_name>.txt`, sends it to the pod,
   and saves `vibe/data/<segment_name>_generated.wav`. No manual wait
   needed — it polls until the pod is ready.

4. Listen to `vibe/data/<segment_name>_generated.wav`.

5. Note any mispronunciations — segment number, what was said vs. expected.
   Add findings to `dev/normalize-future.md` and/or `dev/quirks.md` 
   and fix as needed.

The idle watchdog stops the pod after 3 minutes of inactivity — no manual
cleanup needed.

## Full document test

Use this to validate changes across an entire document.

### 1. Generate segment files

If segment files don't exist yet or the document/segmenting has changed, 
regenerate segment files. Run from the `vibe/` directory:

For `authorship.txt` (all 150–250 word segments, seg01–33):
```bash
python3 split_authorship_all.py
```
Output: `vibe/data/authorship_seg01.txt` … `authorship_seg33.txt`

For `augment` segments, use `split_augment.py`.

### 2. Start a pod

```bash
cargo run -- new-pod gpu e6qma5uqam
```

Note the pod ID and price. Requires ≥24GB VRAM — enforced automatically.

### 3. Synthesize all segments

```bash
for seg in seg01 seg02 seg03 ...; do
  cargo run -- synthesize authorship_$seg <pod_id> --seed 71463 --gpu-price <price>
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
