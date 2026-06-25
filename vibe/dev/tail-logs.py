#!/usr/bin/env python3
"""Reassemble and unescape `text_payload` values from `gcloud beta logging
tail`'s default YAML stream, filtered to the events relevant to concurrency
testing (see dev/parallel.md). YAML wraps long quoted scalars across lines
and escapes ANSI codes (\\e[32m) as literal text — this undoes both so the
terminal renders real color and each event prints as one line.

Usage:
    gcloud beta logging tail '...' | python3 vibe/dev/tail-logs.py
"""
import re
import sys

KEYWORDS = re.compile(
    r"job created|job running|job done|alignment starting|alignment done|"
    r"still running|gpu_mem"
)

buf = None  # accumulated raw (still-escaped) value, or None if not in a text_payload block


def flush(raw):
    if raw is None:
        return
    if raw.endswith('"'):
        raw = raw[:-1]
    val = (
        raw.replace("\\e", "\x1b")
        .replace('\\"', '"')
        .replace("\\ ", " ")  # YAML line-wrap escape for a literal space
        .replace("\\\\", "\\")
    )
    if KEYWORDS.search(val):
        print(val, flush=True)


for line in sys.stdin:
    line = line.rstrip("\n")
    if buf is None:
        m = re.match(r'^text_payload: "(.*)$', line)
        if m:
            buf = m.group(1)
        continue

    cont = line.lstrip()
    # YAML double-quoted scalar: a trailing single backslash means the value
    # continues on the next line; strip it and append the next line's content.
    if buf.endswith("\\") and not buf.endswith("\\\\"):
        buf = buf[:-1] + cont
    else:
        buf += cont

    if cont.endswith("\\") and not cont.endswith("\\\\"):
        continue  # more continuation lines coming

    flush(buf)
    buf = None
