/* ---------------------------------------------------------------
   OPENING EXCEL AND JSON FILES
   csvfab edits CSV files, and only those: a workbook or a JSON file is
   first written as a CSV beside it — <name>.csv — and that CSV is the
   tab. The workbook's first sheet is converted by server.py (stdlib
   zipfile + XML, streamed: cell values as text, dates from their number
   format, numbers with a decimal comma in a ;-file); JSON is flattened
   here: an array of objects (or the first array found in an object, or
   one object per line) gives one column per key, nested keys joined
   with a dot, arrays kept as JSON text. A bridge path is converted on
   the server; a picked or dropped file is converted and written beside
   its source when that folder is known, else through a save picker —
   and failing that, opened as a read-only copy to save from.
----------------------------------------------------------------*/
const IMPORT_RE = /\.(xlsx|xlsm|json|jsonl|ndjson)$/i;
function importKind(name) { const m = String(name).match(IMPORT_RE); return m ? (/^json|^ndjson|^jsonl/i.test(m[1]) ? 'json' : 'xlsx') : null; }
function importDelim() { return parseDefaults.delimiter || ';'; }
const csvNameFor = name => name.replace(/\.[^.]+$/, '') + '.csv';

/* --- JSON → CSV, in the page --- */
function jsonRecords(text) {
    let data;
    try { data = JSON.parse(text); }
    catch (e) {
        const lines = text.split(/\r?\n/).filter(l => l.trim());        // JSON Lines: one record per line
        try { data = lines.map(l => JSON.parse(l)); } catch (e2) { throw new Error(`not JSON: ${e.message}`); }
    }
    if (!Array.isArray(data)) {
        if (!data || typeof data !== 'object') throw new Error('the file holds a single value, not records');
        const k = Object.keys(data).find(k => Array.isArray(data[k]) && data[k].length && typeof data[k][0] === 'object');
        data = k ? data[k] : [data];
    }
    if (data.every(x => Array.isArray(x))) return { cols: null, rows: data.map(r => r.map(v => jsonCell(v))) };   // rows already: line 1 may be the header
    const cols = [], seen = new Set(), rows = [];
    const flat = (o, prefix, out) => { for (const k of Object.keys(o)) { const v = o[k], key = prefix ? `${prefix}.${k}` : k; if (v && typeof v === 'object' && !Array.isArray(v)) flat(v, key, out); else out[key] = v; } };
    for (const item of data) {
        const out = {};
        if (item && typeof item === 'object' && !Array.isArray(item)) flat(item, '', out); else out.value = item;
        for (const k of Object.keys(out)) if (!seen.has(k)) { seen.add(k); cols.push(k); }
        rows.push(out);
    }
    return { cols, rows: rows.map(o => cols.map(k => jsonCell(o[k]))) };
}
let jsonComma = false;
function jsonCell(v) {
    if (v == null) return '';
    if (typeof v === 'number') return jsonComma ? String(v).replace('.', ',') : String(v);
    if (typeof v === 'boolean') return v ? 'true' : 'false';
    if (typeof v === 'object') return JSON.stringify(v);
    return String(v);
}
function jsonToCsv(text, delim) {
    jsonComma = delim === ';';
    const { cols, rows } = jsonRecords(text);
    const quote = v => (v.includes(delim) || v.includes('"') || v.includes('\n') || v.includes('\r')) ? `"${v.replace(/"/g, '""')}"` : v;
    const lines = (cols ? [cols] : []).concat(rows).map(r => r.map(quote).join(delim));
    return { text: lines.join('\n') + '\n', rows: rows.length, cols: cols ? cols.length : Math.max(0, ...rows.map(r => r.length)) };
}

