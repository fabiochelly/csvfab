/* ---------------------------------------------------------------
   DIAGNOSE — "why does this row look wrong?"
   diagnoseRow(t, row) reads one record as it is in the file — its bytes,
   not only its decoded text — and says what is off and why: the field
   count against the file's dominant shape and the probable cause (an
   unquoted delimiter, located by the column types it would realign; a
   line cut by an unquoted line break; a quote never closed; fields
   missing), quote anomalies, encoding evidence (UTF-8 bytes in a
   Windows-1252 file, garbled accents, replacement characters), NUL and
   other control or invisible characters, cells that a spreadsheet would
   run as formulas. Each finding carries the bytes around it, so what is
   seen is what is in the file. A repair is proposed when one is safe and
   unambiguous enough: it is an ordinary, undoable edit.
   Pure analysis (no DOM) — the row card renders it (renderDiagnosis).
----------------------------------------------------------------*/

/* The field count most records have — the header line aside. */
function dominantWidth(t) {
    const B = t.base;
    if (!B || B.lines) return t.headers.length;
    if (B.dominant != null) return B.dominant;
    const skip = t.headerSrc === 0 ? 0 : -1, counts = new Map();
    let regular = B.n - B.odd.size;
    if (skip === 0 && !B.odd.has(0)) regular--;               // the header line is not a row
    counts.set(B.width, regular);
    for (const [b, w] of B.odd) if (!(skip === 0 && b === 0)) counts.set(w, (counts.get(w) || 0) + 1);
    let best = B.width, bn = -1;
    for (const [w, k] of counts) if (k > bn || (k === bn && w === t.headers.length)) { best = w; bn = k; }
    return B.dominant = best;
}

/* The whole file's bytes (not only the decoded copy): where offsets are read. */
function fileBytes(B) { return B.orig || new Uint8Array(B.u8.buffer, 0, B.u8.byteOffset + B.u8.length); }

/* Record b in the file: its byte range (line breaks after it excluded), and how a
   character offset in its text maps to a byte offset there. */
function recordSpan(B, b) {
    if (B.orig) {                                             // UTF-16: 2 bytes a unit, from the record's unit offset
        const u = B.chars || B.starts, s = B.origAt + 2 * u[b];
        let e = B.origAt + 2 * u[b + 1];
        const f = B.orig, be = B.enc === 'utf-16be';
        const brk = i => { const lo = f[be ? i + 1 : i], hi = f[be ? i : i + 1]; return hi === 0 && (lo === 10 || lo === 13); };
        while (e - 2 >= s && brk(e - 2)) e -= 2;
        return { s, e, at: i => s + 2 * i };
    }
    const off = B.u8.byteOffset, s0 = B.starts[b];
    let e0 = B.starts[b + 1];
    while (e0 > s0 && (B.u8[e0 - 1] === 10 || B.u8[e0 - 1] === 13)) e0--;
    const text = recordText(B, b);
    const at = B.enc === 'utf-8' ? i => off + s0 + new TextEncoder().encode(text.slice(0, i)).length : i => off + s0 + i;
    return { s: off + s0, e: off + e0, at };
}

/* The bytes around an offset, as hex and as text: what is really in the file there. */
function bytesAround(B, at, len = 1, radius = 8) {
    const f = fileBytes(B), a = Math.max(0, at - radius), z = Math.min(f.length, at + len + radius);
    const hex = [];
    for (let i = a; i < z; i++) hex.push({ h: f[i].toString(16).toUpperCase().padStart(2, '0'), hit: i >= at && i < at + len });
    const a2 = B.orig ? a - ((a - B.origAt) & 1) : a;         // UTF-16: from a unit's first byte
    const text = new TextDecoder(B.enc).decode(f.subarray(a2, z));
    return { at, hex, text, from: a };
}

/* A record's fields with their places in its text, by the scanner's rules
   (splitRecord), and what is odd about their quotes. */
