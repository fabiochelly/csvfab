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
   A workbook with several sheets holding data asks which one (pickSheet()):
   empty sheets — most workbooks carry a Feuil2 and a Feuil3 — and hidden
   ones are not offered, so a single sheet of data opens without a word. The
   CSV is then named after the sheet too: <name> - <sheet>.csv. A bridge path
   asks the server for the list first (/api/xlsx-sheets, nothing uploaded);
   a picked or dropped workbook is posted with pick=1, and the server answers
   with the list instead of a CSV only when there is a choice to make.
----------------------------------------------------------------*/
const IMPORT_RE = /\.(xlsx|xlsm|json|jsonl|ndjson)$/i;
/* A JSON file is asked about: its records flattened into a CSV beside it, or the file itself as
   raw text, line by line (33-…). true: as text; false: convert (or not an import at all); null: cancelled. */
async function importAsText(name) {
    if (importKind(name) !== 'json') return false;
    const r = await uiChoice(`Open "${name}" as a table or as text?\n\nFlatten to CSV turns its records into rows and columns, written to a CSV beside it. Raw text opens the file itself, one line per row, every space kept.`,
        { ok: 'Flatten to CSV', choices: ['Raw text'] });
    return r === 'Raw text' ? true : r === true ? false : null;
}
function importKind(name) { const m = String(name).match(IMPORT_RE); return m ? (/^json|^ndjson|^jsonl/i.test(m[1]) ? 'json' : 'xlsx') : null; }
function importDelim() { return parseDefaults.delimiter || ';'; }
const csvNameFor = (name, sheet) => name.replace(/\.[^.]+$/, '') + (sheet ? ' - ' + sheet.replace(/[\\/:*?"<>|\x00-\x1f]+/g, '_').trim() : '') + '.csv';

/* The sheets worth offering: visible, with at least one value. */
const sheetChoices = sheets => sheets.filter(s => s.filled && !s.hidden);
/* Which sheet to convert: null when cancelled. A list of the sheets with data, their size
   from the workbook's own <dimension> when it gives one; ↑ ↓ and Enter, or a click. */
function pickSheet(file, sheets) {
    const list = sheetChoices(sheets), empty = sheets.filter(s => !s.filled).length, hidden = sheets.filter(s => s.filled && s.hidden).length;
    if (list.length <= 1) return Promise.resolve(list.length ? list[0].name : sheets[0].name);
    return new Promise(resolve => {
        const bg = document.createElement('div'); bg.id = 'dlg-bg';
        const box = document.createElement('div'); box.id = 'dlg'; box.className = 'sheet-dlg';
        box.setAttribute('role', 'dialog'); box.setAttribute('aria-modal', 'true');
        const left = [empty && `${empty} empty sheet${empty > 1 ? 's' : ''}`, hidden && `${hidden} hidden sheet${hidden > 1 ? 's' : ''}`].filter(Boolean).join(' and ');
        box.innerHTML = `<div class="dlg-main">"${esc(file)}" has ${list.length} sheets with data</div>`
            + `<div class="dlg-sub">Pick the one to open: it becomes a CSV of its own, named after the sheet.${left ? ` Not listed: ${left}.` : ''}</div>`
            + `<div class="sheet-list">${list.map((s, i) => `<div class="sheet-opt${i ? '' : ' act'}" data-i="${i}"><span class="sn">${esc(s.name)}</span>`
                + `<span class="sd">${s.dim ? `${fmt(s.dim[0])} row${s.dim[0] === 1 ? '' : 's'} × ${fmt(s.dim[1])} col${s.dim[1] === 1 ? '' : 's'}` : ''}</span></div>`).join('')}</div>`
            + '<div class="modal-actions"><button class="btn btn-outline" data-r="0">Cancel</button><button class="btn" data-r="1">Open</button></div>';
        document.body.append(bg, box);
        let act = 0;
        const opts = [...box.querySelectorAll('.sheet-opt')];
        const mark = () => { opts.forEach((o, i) => o.classList.toggle('act', i === act)); opts[act].scrollIntoView({ block: 'nearest' }); };
        const done = name => { document.removeEventListener('keydown', onKey, true); bg.remove(); box.remove(); resolve(name); };
        /* Capture phase, as uiDialog: no other shortcut fires underneath. */
        const onKey = e => {
            e.stopPropagation();
            if (e.key === 'Escape') { e.preventDefault(); done(null); }
            else if (e.key === 'Enter') { e.preventDefault(); done(list[act].name); }
            else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); act = (act + (e.key === 'ArrowDown' ? 1 : -1) + list.length) % list.length; mark(); }
        };
        document.addEventListener('keydown', onKey, true);
        opts.forEach((o, i) => { o.onclick = () => { act = i; mark(); }; o.ondblclick = () => done(list[i].name); });
        bg.onclick = () => done(null);
        box.querySelector('[data-r="0"]').onclick = () => done(null);
        box.querySelector('[data-r="1"]').onclick = () => done(list[act].name);
        box.querySelector('[data-r="1"]').focus();
    });
}

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
    const kind = importKind(baseName(p)), name = baseName(p), delim = importDelim();
    /* Several sheets with data: which one, before anything is written (its name is in the CSV's). */
    let sheet = null;
    if (kind === 'xlsx') {
        const r = await srvFetch(`/api/xlsx-sheets?path=${encodeURIComponent(p)}`);
        const j = await r.json().catch(() => ({}));
        if (!r.ok) { uiAlert(`Could not read "${name}".\n\n${j.error || `HTTP ${r.status}`}`); return null; }
        if (sheetChoices(j.sheets).length > 1) { sheet = await pickSheet(name, j.sheets); if (!sheet) return null; }
    }
    const dest = pathDir(p) + csvNameFor(name, sheet);
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
            const r = await srvFetch(`/api/xlsx2csv?src=${encodeURIComponent(p)}&dest=${encodeURIComponent(dest)}&delim=${encodeURIComponent(delim)}${sheet ? '&sheet=' + encodeURIComponent(sheet) : ''}${exists ? '&backup=1' : ''}`, { method: 'POST' });
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
    return `${from} → ${to}: ${fmt(j.rows || 0)} rows` + (j.sheet ? `, sheet "${j.sheet}"` : '') + (j.others ? ` (${j.others} other sheet${j.others > 1 ? 's' : ''} with data not converted)` : '')
        + (j.cols ? `, ${fmt(j.cols)} columns` : '') + '.';
}