/* A bridge path: converted by the server (xlsx) or here (JSON), written beside the source. */
async function importPath(p) {
    const kind = importKind(baseName(p)), name = baseName(p), dest = pathDir(p) + csvNameFor(name), delim = importDelim();
    let exists = true;
    try { await srvStat(dest); } catch (e) { exists = false; }
    if (exists) {
        if (tabs.some(x => x.path === dest)) return dest;
        const r = await uiChoice(`"${baseName(dest)}" already exists beside ${name}.\n\nOpen it as it is, or convert ${name} again and overwrite it (a .bak copy is kept)?`, { ok: 'Convert again', choices: ['Open the existing CSV'], danger: true });
        if (r === 'Open the existing CSV') return dest;
        if (r !== true) return null;
    }
    setStats(`Converting ${name}…`); startProgress();
    try {
        let j;
        if (kind === 'xlsx') {
            const r = await srvFetch(`/api/xlsx2csv?src=${encodeURIComponent(p)}&dest=${encodeURIComponent(dest)}&delim=${encodeURIComponent(delim)}${exists ? '&backup=1' : ''}`, { method: 'POST' });
            j = await r.json().catch(() => ({}));
            if (!r.ok || !j.ok) throw new Error(j.error || `HTTP ${r.status}`);
        } else {
            const { bytes } = await tabBytes({ path: p, name }, setProgress);
            const { text, rows, cols } = jsonToCsv(new TextDecoder('utf-8').decode(bytes), delim);
            const r = await srvFetch(srvFileUrl(dest) + (exists ? '&backup=1' : ''), { method: 'PUT', body: new Blob([text], { type: 'text/csv' }) });
            j = await r.json().catch(() => ({}));
            if (!r.ok || !j.ok) throw new Error(j.error || `HTTP ${r.status}`);
            j.rows = rows; j.cols = cols;
        }
        endProgress();
        toast(importNote(name, baseName(dest), j), { kind: 'ok', ms: 9000 });
        return dest;
    } catch (err) {
        endProgress(); setStats(`Cannot convert ${name}: ${err.message || err}`);
        uiAlert(`Could not convert "${name}".\n\n${err.message || err}`);
        return null;
    }
}
function importNote(from, to, j) {
    return `${from} → ${to}: ${fmt(j.rows || 0)} rows` + (j.sheet ? `, sheet "${j.sheet}"` : '') + (j.sheets > 1 ? ` (${j.sheets - 1} other sheet${j.sheets > 2 ? 's' : ''} left out)` : '')
        + (j.cols ? `, ${fmt(j.cols)} columns` : '') + '.';
}

/* A File (picked, dropped, or a handle's): the CSV as text or bytes. */
async function convertFile(file) {
    const kind = importKind(file.name);
    if (kind === 'json') { const { text, rows, cols } = jsonToCsv(await file.text(), importDelim()); return { blob: new Blob([text], { type: 'text/csv' }), info: { rows, cols } }; }
    if (!SRV) throw new Error('converting a workbook needs the local bridge — start the app with csvfab (or ./serve.sh)');
    const r = await srvFetch(`/api/xlsx2csv?delim=${encodeURIComponent(importDelim())}`, { method: 'POST', body: file });
    if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || `HTTP ${r.status}`); }
    return { blob: await r.blob(), info: JSON.parse(r.headers.get('X-Xlsx-Info') || '{}') };
}
/* A handle (picker, drop): the CSV goes beside the source when its folder is
   known, else wherever a save picker says; a dismissed picker still opens the
   converted rows, as a read-only copy that Save as… can write. */
async function importHandle(h, dirHandle) {
    const name = csvNameFor(h.name);
    setStats(`Converting ${h.name}…`); startProgress();
    let blob, info;
    try { ({ blob, info } = await convertFile(await h.getFile())); }
    catch (err) { endProgress(); uiAlert(`Could not convert "${h.name}".\n\n${err.message || err}`); return null; }
    endProgress();
    let target = null;
    if (dirHandle && await ensureWritable(dirHandle)) {
        let exists = null;
        try { exists = await dirHandle.getFileHandle(name); } catch (e) { }
        if (exists) {
            const r = await uiChoice(`"${name}" already exists beside ${h.name}.\n\nOpen it as it is, or convert ${h.name} again and overwrite it?`, { ok: 'Overwrite', choices: ['Open the existing CSV'], danger: true });
            if (r === 'Open the existing CSV') { const f = await exists.getFile(); return { file: f, handle: exists, dirHandle, name: f.name, size: f.size }; }
            if (r !== true) return null;
        }
        try { target = await dirHandle.getFileHandle(name, { create: true }); } catch (e) { target = null; }
    }
    if (!target && FSA) {
        try { target = await showSaveFilePicker({ suggestedName: name, types: CSV_TYPES, startIn: h }); } catch (e) { target = null; }
    }
    if (target) {
        try {
            const w = await target.createWritable(); await w.write(blob); await w.close();
            const f = await target.getFile();
            toast(importNote(h.name, target.name, info), { kind: 'ok', ms: 9000 });
            return { file: f, handle: target, dirHandle, name: f.name, size: f.size };
        } catch (err) { uiAlert(`Could not write "${name}".\n\n${err.message || err}`); }
    }
    toast(`${h.name} converted and opened as a copy — use Save as… to write it to disk.`, { kind: 'warn', ms: 9000 });
    return { file: new File([blob], name, { type: 'text/csv' }), name, size: blob.size };
}
/* A plain File (legacy input, a drop from an archive): a read-only copy. */
async function importFile(file) {
    try {
        const { blob, info } = await convertFile(file), name = csvNameFor(file.name);
        toast(importNote(file.name, name, info) + ' Opened as a copy: use Save as… to write it.', { kind: 'ok', ms: 9000 });
        return { file: new File([blob], name, { type: 'text/csv' }), name, size: blob.size };
    } catch (err) { uiAlert(`Could not convert "${file.name}".\n\n${err.message || err}`); return null; }
}