function fieldSpans(s, D) {
    const out = [], L = s.length;
    let i = 0;
    for (;;) {
        const f = { start: i, quoted: false, issues: [] };
        let k0 = i;
        while (k0 < L && s[k0] === ' ' && D !== ' ') k0++;
        if (s.charCodeAt(i) === 34) {
            f.quoted = true;
            let j = i + 1, val = '';
            for (;;) {
                const k = s.indexOf('"', j);
                if (k < 0) { val += s.slice(j); f.issues.push({ kind: 'unclosed', at: i }); i = L; break; }
                if (s.charCodeAt(k + 1) === 34) { val += s.slice(j, k + 1); j = k + 2; continue; }
                let m = k + 1;
                while (m < L && (s[m] === ' ' || (s[m] === '\t' && D !== '\t'))) m++;
                if (m >= L || s[m] === D) { val += s.slice(j, k); i = m; break; }
                if (!f.issues.some(x => x.kind === 'after-quote')) f.issues.push({ kind: 'after-quote', at: k + 1 });
                val += s.slice(j, k + 1); j = k + 1;
            }
            f.value = val;
        } else {
            const k = s.indexOf(D, i), end = k < 0 ? L : k;
            f.value = s.slice(i, end);
            const q = f.value.indexOf('"');
            if (q >= 0) f.issues.push({ kind: k0 < end && s[k0] === '"' && k0 > i ? 'space-before-quote' : 'quote-inside', at: i + q });
            i = end;
        }
        f.end = i;
        out.push(f);
        if (i < L && s[i] === D) { i++; if (i >= L) { out.push({ start: i, end: i, value: '', quoted: false, issues: [] }); break; } continue; }
        return out;
    }
    return out;
}

const INVISIBLE = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F­​-‏‪-‮⁠-⁤﻿�]/g;
const FORMULA_START = /^[=+\-@\t\r]/;

function diagnoseRow(t, row) {
    const B = t.base;
    const out = { row: row.id, line: row.src != null ? row.src + 1 : null, findings: [], repair: null };
    if (!B || row.b < 0) { out.none = 'This row was created here: it has no bytes in the file yet.'; return out; }
    const text = recordText(B, row.b), D = B.delim, span = recordSpan(B, row.b), kinds = columnKinds(t);
    const fields = B.lines ? [{ start: 0, end: text.length, value: text, quoted: false, issues: [] }] : fieldSpans(text, D);
    const expected = B.lines ? 1 : (t.syntheticHeader ? dominantWidth(t) : t.headers.length), dominant = dominantWidth(t);
    Object.assign(out, { expected, dominant, observed: fields.length, bytes: [span.s, span.e], encoding: encName(B.enc), edited: !!row.d });
    const add = (kind, severity, title, detail, at, len) => out.findings.push({ kind, severity, title, detail, near: at != null ? bytesAround(B, at, len || 1) : null });
    const colName = c => c < t.headers.length ? `“${t.headers[c]}”` : `field ${c + 1}`;

    /* --- shape: how many fields, and why not the expected count ------------ */
    if (!B.lines && fields.length !== expected) {
        const k = fields.length - expected;
        if (k > 0) shapeExtra(t, row, fields, text, expected, kinds, span, out, add, colName);
        else shapeShort(t, row, fields, expected, out, add);
    }
    /* --- quotes ------------------------------------------------------------- */
    fields.forEach((f, c) => f.issues.forEach(q => {
        if (q.kind === 'unclosed') {
            const lines = text.split('\n').length;
            add('unclosed-quote', 'error', `A quote opened in ${colName(c)} is never closed`,
                lines > 1 ? `Everything after it, ${fmt(lines - 1)} line break${lines > 2 ? 's' : ''} included, was read as one field.` : 'The field runs to the end of the line.', span.at(q.at));
        } else if (q.kind === 'after-quote') {
            const swallowed = f.value.split('\n').length - 1;
            if (swallowed) add('runaway-quote', 'error', `A quote in ${colName(c)} runs on past the end of its line`,
                `“${text.slice(q.at, q.at + 12).split('\n')[0]}” follows its closing quote, so that quote does not close the field: by the CSV rules the field goes on to the next quote, ${fmt(swallowed)} line${swallowed > 1 ? 's' : ''} further. ${swallowed > 1 ? 'Those lines are' : 'That line is'} read as part of this row, not as rows of their own — even when the field count still looks right.`, span.at(q.at));
            else add('text-after-quote', 'warn', `Text after a closing quote in ${colName(c)}`,
                'A quoted field must end at its closing quote; the quote is kept as text here, other programs may split or drop it.', span.at(q.at));
        }
        else if (q.kind === 'space-before-quote') add('space-before-quote', 'warn', `Spaces before a quote in ${colName(c)}`,
            'The field is read as text with its quotes; programs that trim spaces read it as a quoted field.', span.at(q.at));
        else add('quote-inside', 'info', `A quote inside the unquoted ${colName(c)}`, 'Kept as an ordinary character (RFC 4180 does not allow it; most readers accept it).', span.at(q.at));
    }));
    /* A record over several lines of the file, for no reason found above: say so. */
    const spans = text.split('\n').length - 1;
    if (spans && !out.findings.some(f => f.kind === 'runaway-quote' || f.kind === 'unclosed-quote'))
        add('multiline', 'info', `This row spans ${fmt(spans + 1)} lines of the file`, 'A quoted value holds a line break: valid CSV, but programs that read line by line cut the row there.', span.at(text.indexOf('\n')));
    /* --- characters ----------------------------------------------------------- */
    let m; INVISIBLE.lastIndex = 0;
    const seen = new Set();
    while ((m = INVISIBLE.exec(text))) {
        const cp = m[0].codePointAt(0); if (seen.has(cp)) continue;
        /* A zero-width joiner between two pictographs is how 👨‍👩‍👧 is written, not an anomaly. */
        if (cp === 0x200D && /\p{Extended_Pictographic}/u.test(text.slice(Math.max(0, m.index - 4), m.index)) && /\p{Extended_Pictographic}/u.test(text.slice(m.index + 1, m.index + 3))) continue;
        seen.add(cp);
        const c = fields.findIndex(f => m.index >= f.start && m.index < Math.max(f.end, f.start + 1)), where = c >= 0 ? ` in ${colName(c)}` : '';
        if (cp === 0) add('nul', 'error', `NUL byte${where}`, 'Many programs stop reading a value, or the whole file, at a NUL.', span.at(m.index));
        else if (cp === 0xFFFD) add('replacement', 'error', `Replacement character �${where}`, B.transcoded
            ? 'Invalid UTF-16 here (a lone surrogate): kept byte for byte in the file, shown as �.'
            : 'U+FFFD is in the file itself: a character was already lost by the program that wrote it.', span.at(m.index), B.orig ? 2 : 3);
        else add('invisible', cp < 32 || cp === 127 ? 'warn' : 'info', `${invName(cp)} (U+${cp.toString(16).toUpperCase().padStart(4, '0')})${where}`,
            'Invisible in most programs; it makes two values that look the same compare different.', span.at(m.index));
    }
    /* --- encoding ------------------------------------------------------------- */
    diagnoseEncoding(t, B, row, text, span, add, out);
    /* --- spreadsheet formulas --------------------------------------------------- */
    const inj = fields.findIndex(f => FORMULA_START.test(f.value) && !/^[+-]?[\d\s.,]+$/.test(f.value) && !/^-+$/.test(f.value));
    if (inj >= 0) add('formula', 'info', `${colName(inj)} starts with “${fields[inj].value[0] === '\t' ? '⇥' : fields[inj].value[0]}”`,
        'Excel and LibreOffice run a cell starting with = + - @ as a formula when the file is opened there (CSV injection).', span.at(fields[inj].start));
    return out;
}

