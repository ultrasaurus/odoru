#!/usr/bin/env bash
# Flags segments where VibeVoice has regurgitated the voice-clone reference
# clip's own transcript instead of (or in addition to) the intended text —
# a real artifact that the normal per-segment forced-alignment QA (run
# server-side against each segment's *own* text) does not reliably catch:
# a short/partial leak can pass that check cleanly. See
# dev/artifact-hypertext87.md and the seg06/seg14 investigation.
#
# Detection: align just the leading N seconds of each segment's audio
# against the reference clip's own transcript (not the segment's intended
# text). A clean/confident alignment there means the reference clip's
# words are audible at the start of the segment, which they should never
# be — confirmed on a known-bad sample: 2 of 10 reference-clip words came
# back low-score, vs 9 of 9 when aligning the same audio against the
# segment's own (correct) text.
#
# Runs up to --jobs segments in parallel (each forced-alignment run is
# single-segment CPU work; on an 8-core machine, -j 8 is a reasonable
# default) — useful for QA-ing a full batch resynth (e.g. after switching
# seeds) without waiting on Cloud Run's QA alone, which has this same
# blind spot.
#
# Usage:
#   check-ref-leak.sh <basedir> <name> <ref_clip_text_file> [duration] [jobs]
#
# Example:
#   vibe/listen-test/check-ref-leak.sh \
#     vibe/data/andy/hypertext87-2026-06-27 hypertext87 \
#     voices/andy/ref_clip_text.txt 8 8
#
# Requires: ffmpeg, and FORCED_ALIGNMENT_BIN pointing at a built
# forced-alignment-rs release binary (default assumes the sibling repo at
# ~/src/claude/forced-alignment).

set -euo pipefail

basedir=$1
name=$2
ref_text=$3
duration=${4:-8}
jobs=${5:-8}

fa_bin="${FORCED_ALIGNMENT_BIN:-$HOME/src/claude/forced-alignment/target/release/forced-alignment}"
if [[ ! -x "$fa_bin" ]]; then
  echo "forced-alignment binary not found/executable at $fa_bin (set FORCED_ALIGNMENT_BIN)" >&2
  exit 1
fi

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

check_one() {
  local wav="$1"
  local seg
  seg=$(basename "$wav" "_generated.wav")
  local slice="$work_dir/${seg}_lead.wav"
  local out="$work_dir/${seg}_refcheck.json"

  ffmpeg -y -loglevel error -i "$wav" -t "$duration" -acodec pcm_s16le "$slice"
  local result
  result=$("$fa_bin" "$slice" "$ref_text" -o "$out" 2>&1) || true

  local total suspect
  total=$(python3 -c "import json; d=json.load(open('$out')); print(sum(len(s['words']) for s in d['segments']))" 2>/dev/null || echo "?")
  suspect=$(echo "$result" | grep -c "suspect \[" || echo "0")

  echo "$seg: $suspect/$total words suspect against ref-clip text"
}
export -f check_one
export work_dir duration ref_text fa_bin

find "$basedir" -maxdepth 1 -name "${name}_seg*_generated.wav" | sort \
  | xargs -P "$jobs" -I{} bash -c 'check_one "$@"' _ {}
