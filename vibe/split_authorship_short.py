#!/usr/bin/env python3
"""Create 5 shorter segments (150-250 words) from authorship.txt, starting
from the 'User-Specified Content Filters' paragraph, saved as seg12-seg16."""

START_MARKER = "User-Specified Content Filters"
NUM_SEGS = 5
MIN, MAX = 150, 250

with open('../data/authorship.txt') as f:
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

# Find start paragraph
start_idx = next(i for i, p in enumerate(paragraphs) if START_MARKER in p)
body = paragraphs[start_idx:]

# Find References section — don't include it
ref_idx = next((i for i, p in enumerate(body) if p.strip() == 'References'), None)
if ref_idx is not None:
    body = body[:ref_idx]

def ends_sentence(p):
    return p.rstrip().endswith(('.', '?', '!'))

# Merge headings/short fragments into following paragraph
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

# Use only the first NUM_SEGS
segments = segments[:NUM_SEGS]

# Write output files
for i, seg in enumerate(segments, 12):
    name = f'authorship_seg{i:02d}'
    path = f'data/{name}.txt'
    with open(path, 'w') as f:
        for p in seg:
            f.write(f'Speaker 1: {p}\n')
    wc = sum(word_count(p) for p in seg)
    print(f'{name}: {len(seg)} paragraphs, {wc} words')
    # Show start of first paragraph
    print(f'  starts: {seg[0][:80]}...')
