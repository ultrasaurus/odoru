#!/usr/bin/env python3
"""Split data/authorship.txt into 250-400 word segments at paragraph boundaries."""

import re

with open('../data/authorship.txt') as f:
    lines = [l.rstrip('\n') for l in f]

# Group into paragraph blocks (separated by blank lines)
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

# Find where References section starts
ref_idx = next((i for i, p in enumerate(paragraphs) if p.strip() == 'References'), None)

if ref_idx is not None:
    body = paragraphs[:ref_idx]
    refs = paragraphs[ref_idx:]  # includes "References" heading
else:
    body = paragraphs
    refs = []

def word_count(p):
    return len(p.split())

# Greedily accumulate into 250-400 word segments
MIN, MAX = 250, 400
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

if refs:
    segments.append(refs)

# Write output files
for i, seg in enumerate(segments, 1):
    name = f'authorship_seg{i:02d}'
    path = f'data/{name}.txt'
    with open(path, 'w') as f:
        for p in seg:
            f.write(f'Speaker 1: {p}\n')
    wc = sum(word_count(p) for p in seg)
    print(f'{name}: {len(seg)} paragraphs, {wc} words')
