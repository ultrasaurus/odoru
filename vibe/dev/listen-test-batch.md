# Performing a listen test — batch client (Cloud Run)

Companion to [listen-test.md](listen-test.md), for the new `synthesize
... segments <spec>` batch path (`dev/parallel.md`) instead of the
per-segment `segment <name>` loop. One client call submits N segments
together as a single `POST /batches`/`generate()` call on the server,
instead of N separate `POST /jobs` round trips.

**Not literally "render a whole doc" in one shot.** `synthesize ...
doc <name>` is still an unimplemented stub — you pick an explicit list
or range of already-segmented files yourself (e.g. `seg72-120`), not
"the whole document" as a single concept. In practice this still lets
you render a large chunk of a document in one client invocation; just
know the boundary is whatever range you type, not document metadata.

Currently only exercised against Cloud Run (Blackwell) — the active
synth target during the 90-day eval. The server-side batching code is
host-agnostic by design, but RunPod hasn't been tried yet; if you do,
note what happened here or in `dev/parallel.md`.

## 1. Set up environment variables

```bash
source vibe/.env
```

`$VIBE_BW_URL` should be set here to the deployed Cloud Run Blackwell
service URL.

## 2. Check the instance is healthy

No pod to start — Cloud Run is serverless, so this just confirms the
deployed revision is up and the GPU is visible:

```bash
curl -sS "$VIBE_BW_URL/health"
```

```json
{"gpu":"NVIDIA RTX PRO 6000 Blackwell Server Edition, 97887 MiB","status":"ready"}
```

If it's been idle a while, the first request after a cold scale-up
takes ~55-60s before responding (see `dev/parallel.md` "Measuring
subprocess-start warmup cost") — that's normal, not a hang.

## 3. Upload the voice

Voices uploaded via `upload-voice` don't persist across redeploys or
fresh instances — re-upload each time you start a session against a
new deploy:

```bash
cargo run -- upload-voice --name Sarah --gender woman \
  --wav-path ../voices/sarah/ref.wav --url "$VIBE_BW_URL"
```

## 4. Make sure the segment files exist

Same as the single-segment path — if the range you want to render
hasn't been segmented yet, run `segment` first (see listen-test.md §
"Full document test" step 1). This doc assumes `.txt` segment files
already exist under `--basedir`.

To find what's actually missing before picking a range:

```bash
cargo run -- summary <basename> --basedir <path>
```

## 5. Submit the batch

```bash
cargo run -- synthesize --speaker Sarah --seed 71463 \
  --url "$VIBE_BW_URL" --basedir <path> \
  segments <basename>_seg72-90
```

`<spec>` after `segments` accepts:
- A range: `augment_seg72-90` (zero-padded to match however you type
  the start number, e.g. `seg08-10` pads to 2 digits).
- A comma list: `augment_seg72,augment_seg74,augment_seg90`.
- A mix of both, comma-separated.

One shared `--seed`/`--speaker`/`--cfg-scale`/`--temp`/`--speed` apply
to every segment in the batch — there's no per-segment override (see
`dev/parallel.md` "Stage 3 implementation plan").

What happens, in order:
1. Each segment's `.txt` is normalized locally (same as the
   single-segment path) and the whole set goes up in one `POST
   /batches` call.
2. The client polls `GET /batches/:id` once (not once per segment —
   all segments in a batch share fate, since they're one `generate()`
   call) until the batch reports `done` or `error`.
3. If the batch_id isn't found on a later poll (e.g. instance churn —
   the batch grouping is in-memory only, see `dev/parallel.md`), the
   client falls back to polling each segment's job_id individually —
   you don't lose anything, it just degrades to per-segment polling.
4. For each segment, the client fetches wav/transcript/report and
   updates `runs.jsonl` (now carrying a `batch_id` alongside the usual
   `job_id`) and the sidecar, exactly like the single-segment path.

A batch of ~15-20 typically-sized segments takes on the order of a
minute or two end to end, not 15-20x a single segment's time — that's
the whole point of batching (see the throughput tables in
`dev/cloudrun/cloudrun-blackwell.md`).

## 6. Check the AlignReport verdict before listening

Same as the single-segment path — per-segment `QA <name>: ...` lines
print for each segment in the batch:
- `clean` — no issues.
- `N filtered word(s)` — low-confidence matches, usually benign
  (numbers/IDs); check `_report.json`.
- `⚠ TRUNCATED — ...` — has correlated with an audible skip both times
  checked.

## 7. Listen and stitch

Same as listen-test.md §4-5 — `_generated.wav` files land with the
same naming convention (`<basename>_segNN_generated.wav`), so
`listen-test/stitch.sh` and the manual concat approach both work unchanged on
a batch-rendered range.

## 8. If something fails partway through a batch

A whole-batch failure (the subprocess itself crashed) errors every
job_id in that batch together. A single segment with no produced wav
(rare — partial failure within an otherwise-successful batch) only
errors that one job_id; the rest of the batch's results still land
normally. Either way, `cargo run -- summary <basename> --basedir
<path>` shows exactly which segments are still missing — feed those
into a follow-up `segments <spec>` call (or fall back to individual
`segment <name>` calls) rather than re-running the whole range.

## Tips

- Same seed/cost/restart guidance as listen-test.md applies — this
  doc only covers what's different about the batch path.
- Picking a sane batch size: `dev/cloudrun/cloudrun-blackwell.md` has
  throughput/VRAM data up to N=32 with no sign of a ceiling yet, so
  there's no currently-known reason to keep batches small — but a
  contiguous range from one in-progress doc, not a huge cross-document
  grab, keeps a partial failure easy to re-run.
- Cloud Run instances scale to zero — if you've been idle a while
  before step 5, expect the cold-start delay described in step 2.