/* A File (picked, dropped, or a handle's): the CSV as text or bytes, and the sheet chosen when
   the workbook has several with data (its name goes into the CSV's) — null: cancelled. */
async function convertFile(file) {
    const kind = importKind(file.name);
    if (kind === 'json') { const { text, rows, cols } = jsonToCsv(await file.text(), importDelim()); return { blob: new Blob([text], { type: 'text/csv' }), info: { rows, cols }, sheet: null }; }
    if (!SRV) throw new Error('converting a workbook needs the local bridge — start the app with csvfab (or ./serve.sh)');
    const url = `/api/xlsx2csv?delim=${encodeURIComponent(importDelim())}`;
    let r = await srvFetch(url + '&pick=1', { method: 'POST', body: file }), sheet = null;
    if (r.ok && (r.headers.get('Content-Type') || '').startsWith('application/json')) {   // a choice to make: the list, nothing converted
        const j = await r.json();
        endProgress();
        sheet = await pickSheet(file.name, j.choose);
        if (!sheet) return null;
        startProgress();
        r = await srvFetch(url + '&sheet=' + encodeURIComponent(sheet), { method: 'POST', body: file });
    }
    if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || `HTTP ${r.status}`); }
    return { blob: await r.blob(), info: JSON.parse(r.headers.get('X-Xlsx-Info') || '{}'), sheet };
}
/* A handle (picker, drop): the CSV goes beside the source when its folder is
   known, else wherever a save picker says; a dismissed picker still opens the
   converted rows, as a read-only copy that Save as… can write. */
async function importHandle(h, dirHandle) {
    setStats(`Converting ${h.name}…`); startProgress();
    let conv;
    try { conv = await convertFile(await h.getFile()); }
    catch (err) { endProgress(); uiAlert(`Could not convert "${h.name}".\n\n${err.message || err}`); return null; }
    endProgress();
    if (!conv) { setStats(`${h.name}: not opened.`); return null; }
    const { blob, info } = conv, name = csvNameFor(h.name, conv.sheet);
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
        const conv = await convertFile(file); if (!conv) return null;
        const { blob, info } = conv, name = csvNameFor(file.name, conv.sheet);
        toast(importNote(file.name, name, info) + ' Opened as a copy: use Save as… to write it.', { kind: 'ok', ms: 9000 });
        return { file: new File([blob], name, { type: 'text/csv' }), name, size: blob.size };
    } catch (err) { uiAlert(`Could not convert "${file.name}".\n\n${err.message || err}`); return null; }
}
