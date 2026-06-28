/**
 * reader-util.ts — small helpers shared across the live reader (ui.ts,
 * edit.ts, reader-author.ts) and the static SPA export (reader-export.ts).
 */

/** Formats a duration in seconds as `m:ss` (e.g. 65 -> "1:05"). Used for
 *  playback time displays (current position, total length) — not to be
 *  confused with edit.ts's fmtDuration, which formats an approximate
 *  synthesis-time *estimate* like "~2m 30s" and is a different format for a
 *  different purpose. */
export function fmt(s: number): string {
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${sec.toString().padStart(2, '0')}`
}