/* Too many fields: which delimiter is part of a value? Each place where k+1
   consecutive fields could be one is tried, and the realigned row scored
   against the columns' types; a delimiter followed by a space ("Lyon; France")
   is the likeliest to be text. */
function shapeExtra(t, row, fields, text, expected, kinds, span, out, add, colName) {
    const k = fields.length - expected, D = t.base.delim, vals = fields.map(f => f.value);
    const trailing = vals.slice(expected).every(v => v === '');
    if (trailing) {
        add('trailing-delimiter', 'warn', `${fmt(k)} empty field${k > 1 ? 's' : ''} at the end of the line`, 'A delimiter at the end of the line: harmless, but the row is one field wider than the header.', span.at(fields[expected].start - 1));
        out.cause = 'a delimiter at the end of the line';
        out.repair = { kind: 'trim', label: `Drop the ${fmt(k)} empty trailing field${k > 1 ? 's' : ''}`, fields: vals.slice(0, expected) };
        return;
    }
    const match = (v, kind) => { const ty = cellType(v); return !ty || !kind ? 0.5 : ty === kind ? 1 : 0; };
    let best = null;
    for (let p = 0; p + k < fields.length; p++) {
        const merged = vals.slice(p, p + k + 1).join(D);
        const aligned = [...vals.slice(0, p), merged, ...vals.slice(p + k + 1)];
        let score = 0;
        aligned.forEach((v, c) => score += match(v, kinds[c]));
        for (let j = p; j < p + k; j++) if (text[fields[j].end + 1] === ' ') score += 0.75;   // "; " reads as text
        if (kinds[p] === 't') score += 0.25;
        if (!best || score > best.score) best = { p, score, aligned, tie: false };
        else if (score === best.score) best.tie = true;
    }
    const p = best.p, at = fields[p].end;                       // the first delimiter that should not split
    if (best.tie) {
        add('extra-fields', 'error', `${fmt(k)} field${k > 1 ? 's' : ''} more than the header`, `An unquoted delimiter inside a value, most likely in ${colName(p)} — the column types do not settle it for sure.`, span.at(at));
        out.cause = `an unquoted delimiter, perhaps in ${colName(p)}`;
    } else {
        add('extra-fields', 'error', `Unquoted delimiter in ${colName(p)}`, `“${vals.slice(p, p + k + 1).join(D)}” is one value cut in ${fmt(k + 1)}: realigned this way, every column gets the kind of value it usually holds.`, span.at(at));
        out.cause = `unquoted delimiter in ${colName(p)}`;
    }
    out.repair = { kind: 'merge', label: k === 1 ? `Quote fields ${p + 1}–${p + 2} as one ${colName(p)}` : `Quote fields ${p + 1}–${p + k + 1} as one ${colName(p)}`, fields: best.aligned };
}

