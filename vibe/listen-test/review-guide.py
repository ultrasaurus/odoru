#!/usr/bin/env python3
"""Build a listening-review guide for a synthesized document.

Writes <basedir>/<basename>_with_align.txt: every segment's source text in
order, each preceded by `--- NN` (the segment number), with an `[ALIGN]`
line right after the number for any segment that wasn't clean (TRUNCATED,
low-score, or filtered words, read from `<basename>_segNN_report.json`).

Usage: review-guide.py <basedir> <basename>
  listen-test/review-guide.py vibe/data/andy/hypertext87-2026-06-26 hypertext87
"""
import json
import sys
from pathlib import Path


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(1)
    basedir, basename = Path(sys.argv[1]), sys.argv[2]

    seg_paths = sorted(basedir.glob(f"{basename}_seg*.txt"))
    out = []
    for txt_path in seg_paths:
        n = txt_path.stem[len(f"{basename}_seg"):]
        report_path = basedir / f"{basename}_seg{n}_report.json"
        text = txt_path.read_text().strip()

        header = f"--- {n}"
        if report_path.exists():
            report = json.loads(report_path.read_text())
            suspects = report.get("suspect", [])
            filtered = report.get("filtered", [])
            if suspects or filtered:
                lines = []
                truncated = [s for s in suspects if s.get("reason") == "Truncated"]
                low = [s for s in suspects if s.get("reason") == "LowScore"]
                if truncated:
                    words = " ".join(f"{s['word']}({s['score']:.2f})" for s in truncated)
                    lines.append(f"⚠ TRUNCATED — {words}")
                if low:
                    words = " ".join(f"{s['word']}({s['score']:.2f})" for s in low)
                    lines.append(f"low-score — {words}")
                if filtered:
                    words = " ".join(f["word"] for f in filtered)
                    lines.append(f"filtered — {words}")
                header += "\n[ALIGN] " + "; ".join(lines)

        out.append(header)
        out.append(text)

    out_path = basedir / f"{basename}_with_align.txt"
    out_path.write_text("\n\n".join(out) + "\n")
    print(f"wrote {out_path}")


if __name__ == "__main__":
    main()
