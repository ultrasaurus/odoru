#!/usr/bin/env python3
"""Split all of authorship.txt into 150-250 word segments at paragraph
boundaries, starting at seg01. References become the final segment."""

MIN, MAX = 150, 250

with open('../data/authorship.txt') as f:
    lines = [l.rstrip('\n') for l in f]

# Group into paragraphs
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

# Split off References as its own final segment
ref_idx = next((i for i, p in enumerate(paragraphs) if p.strip() == 'References'), None)
body = paragraphs[:ref_idx] if ref_idx is not None else paragraphs
refs = paragraphs[ref_idx:] if ref_idx is not None else []

def ends_sentence(p):
    return p.rstrip().endswith(('.', '?', '!'))

def word_count(p):
    return len(p.split())

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

# Greedily accumulate into 150-250 word segments
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
    print(f'  starts: {seg[0][:80]}...')