/* Too few fields: a line cut by an unquoted line break (it and the next line
   make one row), or fields simply missing at the end. */
function shapeShort(t, row, fields, expected, out, add) {
    const B = t.base, k = expected - fields.length, vals = fields.map(f => f.value);
    const i = t.allData.indexOf(row), next = i >= 0 ? t.allData[i + 1] : null;
    if (next && !next.d && next.b === row.b + 1 && fields.length + next.len - 1 === expected) {
        const nv = next.data;
        add('split-line', 'error', `This row continues on the next line`, `${fmt(fields.length)} fields here and ${fmt(next.len)} on line ${fmt(next.src + 1)}: together, with the line break as part of a value, they make the ${fmt(expected)} expected. A line break inside a value that was not quoted.`, recordSpan(B, row.b).e, 1);
        out.cause = 'an unquoted line break: the row is cut in two';
        out.repair = { kind: 'join', label: `Join with line ${fmt(next.src + 1)}`, fields: [...vals.slice(0, -1), vals[vals.length - 1] + '\n' + nv[0], ...nv.slice(1)], next };
        return;
    }
    add('missing-fields', 'warn', `${fmt(k)} field${k > 1 ? 's' : ''} missing`, `${fmt(fields.length)} fields for ${fmt(expected)} columns. Saved as is, the line stays short; most readers fill the missing columns with empty values.`, null);
    out.cause = `${fmt(k)} field${k > 1 ? 's' : ''} missing at the end`;
    out.repair = { kind: 'pad', label: `Add ${fmt(k)} empty field${k > 1 ? 's' : ''} at the end`, fields: [...vals, ...new Array(k).fill('')] };
}

/* What the bytes say about the encoding of this line. */
function diagnoseEncoding(t, B, row, text, span, add, out) {
    if (B.transcoded) return;
    const f = fileBytes(B), bytes = f.subarray(span.s, span.e);
    let high = 0, firstHigh = -1;
    for (let i = 0; i < bytes.length; i++) if (bytes[i] >= 0x80) { high++; if (firstHigh < 0) firstHigh = i; }
    if (B.enc === 'utf-8') {
        if (hasMojibake(text)) {
            const at = text.search(mojibakeRe());
            add('mojibake', 'warn', 'Garbled accents (mojibake)', `“${(text.match(mojibakeRe()) || [''])[0]}” is UTF-8 that was read as Windows-1252 and saved again, before this file was written. Clean up › Repair garbled accents decodes it back.`, at >= 0 ? span.at(at) : null, 2);
        }
        return;
    }
    if (!high) return;
    /* Read as a single-byte encoding: are this line's bytes valid UTF-8 after all? */
    let utf8 = null;
    try { utf8 = new TextDecoder('utf-8', { fatal: true }).decode(bytes); } catch (e) { }
    if (utf8 != null) {
        add('utf8-line', 'error', `This line is UTF-8 in a file read as ${encName(B.enc)}`,
            `Its ${fmt(high)} bytes ≥ 0x80 form valid UTF-8 sequences, which text in ${encName(B.enc)} almost never does: read as UTF-8 it says “${utf8.length > 60 ? utf8.slice(0, 60) + '…' : utf8}”. The file mixes two encodings (lines appended by another program?).`, span.s + firstHigh, 2);
        /* Repaired by reading the line as UTF-8 — saved in the file's encoding, so only
           when every character exists there (else the repair would write "?"). */
        const fields = splitRecord(utf8, B.delim), bad = SINGLE_BYTE.includes(B.enc) ? sbTable(B.enc).bad : null;
        if (!out.repair && !(bad && fields.some(v => { bad.lastIndex = 0; return bad.test(v); })))
            out.repair = { kind: 'reencode', label: `Read this line as UTF-8 (saved back in ${encName(B.enc)})`, fields };
    } else if (B.enc === 'windows-1252') {
        const ch = new TextDecoder(B.enc).decode(bytes.subarray(firstHigh, firstHigh + 1));
        add('cp1252', 'info', 'Read as Windows-1252', `${fmt(high)} byte${high > 1 ? 's' : ''} ≥ 0x80 here (first: 0x${bytes[firstHigh].toString(16).toUpperCase()}, “${ch}”), not valid UTF-8: consistent with the file's encoding.`, span.s + firstHigh);
    }
}

