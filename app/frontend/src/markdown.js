import { marked } from 'marked';
// ---------------------------------------------------------------------------
// Abbreviation protection — mirrors server-side splitter.rs ABBREVS list.
// Replaces the trailing period of each abbreviation with a placeholder so
// the sentence segmenter doesn't treat it as a sentence boundary.
// ---------------------------------------------------------------------------
const PLACEHOLDER = '￾';
const ABBREVS = [
    // Titles
    'Mr', 'Mrs', 'Ms', 'Miss', 'Dr', 'Prof', 'Rev', 'Sr', 'Jr',
    // Geographic
    'St', 'Ave', 'Blvd', 'Rd', 'Mt', 'Dept',
    // Latin
    'vs', 'etc', 'e.g', 'i.e', 'et al',
    // Months
    'Jan', 'Feb', 'Mar', 'Apr', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
    // Corporate
    'Corp', 'Inc', 'Ltd', 'Est',
];
function protectAbbrevs(text) {
    let out = text;
    for (const abbrev of ABBREVS) {
        out = out.replaceAll(`${abbrev}.`, `${abbrev}${PLACEHOLDER}`);
    }
    return out;
}
function restorePlaceholders(text) {
    return text.replaceAll(PLACEHOLDER, '.');
}
// ---------------------------------------------------------------------------
// Strip inline markdown markers to get plain text for sentence splitting.
// Only needs to handle what trafilatura produces: bold, italic, code, links.
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Silent text — bracketed spans followed by a `<!--silent-->` comment, e.g.
// `[Doug Engelbart]<!--silent-->`. Displayed (brackets kept) but excluded from
// TTS and playback. See dev/silent-text.md. Mirrors strip_silent in
// tts/src/markdown.rs.
// ---------------------------------------------------------------------------
const SILENT_SPAN = /\[[^\]]*\]\s*<!--\s*silent\s*-->/g;
// True when a whole block (heading/paragraph) is nothing but a single silent
// span — the only case handled in this first pass (mid-sentence inline silent
// is deferred).
const FULLY_SILENT = /^\s*\[[^\]]*\]\s*<!--\s*silent\s*-->\s*$/;
function isFullySilent(text) {
    return FULLY_SILENT.test(text);
}
// The bracketed display text for a silent span, with the comment stripped and
// the brackets kept (the editorial-insertion convention).
function silentDisplayText(text) {
    return text.replace(/<!--\s*silent\s*-->/g, '').trim();
}
// Outline label for a heading: bracketed display text if silent, otherwise
// the inline-stripped plain text.
function silentOrPlain(text) {
    return isFullySilent(text) ? silentDisplayText(text) : stripInline(text);
}
// Remove silent spans to derive the plain text fed to TTS. Drops a line that
// became empty (or only heading `#` markers) because of the stripping;
// preserves originally-blank lines so paragraph boundaries survive.
export function stripSilent(markdown) {
    const out = [];
    for (const line of markdown.split('\n')) {
        const removed = line.replace(SILENT_SPAN, '');
        const trimmed = removed.trim();
        if (removed !== line && (trimmed === '' || /^#+$/.test(trimmed)))
            continue;
        out.push(removed);
    }
    return out.join('\n');
}
function stripInline(text) {
    return text
        .replace(/\*\*(.*?)\*\*/gs, '$1')
        .replace(/__(.*?)__/gs, '$1')
        .replace(/\*(.*?)\*/gs, '$1')
        .replace(/_(.*?)_/gs, '$1')
        .replace(/`(.*?)`/g, '$1')
        .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1');
}
// ---------------------------------------------------------------------------
// splitLines — shared sentence splitting used for both plain-text (server
// match) and raw markdown (for inline rendering). Mirrors split_paragraph
// in splitter.rs: single newlines are hard breaks, Unicode sentence
// boundaries are found within each line, abbreviations are protected.
// ---------------------------------------------------------------------------
// Mirrors merge_outline_labels in splitter.rs.
// Merges a short all-caps label (e.g. "I.", "XIV.", "A.") with the sentence that follows.
// Lowercase Roman numeral chars only, max 4 chars (covers i–xvii).
const LOWERCASE_ROMAN_RE = /^[ivxlcdm]{1,4}$/;
function mergeOutlineLabels(sentences) {
    const isLabel = (s) => {
        const stem = s.trim().replace(/\.$/, '');
        if (!stem)
            return false;
        if (/^[A-Z0-9]+$/.test(stem)) {
            const alpha = stem.replace(/[^A-Za-z]/g, '');
            return alpha.length <= 4;
        }
        if (/^[a-z]+$/.test(stem)) {
            return LOWERCASE_ROMAN_RE.test(stem);
        }
        return false;
    };
    const out = [];
    let i = 0;
    while (i < sentences.length) {
        if (isLabel(sentences[i]) && i + 1 < sentences.length) {
            out.push(sentences[i].trimEnd() + ' ' + sentences[i + 1].trimStart());
            i += 2;
        }
        else {
            out.push(sentences[i]);
            i++;
        }
    }
    return out;
}
// Splits a markdown block's text into sentences, treating intra-block single
// newlines as soft breaks (collapsed to a space) rather than hard sentence
// breaks — mirrors the server's CommonMark handling in tts/src/markdown.rs
// (`Event::SoftBreak | Event::HardBreak => current.push(' ')`). Blocks never
// contain blank lines internally, so any `\n` here is a soft/hard break, not
// a paragraph boundary.
function splitBlockText(text) {
    return splitLines(text.replace(/\n+/g, ' '));
}
function splitLines(text) {
    const sentences = [];
    for (const line of text.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed)
            continue;
        const protected_ = protectAbbrevs(trimmed);
        if (segmenter) {
            for (const { segment } of segmenter.segment(protected_)) {
                const s = restorePlaceholders(segment.trim());
                if (s)
                    sentences.push(s);
            }
        }
        else {
            protected_.split(/(?<=[.!?])\s+/).forEach(s => {
                const r = restorePlaceholders(s.trim());
                if (r)
                    sentences.push(r);
            });
        }
    }
    return mergeOutlineLabels(sentences).filter(s => /[a-zA-Z]/.test(s));
}
export function renderMarkdown(content, plainText, container) {
    // Split plain_text into sentences — ground truth that matches the server.
    // Server splits on \n\n for paragraphs, then single \n + unicode_sentences
    // within each paragraph. Mirror that here.
    const allSentences = [];
    for (const para of plainText.split(/\n\n+/).map(p => p.trim()).filter(Boolean)) {
        allSentences.push(...splitLines(para));
    }
    const pendingSpans = [];
    const headings = [];
    let globalIdx = 0;
    const fragment = document.createDocumentFragment();
    const tokens = marked.lexer(content);
    for (const token of tokens) {
        globalIdx = renderToken(token, fragment, allSentences, globalIdx, pendingSpans, headings);
    }
    container.appendChild(fragment);
    return { pendingSpans, headings };
}
// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------
// Returns the updated globalIdx after consuming sentences for this token.
function renderToken(token, container, allSentences, globalIdx, pendingSpans, headings) {
    switch (token.type) {
        case 'heading': {
            const el = document.createElement(`h${token.depth}`);
            el.className = 'md-heading';
            const sentenceIndex = globalIdx;
            if (isFullySilent(token.text)) {
                // Display-only navigation heading: shown in body + outline, never
                // spoken. No span woven, globalIdx unchanged, so it points at the
                // next real sentence — the natural scroll target.
                el.classList.add('silent');
                el.textContent = silentDisplayText(token.text);
            }
            else {
                globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans);
            }
            container.appendChild(el);
            headings.push({ depth: token.depth, text: silentOrPlain(token.text), element: el, sentenceIndex });
            break;
        }
        case 'paragraph': {
            const el = document.createElement('p');
            el.className = 'md-paragraph';
            if (isFullySilent(token.text)) {
                el.classList.add('silent');
                el.textContent = silentDisplayText(token.text);
            }
            else {
                globalIdx = weaveSpans(token.text, el, allSentences, globalIdx, pendingSpans);
            }
            container.appendChild(el);
            break;
        }
        case 'blockquote': {
            const el = document.createElement('blockquote');
            el.className = 'md-blockquote';
            for (const child of token.tokens ?? []) {
                globalIdx = renderToken(child, el, allSentences, globalIdx, pendingSpans, headings);
            }
            container.appendChild(el);
            break;
        }
        case 'list': {
            const el = document.createElement(token.ordered ? 'ol' : 'ul');
            el.className = 'md-list';
            for (const item of token.items) {
                const li = document.createElement('li');
                li.className = 'md-list-item';
                globalIdx = weaveSpans(item.text, li, allSentences, globalIdx, pendingSpans);
                el.appendChild(li);
            }
            container.appendChild(el);
            break;
        }
        case 'code': {
            const pre = document.createElement('pre');
            pre.className = 'md-code';
            const code = document.createElement('code');
            code.textContent = token.text;
            pre.appendChild(code);
            container.appendChild(pre);
            break;
        }
        case 'hr': {
            container.appendChild(document.createElement('hr'));
            break;
        }
        case 'space':
            break;
        default: {
            const text = token.text;
            if (text?.trim()) {
                const el = document.createElement('p');
                el.className = 'md-paragraph';
                globalIdx = weaveSpans(text, el, allSentences, globalIdx, pendingSpans);
                container.appendChild(el);
            }
        }
    }
    return globalIdx;
}
// ---------------------------------------------------------------------------
// Sentence span weaving
//
// For each block, we split the raw markdown text into sentences to get the
// renderable (inline-formatted) version of each sentence, and split the
// stripped plain text to know how many sentences this block contributes.
// Those counts should match — if they do, we render spans using the raw
// markdown sentences (so bold/italic display correctly). If they diverge
// (unexpected edge case), we fall back to plain-text sentences from the
// global list so synthesis indices stay aligned.
// ---------------------------------------------------------------------------
const segmenter = typeof Intl !== 'undefined' && 'Segmenter' in Intl
    ? new Intl.Segmenter('en', { granularity: 'sentence' })
    : null;
function weaveSpans(rawText, container, allSentences, globalIdx, pendingSpans) {
    // Count sentences via stripped plain text — matches what the server sees.
    // Intra-block newlines are soft/hard breaks (collapsed to a space), not
    // sentence boundaries — see splitBlockText.
    const plainSentences = splitBlockText(stripInline(rawText));
    const count = plainSentences.length;
    // Raw markdown sentences for inline rendering. Should be the same count.
    const rawSentences = splitBlockText(rawText);
    for (let i = 0; i < count; i++) {
        const span = document.createElement('span');
        span.className = 'segment pending';
        if (rawSentences.length === count) {
            // Counts match — render with inline formatting.
            span.innerHTML = marked.parseInline(rawSentences[i]);
        }
        else {
            // Fallback — use plain text from the global list.
            span.textContent = allSentences[globalIdx] ?? plainSentences[i];
        }
        pendingSpans.push(span);
        container.appendChild(span);
        if (i < count - 1) {
            container.appendChild(document.createTextNode(' '));
        }
        globalIdx++;
    }
    return globalIdx;
}
