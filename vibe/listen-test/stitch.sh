#!/usr/bin/env bash
# Stitch segment wavs into one file with a fade in/out on each segment and
# silence gaps between them, to avoid the abrupt "pop" of a hard concat.
#
# Usage: dev/stitch.sh <basedir> <basename> [fade_secs] [gap_secs]
#   dev/stitch.sh vibe/data/andy/hypertext87-2026-06-22 hypertext87 0.15 0.8
#
# Reads <basedir>/<basename>_segNN_generated.wav (in name-sorted order),
# writes <basedir>/<basename>_stitched.wav.

set -euo pipefail

basedir=$1
basename=$2
fade=${3:-0.15}
gap=${4:-0.8}

cd "$basedir"

sample_rate=$(ffprobe -v error -show_entries stream=sample_rate \
  -of default=noprint_wrappers=1:nokey=1 "${basename}_seg01_generated.wav")

silence="${basename}_silence_tmp.wav"
ffmpeg -y -f lavfi -i "anullsrc=channel_layout=mono:sample_rate=${sample_rate}" \
  -t "$gap" -acodec pcm_s16le "$silence" -loglevel error

concat_list="${basename}_concat_list_tmp.txt"
> "$concat_list"

for f in ${basename}_seg*_generated.wav; do
  dur=$(ffprobe -v error -show_entries format=duration \
    -of default=noprint_wrappers=1:nokey=1 "$f")
  fade_start=$(python3 -c "print(max(0, $dur - $fade))")
  faded="${f%_generated.wav}_faded_tmp.wav"
  ffmpeg -y -i "$f" \
    -af "afade=t=in:st=0:d=${fade},afade=t=out:st=${fade_start}:d=${fade}" \
    "$faded" -loglevel error
  echo "file '$faded'" >> "$concat_list"
  echo "file '$silence'" >> "$concat_list"
done

# drop the trailing silence entry after the last segment
sed -i '' '$ d' "$concat_list"

ffmpeg -y -f concat -safe 0 -i "$concat_list" -acodec copy \
  "${basename}_stitched.wav" -loglevel error

rm -f "$concat_list" "$silence" ${basename}_seg*_faded_tmp.wav

echo "wrote ${basedir}/${basename}_stitched.wav"