/* --- the panel ------------------------------------------------------------- */
function diagnosisHtml(t, row) {
    const d = diagnoseRow(t, row);
    if (d.none) return `<div class="dg-none">${esc(d.none)}</div>`;
    const head = `<div class="dg-shape"><span>Expected <b>${fmt(d.expected)}</b> field${d.expected > 1 ? 's' : ''}${!t.syntheticHeader && d.dominant !== d.expected ? ` <span class="dg-dim">(header; most rows: ${fmt(d.dominant)})</span>` : ''}</span>`
        + `<span>Observed <b class="${d.observed !== d.expected ? 'dg-bad' : ''}">${fmt(d.observed)}</b>${d.observed !== d.expected ? ` <span class="dg-dim">(${d.observed > d.expected ? '+' : '−'}${fmt(Math.abs(d.observed - d.expected))})</span>` : ''}</span>`
        + `<span class="dg-dim">line ${d.line != null ? fmt(d.line) : '—'} · bytes ${fmt(d.bytes[0])}–${fmt(d.bytes[1])} · ${esc(d.encoding)}${d.edited ? ' · edited since read' : ''}</span></div>`;
    if (!d.findings.length) return head + '<div class="dg-ok">Nothing wrong found in this line’s bytes.</div>';
    const items = d.findings.map(f => `<div class="dg-f ${f.severity}"><div class="dg-t">${esc(f.title)}</div><div class="dg-d">${esc(f.detail)}</div>${f.near ? nearHtml(f.near) : ''}</div>`).join('');
    const rep = d.repair ? `<div class="dg-rep"><div class="dg-t">Suggested repair</div><div class="dg-d">${esc(d.repair.label)}</div>`
        + `<div class="dg-prev">${d.repair.fields.map((v, c) => `<span title="${esc(c < t.headers.length ? t.headers[c] : 'field ' + (c + 1))}">${esc(v)}</span>`).join('<i>│</i>')}</div>`
        + `<button class="btn btn-outline" onclick="applyRepair()">Apply</button></div>` : '';
    return head + (d.cause ? `<div class="dg-cause">Probable cause: <b>${esc(d.cause)}</b></div>` : '') + items + rep;
}
function nearHtml(n) {
    return `<div class="dg-near"><span class="dg-off">@${fmt(n.at)}</span>${n.hex.map(x => `<span class="${x.hit ? 'hit' : ''}">${x.h}</span>`).join(' ')}`
        + `<span class="dg-txt">${esc(n.text.replace(/[\u0000-\u001F]/g, c => c === '\n' ? '↵' : c === '\r' ? '␍' : c === '\t' ? '⇥' : '·'))}</span></div>`;
}

/* The suggested repair, as one undoable edit. */
function applyRepair() {
    const { t, row } = rowCard; if (!t || !row || T() !== t) return;
    const d = diagnoseRow(t, row), r = d.repair; if (!r) return;
    if (r.kind === 'join') {
        const prevD = row.d, next = r.next;
        row.d = r.fields;
        commitRows(t, t.allData.filter(x => x !== next), { id: row.id, col: '---', old: 'rows', new: 'joined', what: `join line ${fmt(next.src + 1)} into row ${fmt(row.id)}` }, () => { row.d = prevD; });
    } else {
        const ed = rowEdits(); ed.put(row, r.fields);
        t.modificationsLog.push({ id: row.id, col: '---', old: 'row', new: r.kind, what: `${r.label.toLowerCase()} (row ${fmt(row.id)})`, undo: () => ed.undo() });
        updateSaveBtn(); renderTabBar(); applyFilters(); updateStats();
    }
    updateIrregular(t); rowCard.key = ''; rowCardSync(true);
}
