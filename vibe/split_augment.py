#!/usr/bin/env python3
"""Split augment.txt (Section I.A up to B. OBJECTIVE) into 150-250 word
segments saved as vibe/data/augment_seg01.txt, augment_seg02.txt, ..."""

import sys

STOP_MARKER = "B. OBJECTIVE OF THE STUDY"
MIN, MAX = 150, 250

with open('../data/augment.txt') as f:
    lines = [l.rstrip('\n') for l in f]

# Group into paragraphs (separated by blank lines)
paragraphs = []
current = []
for line in lines:
    if line.strip() == '':
        if current:
            paragraphs.append(' '.join(current))
            current = []
    else:
        current.append(line.strip())
if current:
    paragraphs.append(' '.join(current))

# Take everything before the stop marker
stop_idx = next((i for i, p in enumerate(paragraphs) if STOP_MARKER in p), None)
if stop_idx is None:
    sys.exit(f'ERROR: stop marker not found: {STOP_MARKER!r}')
body = paragraphs[:stop_idx]

def ends_sentence(p):
    return p.rstrip().endswith(('.', '?', '!'))

# Merge headings/short fragments into the following paragraph
merged = []
i = 0
while i < len(body):
    p = body[i]
    while not ends_sentence(p) and i + 1 < len(body):
        i += 1
        p = p + ' ' + body[i]
    merged.append(p)
    i += 1
body = merged

def word_count(p):
    return len(p.split())

# Greedily accumulate into segments of 150-250 words
segments = []
current_seg = []
current_wc = 0

for p in body:
    pw = word_count(p)
    if current_wc + pw > MAX and current_wc >= MIN:
        segments.append(current_seg)
        current_seg = [p]
        current_wc = pw
    else:
        current_seg.append(p)
        current_wc += pw

if current_seg:
    segments.append(current_seg)

# Write output files
for i, seg in enumerate(segments, 1):
    name = f'augment_seg{i:02d}'
    path = f'data/{name}.txt'
    with open(path, 'w') as f:
        for p in seg:
            f.write(f'Speaker 1: {p}\n')
    wc = sum(word_count(p) for p in seg)
    print(f'{name}: {len(seg)} paragraphs, {wc} words')
    print(f'  starts: {seg[0][:80]}...')
