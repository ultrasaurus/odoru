## Playback with Word-level selection of text and audio
Odoru currently uses TTS exlusively, which involves a "normalization" step
which handles pronounciation of names, abbreviations, etc. 

### Audio cache format

The audio cache lives in `~/.odoru/audio/`. Each cached sentence is stored as
two sibling files keyed by `SHA-256(normalized_text + "|" + voice_cache_key)`:

- `{key}.mp3` — the encoded audio
- `{key}.json` — metadata sidecar:
  ```json
  { "text": "the normalized sentence text", "duration": 4.14, "invalid": false }
  ```
  `invalid: true` marks a failed synthesis that should not be replayed.

### Word-level timestamps

Transclusion playback requires seeking into a sentence at an arbitrary word
boundary — the `sentence:char` offsets in `refs.json` resolve to a word index,
and that word index must map to an audio timestamp.

The forced-alignment crate (more detail below) produces per-word
`start`/`end` timestamps given audio and its ground-truth text. For TTS
audio the ground truth is always the normalized text stored in the `.json`
sidecar.

Word timestamps will be stored as an additional field in the `.json` sidecar:

```json
{
  "text": "the normalized sentence text",
  "duration": 4.14,
  "invalid": false,
  "words": [
    { "word": "the", "start": 0.10, "end": 0.18 },
    { "word": "normalized", "start": 0.22, "end": 0.61 },
    ...
  ]
}
```

Timestamps are generated lazily — on first access for transclusion — and
cached back to the sidecar. Documents that are never transcluded never incur
the alignment cost.

### Normalization mapping

The normalized text sent to TTS may differ from the original markdown text
(e.g. "ARPANET" → "arpa net", "1945" → "nineteen forty five"). The `sentence:char`
offsets in `refs.json` reference the **original plain-text sentence**, so
resolving a transclusion anchor requires a two-step mapping:

1. Map original char offset → normalized word index (using the normalization
   rules applied at synthesis time)
2. Map normalized word index → audio timestamp (from the `words` array above)

This mapping is computed at playback time from the stored normalized text and
the original sentence text.


## forced-alignment crate
In `../forced-alignment`, this pure rust crate needs to be modified so that:
* data structures match corresponding data structures in Odoru
  (see `tts/src/transcript.rs`)
* create lib with an API that is easy to use for Odoru use case


### Future
* imported human-read-aloud audio will also be supported, which can sometimes
  include words not in the text that make it more listenable (e.g. Section 1,
  where the text shows "1").