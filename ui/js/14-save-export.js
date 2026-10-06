// --- ASYNC CSV WRITING ENGINE ---
/* Rows are serialised in slices with a yield between them, so a million-row
   file neither freezes the UI nor has to exist as one giant string: a sink
   is either an array of chunks (the bridge) or a FileSystemWritableFileStream
   writing straight to disk.
   A row still its record (no edit gave it an array), written with the
   file's own delimiter and encoding and every column in place, is copied
   as its bytes — a run of such rows as one slice of the file — so saving
   a large file with a few edits costs little more than a copy. */
async function streamCSV(t, dataToExport, columnsToExport, rawSink, delimOverride, encOverride) {
    const delim = delimOverride || currentDelim(t);
    const we = encOverride && typeof encOverride === 'object' ? encOverride : writeEnc(t, encOverride);
    const encode = encoderFor(we.enc);
    const sink = c => rawSink(encode(c));
    if (we.bom) await rawSink(bomFor(we.enc));
    const eol = t.detectedEol || '\n';

    const chunkSize = 10000;
    const total = dataToExport.length;
    /* Writing the whole row: a row longer than the header keeps its extra
       fields rather than losing them (a short one is padded). */
    const width = t.headers.length;
    const whole = columnsToExport.length === width && columnsToExport.every((c, k) => c === k);
    /* Raw text (33-…): a line is written as it is. */
    const text = !!(t.base && t.base.lines) && delim === '\n';
    const quote = text ? v => String(v ?? '') : v => {
        const sv = String(v ?? '');
        return (sv.includes(delim) || sv.includes('"') || sv.includes('\n') || sv.includes('\r')) ? `"${sv.replace(/"/g, '""')}"` : sv;
    };
    const B = t.base, raw = !!B && whole && !B.cmap && B.delim === delim && B.enc === we.enc;
    /* Untouched records' bytes, as they are in the file: records b0…b1−1, less
       `cut` trailing line-break bytes. A UTF-16 file is read through a UTF-8
       copy (B.transcoded) but keeps its own bytes (B.orig): a record starts
       there at 2 × its offset in UTF-16 units (B.chars, or B.starts when the
       text is ASCII) — the decoder turns each invalid unit into one U+FFFD,
       so the offsets hold even across broken surrogates, copied as they are. */
    const units = B && (B.chars || B.starts);
    const span = B && B.orig
        ? (b0, b1, cut = 0) => B.orig.subarray(B.origAt + 2 * units[b0], Math.min(B.orig.length, B.origAt + 2 * (units[b1] - cut)))
        : (b0, b1, cut = 0) => B.u8.subarray(B.starts[b0], B.starts[b1] - cut);
    /* How many line-break bytes end record b (its own break and blank lines after it). */
    const breaks = b => { const s = B.starts[b], e = B.starts[b + 1]; let k = 0; while (e - k > s && (B.u8[e - k - 1] === 10 || B.u8[e - k - 1] === 13)) k++; return k; };
    /* A file that ended without a line break still does: none after the last line written. */
    const noEnd = !!B && B.u8.length > 0 && B.u8[B.u8.length - 1] !== 10 && B.u8[B.u8.length - 1] !== 13;
    /* Same line break as the file: a record's span, blank lines after it
       included, is written as is; otherwise the record and our break. */
    const sameEol = raw && B.eol === eol, eolBytes = encode(eol);
    const endsNl = raw && B.u8.length > 0 && !noEnd;

    /* Empty lines before the first record belong to no record (the scan hangs an
       empty line on the record before it): copied first, or a file opening with
       blank lines — or made of nothing else — would lose them. */
    if (sameEol && B.starts[0] > 0) await rawSink(B.orig ? B.orig.subarray(B.origAt, B.origAt + 2 * units[0]) : B.u8.subarray(0, B.starts[0]));

    /* The header line: a generated 0,1,2… header never existed in the file and
       is not written. One that is still the file's line 1, unchanged, is copied
       as its bytes like any untouched row — its quotes, spacing and all; a
       changed one is written quoted where it needs to be. */
    if (!t.syntheticHeader && width) {
        const lastLine = !total && noEnd;
        if (raw && t.headerSrc === 0 && B.n > 0 && sameFields(t.headers, recordFields(B, 0))) {
            if (sameEol) {
                await rawSink(span(0, 1));
                if (B.n === 1 && !endsNl && total) await rawSink(eolBytes);   // the file was its header alone, without a break
            } else {
                await rawSink(span(0, 1, breaks(0)));
                if (!lastLine) await rawSink(eolBytes);
            }
        } else await sink(columnsToExport.map(i => t.headers[i]).map(quote).join(delim) + (lastLine ? '' : eol));
    }
    let lastYield = performance.now();
    for (let i = 0; i < total; i += chunkSize) {
        const parts = [];
        let str = '', runS = -1, runE = -1;                       // a run of consecutive untouched records, runS…runE−1
        const flushStr = () => { if (str) { parts.push(encode(str)); str = ''; } };
        const flushRun = () => { if (runS >= 0) { parts.push(span(runS, runE)); runS = -1; } };
        for (let j = i; j < Math.min(i + chunkSize, total); j++) {
            const r = dataToExport[j];
            if (raw && !r.d && r.b >= 0) {
                flushStr();
                if (sameEol) {
                    if (runS >= 0 && r.b === runE) runE = r.b + 1; else { flushRun(); runS = r.b; runE = r.b + 1; }
                    if (r.b === B.n - 1 && !endsNl && !(noEnd && j === total - 1)) { flushRun(); parts.push(eolBytes); }   // the file's last line had no break
                } else {
                    flushRun();
                    parts.push(span(r.b, r.b + 1, breaks(r.b))); if (!(noEnd && j === total - 1)) parts.push(eolBytes);
                }
                continue;
            }
            flushRun();
            const d = r.data;
            str += (whole && d.length > width ? d : columnsToExport.map(cIdx => d[cIdx])).map(quote).join(delim) + (noEnd && j === total - 1 ? '' : eol);
        }
        flushStr(); flushRun();
        if (parts.length) await rawSink(parts.length === 1 ? parts[0] : new Blob(parts));
        /* Yield now and then, not at every slice: Chromium clamps a nested
           setTimeout(0) to 4 ms, which was 9 s of waiting on 20 M rows. */
        if (performance.now() - lastYield > 30) {
            setProgress((i + chunkSize) / total);
            await new Promise(resolve => setTimeout(resolve, 0));
            lastYield = performance.now();
        }
    }
}

/* ---------------------------------------------------------------
   SAVE OPTIONS
   The caret next to Save opens a small dialog for the writes that are
   not "same file, same format": another delimiter, an Excel workbook
   beside the source, or another name. A new CSV name is a save-as —
   the tab moves onto the new file and the source keeps its last saved
   content; a workbook is a second artefact and never takes the tab
   over (the server builds it: stdlib zipfile + XML, zero deps).
----------------------------------------------------------------*/
const DELIMS = [
    { v: ';', label: 'CSV — semicolon', shown: ';' },
    { v: ',', label: 'CSV — comma', shown: ',' },
    { v: '\t', label: 'CSV — tab', shown: '\\t' },
    { v: '|', label: 'CSV — pipe', shown: '|' }
];

function currentDelim(t) { return t.delimiter || t.detectedDelim || ';'; }
function stemOf(name) { return name.replace(/\.(csv|tsv|txt|xlsx|sqlite)$/i, ''); }
function xlsxName(t) { return stemOf(t.name) + '.xlsx'; }
function csvExt(t) { const m = t.name.match(/\.(csv|tsv|txt)$/i); return m ? m[0] : '.csv'; }
function saveChoice() { const r = document.querySelector('input[name="save-fmt"]:checked'); return r ? r.value : ''; }
/* The delimiter a format choice writes: r is the raw text of a text file (33-…), lines as they are. */
function saveDelimOf(c) { return c === 'r' ? '\n' : DELIMS[+c.slice(1)].v; }
function saveEncChoice() { const r = document.querySelector('input[name="save-enc"]:checked'); return r ? r.value : ''; }

function openSaveModal() {
    const t = T(); if (!t) return;
    if (!t.loaded) { uiAlert('The file is still loading.'); return; }
    const cur = currentDelim(t);
    document.getElementById('save-fmt').innerHTML = (cur === '\n' ? `<label class="col-label"><span><input type="radio" name="save-fmt" value="r" checked> Text file — lines as they are (current)</span><span class="k">¶</span></label>` : '')
        + DELIMS.map((d, i) =>
        `<label class="col-label"><span><input type="radio" name="save-fmt" value="d${i}" ${d.v === cur ? 'checked' : ''}> ${esc(d.label)}${d.v === cur ? ' (current)' : ''}</span><span class="k">${esc(d.shown)}</span></label>`).join('')
        + '<label class="col-label"><span><input type="radio" name="save-fmt" value="x"> Excel workbook</span><span class="k">.xlsx</span></label>'
        + '<label class="col-label"><span><input type="radio" name="save-fmt" value="q"> SQLite database — one table, typed columns</span><span class="k">.sqlite</span></label>';
    const curEnc = currentEnc(t);
    const encs = ENCODINGS.some(x => x[0] === curEnc) ? ENCODINGS : ENCODINGS.concat([[curEnc, encName(curEnc)]]);
    document.getElementById('save-enc').innerHTML = encs.map(([v, l]) =>
        `<label class="col-label"><span><input type="radio" name="save-enc" value="${v}" ${v === curEnc ? 'checked' : ''}> ${esc(l)}${v === curEnc ? ' (current)' : ''}</span></label>`).join('');
    document.querySelectorAll('input[name="save-enc"]').forEach(r => r.onchange = updateSaveNote);
    const name = document.getElementById('save-name');
    name.value = t.name;
    document.querySelectorAll('input[name="save-fmt"]').forEach(r => r.onchange = () => {
        /* The extension follows the format; the stem the user typed is kept. */
        const ext = (name.value.match(/\.[^.]*$/) || [''])[0];
        if (saveChoice() === 'x') name.value = stemOf(name.value) + '.xlsx';
        else if (saveChoice() === 'q') name.value = stemOf(name.value) + '.sqlite';
        else if (/^\.(xlsx|sqlite)$/i.test(ext)) name.value = stemOf(name.value) + csvExt(t);
        updateSaveNote();
    });
    name.oninput = updateSaveNote;
    name.onkeydown = (e) => { if (e.key === 'Enter') submitSaveModal(); };
    updateSaveNote();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-save').style.display = 'block';
    name.focus(); name.setSelectionRange(0, stemOf(name.value).length);
}

function updateSaveNote() {
    const t = T(); if (!t) return;
    const name = document.getElementById('save-name').value.trim(), c = saveChoice();
    document.getElementById('save-enc-box').style.display = c === 'x' || c === 'q' ? 'none' : '';   // a workbook or a database has no text encoding
    const changes = [];
    if (c !== 'x' && c !== 'q' && saveDelimOf(c) !== currentDelim(t)) changes.push(saveDelimOf(c) === '\n' ? 'as text' : currentDelim(t) === '\n' ? 'as a one-column CSV' : 're-delimited');
    if (c !== 'x' && c !== 'q' && saveEncChoice() !== currentEnc(t)) changes.push(`re-encoded as ${encName(saveEncChoice())}`);
    const where = t.path ? 'beside the source' : 'where you choose';
    let note;
    if (c === 'x' || c === 'q') note = `Writes the ${c === 'x' ? 'workbook' : 'database'} ${where}. ${t.name} is left as it is${isDirty(t) ? ' and keeps its pending edits' : ''}.`;
    else if (name === t.name) note = !t.path && !t.handle
        ? 'This tab is a read-only copy: change the name to write a new file.'
        : `Overwrites ${t.name}${changes.length ? ', ' + changes.join(', ') : ''} — a timestamped .bak copy is kept.`;
    else note = `Writes a new file ${where} and switches this tab to it. ${t.name} stays as it was last saved.`;
    document.getElementById('save-note').textContent = note;
}

async function submitSaveModal() {
    const t = T(); if (!t) return;
    const c = saveChoice();
    let name = document.getElementById('save-name').value.trim();
    if (!name || /[\/\\]/.test(name)) { uiAlert('Enter a file name, without any folder.'); return; }
    closeAllModals();
    /* Nothing is awaited before this point: a handle tab's save-as opens a
       picker, which needs the click's user activation. */
    if (c === 'x') { if (!/\.xlsx$/i.test(name)) name += '.xlsx'; return saveExcel(name); }
    if (c === 'q') { if (!/\.(sqlite|sqlite3|db|db3)$/i.test(name)) name += '.sqlite'; return saveExcel(name, 'sqlite'); }
    if (!/\.[^.]+$/.test(name)) name += '.csv';
    const delim = saveDelimOf(c), enc = saveEncChoice() || currentEnc(t);
    if (name === t.name) {
        /* A delimiter or encoding change is itself worth writing: it bypasses "nothing to save". */
        if (await saveInPlace({ delim: delim === currentDelim(t) ? null : delim, enc: enc === currentEnc(t) ? null : enc })) adoptFormat(t, delim, enc);
        return;
    }
    await saveAs(t, name, delim, enc);
}

/* The file really uses that delimiter and encoding now, so the tab has to
   agree — otherwise the next save, or a re-read, would disagree with disk. */
function adoptFormat(t, d, enc) {
    const bom = writeEnc(t, enc).bom;   // before detectedEnc changes: it reads the old one
    t.detectedDelim = d;
    if (t.delimiter) t.delimiter = d;
    t.detectedEnc = enc; t.bom = bom;
    if (t.encoding) t.encoding = enc;
    if (t === T()) refreshParseOpts();
}

/* Save-as: every row to a new file, which becomes this tab's file. The
   source keeps its last saved content — the pending edits move with the
   tab. A bridge tab writes beside the source; a handle tab (or a
   read-only copy) goes through the picker. */
async function saveAs(t, name, delim, enc) {
    const we = writeEnc(t, enc);   // fixed now: the tab's own encoding changes below
    if (t.path) {
        if (SINGLE_BYTE.includes(we.enc) && !await confirmEncodable(t, t.allData, we.enc)) return false;
        const dest = pathDir(t.path) + name;
        let exists = true;
        try { await srvStat(dest); } catch (e) { exists = false; }
        if (exists) {
            if (tabs.some(x => x !== t && x.path === dest)) { uiAlert(`"${name}" is open in another tab — close it first.`); return false; }
            if (!await uiConfirm(`"${name}" already exists. Overwrite it?\n\nA timestamped .bak copy of it is kept.`, { ok: 'Overwrite', danger: true })) return false;
        }
        const j = await srvWrite(t, dest, exists, delim, null, we);
        if (!j) return false;
        t.path = dest; t.size = j.size;
    } else {
        if (!FSA) { uiAlert('Writing a new file needs the File System Access API.\n\nOpen this page over http://localhost (run ./serve.sh) in Chromium or Brave.'); return false; }
        let handle;
        try {
            handle = await showSaveFilePicker({ suggestedName: name, types: CSV_TYPES, startIn: t.handle || t.dirHandle || 'documents' });
        } catch (e) { return false; }                // picker dismissed
        if (SINGLE_BYTE.includes(we.enc) && !await confirmEncodable(t, t.allData, we.enc)) return false;
        if (!await writeToHandle(t, handle, t.allData, t.headers.map((_, i) => i), delim, we)) return false;
        t.handle = handle; t.file = null; t.dirHandle = null; name = handle.name;
        try { t.size = (await handle.getFile()).size; } catch (e) { }
    }
    const from = t.name;
    t.name = name; t.backedUp = true; t.modificationsLog = [];   // nothing to protect: we just wrote it
    markPristine(t);
    await refreshStamp(t);
    adoptFormat(t, delim, we.enc);
    updateSaveBtn(); renderTabBar();
    doneMsg(`${t.name} | Saved as a new file — ${fmt(t.allData.length)} ${t.lang ? 'lines' : 'rows'} written, ${from} left as it was.`);
    return true;
}

/* Where the workbook goes when the tab has no server-side path: next to the
   file if the tab came from an open folder, otherwise wherever the user says. */
async function xlsxDestination(t, name, sqlite) {
    if (t.dirHandle && await ensureWritable(t.dirHandle)) {
        try { return await t.dirHandle.getFileHandle(name, { create: true }); }
        catch (e) { /* fall through to the picker */ }
    }
    if (!FSA) return null;
    try {
        return await showSaveFilePicker({
            suggestedName: name,
            types: [sqlite ? { description: 'SQLite database', accept: { 'application/vnd.sqlite3': ['.sqlite', '.sqlite3', '.db'] } }
                : { description: 'Excel workbook', accept: { 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': ['.xlsx'] } }]
        });
    } catch (e) { return null; }
}

/* Excel is a second artefact, not a second save target: the CSV is untouched
   and keeps whatever pending edits it had. */
/* kind 'sqlite': the same route to a SQLite database of one table (server.py, write_sqlite), named
   after the file, its columns typed from their values (a code with a leading zero stays text). */
async function saveExcel(name, kind) {
    const t = T(); if (!t) return;
    const sq = kind === 'sqlite', what = sq ? 'Database' : 'Workbook';
    name = name || (sq ? stemOf(t.name) + '.sqlite' : xlsxName(t));
    if (!t.loaded) { uiAlert('The file is still loading.'); return; }
    if (!SRV) { uiAlert(`Building a ${what.toLowerCase()} needs the local bridge.\n\nStart the app with csvfab (or ./serve.sh).`); return; }

    setStats(`Building ${name}…`);
    startProgress();
    await new Promise(r => setTimeout(r, 10));

    const delim = currentDelim(t);
    const chunks = [];
    await streamCSV(t, t.allData, t.headers.map((_, i) => i), c => chunks.push(c), delim, { enc: 'utf-8', bom: false });   // what server.py reads
    const body = new Blob(chunks, { type: 'text/csv' });
    const q = `/api/${sq ? 'sqlite' : 'xlsx'}?delim=${encodeURIComponent(delim)}&sheet=${encodeURIComponent(stemOf(name).replace(/\.(sqlite3|db3?)$/i, ''))}&header=${t.syntheticHeader ? 0 : 1}`;
    const limits = (j) => (j.truncated_rows || j.truncated_cols) ? ' · truncated to Excel\'s limits' : '';

    try {
        if (t.path) {
            const dest = pathDir(t.path) + name;
            const r = await srvFetch(`${q}&path=${encodeURIComponent(dest)}`, { method: 'POST', body });
            const j = await r.json().catch(() => ({}));
            if (!r.ok || !j.ok) throw new Error(j.error || `HTTP ${r.status}`);
            endProgress();
            doneMsg(`${t.name} | ${what} written: ${baseName(dest)} — ${fmt(j.rows)} rows${limits(j)}.`);
        } else {
            const r = await srvFetch(q, { method: 'POST', body });
            if (!r.ok) throw new Error(`HTTP ${r.status}`);
            const j = JSON.parse(r.headers.get('X-Xlsx-Info') || '{}');
            const blob = await r.blob();
            const handle = await xlsxDestination(t, name, sq);
            if (!handle) { endProgress(); setStats(`${what} not written.`); return; }
            const w = await handle.createWritable();
            await w.write(blob);
            await w.close();
            endProgress();
            doneMsg(`${t.name} | ${what} written: ${handle.name} — ${fmt(j.rows || 0)} rows${limits(j)}.`);
        }
    } catch (err) {
        endProgress();
        setStats(`${what} failed: ${err.message || err}`);
        uiAlert(`Could not build the ${what.toLowerCase()}:\n${err.message || err}`);
    }
}

// --- SAVING ---
/* A name no copy holds yet in the backup folder: it is shared by every file, and the
   stamp is to the second — two "clients.csv" from two folders saved in the same second
   wrote one copy over the other. <name>.<stamp>.bak, else -2, -3… */
async function freeBackupName(dir, base) {
    for (let k = 1; ; k++) {
        const name = k === 1 ? `${base}.bak` : `${base}-${k}.bak`;
        try { await dir.getFileHandle(name); } catch (e) { if (e.name === 'NotFoundError') return name; throw e; }
    }
}
function stamp() {
    const d = new Date(), z = n => String(n).padStart(2, '0');
    return `${d.getFullYear()}${z(d.getMonth() + 1)}${z(d.getDate())}-${z(d.getHours())}${z(d.getMinutes())}${z(d.getSeconds())}`;
}

async function chooseBackupDir() {
    if (!FSA) return null;
    try {
        const dir = await showDirectoryPicker({ mode: 'readwrite' });
        backupDir = dir; backupOptOut = false;
        await idbSet('backupDir', dir);
        return dir;
    } catch (e) { return null; }
}

/* Copy the file as it currently stands on disk, once per tab per session,
   before the first overwrite. Returns false only if the user refused. */
async function backupBeforeWrite(t) {
    if (t.backedUp) return true;
    let dir = t.dirHandle || backupDir;
    if (dir && !await hasPerm(dir, 'readwrite') && !await ensureWritable(dir)) dir = null;

    if (!dir) {
        if (backupOptOut) return true;
        const pick = await uiConfirm(`No backup folder is set.\n\nChoose a folder, and a timestamped copy of "${t.name}" is written there before every overwrite — or write without any backup for this session.`, { ok: 'Choose a folder', cancel: 'No backup' });
        if (!pick) { backupOptOut = true; return true; }
        dir = await chooseBackupDir();
        if (!dir) { backupOptOut = true; return true; }
    }

    setStats(`Backing up ${t.name}…`);
    const src = await tabFile(t);                       // bytes currently on disk
    const bh = await dir.getFileHandle(await freeBackupName(dir, `${t.name}.${stamp()}`), { create: true });
    const w = await bh.createWritable();
    await w.write(src);                                 // streamed by the browser, not buffered here
    await w.close();
    t.backedUp = true;
    return true;
}

async function writeToHandle(t, handle, dataToExport, columnsToExport, delim, enc) {
    startProgress();
    setStats(`Writing ${handle.name}…`);
    await new Promise(r => setTimeout(r, 10));

    /* createWritable() buffers into a swap file: the original is only
       replaced at close(), so a crash mid-write cannot truncate it. */
    const w = await handle.createWritable();
    let bytes = 0;
    try {
        await streamCSV(t, dataToExport, columnsToExport, async c => { bytes += c.size ?? c.length; await w.write(c); }, delim, enc);
        await w.close();
    } catch (err) {
        try { await w.abort(); } catch (_) { }
        endProgress();
        setStats(`Write failed: ${err.message || err}`);
        uiAlert(`Could not write "${handle.name}":\n${err.message || err}\n\nThe original file is untouched.`);
        return false;
    }
    endProgress();
    return true;
}

/* Bridge write. Chromium only streams a request body over HTTP/2, so the
   CSV is handed over as one Blob — built by the same chunked, yielding
   streamCSV, and backed by disk rather than RAM once it gets big. The
   server writes to a neighbouring temp file and renames it over the
   original, so a failure mid-write cannot truncate anything. */
/* expect: the file as checkDisk() last saw it ({size, mtime_ns}); the server compares it just
   before replacing the file and refuses with 409 if it moved — then { conflict } comes back. */
async function srvWrite(t, path, doBackup, delim, rows, enc, expect) {
    startProgress();
    setStats(`Writing ${t.name}…`);
    await new Promise(r => setTimeout(r, 10));

    const chunks = [];
    await streamCSV(t, rows || t.allData, t.headers.map((_, i) => i), c => chunks.push(c), delim, enc);

    let res, j = {};
    try {
        res = await srvFetch(srvFileUrl(path) + (doBackup ? '&backup=1' : '')
            + (expect && expect.mtime_ns ? `&expect_size=${expect.size}&expect_mtime_ns=${expect.mtime_ns}` : ''),
            { method: 'PUT', body: new Blob(chunks, { type: 'text/csv' }) });
        j = await res.json().catch(() => ({}));
    } catch (err) {
        endProgress(); setStats(`Write failed: ${err.message || err}`);
        uiAlert(`Could not write "${path}":\n${err.message || err}\n\nThe original file is untouched.`);
        return null;
    }
    endProgress();
    if (res.status === 409) { setStats(`${t.name} | ${j.error || 'changed on disk'} while it was being saved — nothing written.`); return { conflict: j.error || 'changed on disk' }; }
    if (!res.ok || !j.ok) {
        const msg = j.error || `HTTP ${res.status}`;
        setStats(`Write failed: ${msg}`);
        uiAlert(`Could not write "${path}":\n${msg}\n\nThe original file is untouched.`);
        return null;
    }
    return j;
}

/* The only save there is: overwrite the source, keeping one timestamped
   .bak per tab per session. No download — the editor writes back exactly
   where it read from; only a new name typed in the save dialog (saveAs)
   moves the tab onto another file. */
/* ---------------------------------------------------------------
   CHANGED ON DISK?
   t.stamp is the file's size + modification time when it was read or
   last written by us. Before overwriting, a different stamp means
   someone else wrote the file meanwhile (another program, a sync
   client): saving would silently throw their version away.
----------------------------------------------------------------*/
async function diskStamp(t, file, stat) {   // stat: what the read already said (tabBytes), else asked
    try {
        if (t.path) { const st = stat || await srvStat(t.path); return { size: st.size, mtime: st.mtime, mtime_ns: st.mtime_ns }; }
        if (t.handle) { const f = file && file.lastModified ? file : await t.handle.getFile(); return { size: f.size, mtime: f.lastModified / 1000 }; }
    } catch (e) { }
    return null;                          // a read-only copy: nothing to compare
}
/* true = go ahead and write; false = stop (cancelled, or reloaded instead). t.checked: the file
   as it was found here, which the server must still find when it writes (srvWrite's expect). */
async function checkDisk(t) {
    t.checked = null;
    if (!t.stamp) return true;
    if (t.afterShown) t.afterShown();     // a save right after opening: the fingerprint now (03-…)
    if (t.fpWait) await t.fpWait;
    const now = await diskStamp(t);
    if (!now) {
        return await uiConfirm(`"${t.name}" can no longer be found where it was opened.\n\nIt may have been moved, renamed or deleted. Saving writes it back there.`, { ok: 'Save anyway' });
    }
    t.checked = now;
    /* A different mtime alone proves nothing: sync clients rewrite it after
       uploading our own save (kDrive sets its server's time, truncated to the
       second — observed 2.3 s EARLIER than the write), FAT keeps even seconds.
       With the same size, the content decides: same fingerprint = our file. */
    if (now.size === t.stamp.size) {
        if (Math.abs(now.mtime - t.stamp.mtime) < 2) return true;
        try {
            if (t.stamp.fp && await fingerprint(await tabFile(t)) === t.stamp.fp) { t.stamp = { ...now, fp: t.stamp.fp }; return true; }
        } catch (e) { }
    }
    const r = await uiChoice(`"${t.name}" was changed by another program since it was opened here.\n\nOverwriting replaces that version with yours${t.path ? ' (a .bak copy of it is kept)' : ''}. Reloading shows the new version and drops your ${fmt(t.modificationsLog.length)} pending edits.`,
        { ok: 'Overwrite', danger: true, choices: ['Reload from disk'] });
    if (r === 'Reload from disk') { t.modificationsLog = []; reloadKeepingView(t); return false; }
    if (r !== true) { setStats('Save cancelled.'); return false; }
    t.backedUp = false;                   // the .bak of this session predates their version: copy that one too
    return true;
}
/* After our own write: size, mtime and the fingerprint of what is now on disk. */
async function refreshStamp(t) {
    t.stamp = await diskStamp(t);
    if (t.stamp) { try { t.stamp.fp = await fingerprint(await tabFile(t)); } catch (e) { } }
}
/* SHA-256 of the bytes — of their first, middle and last MB past 64 MB, which
   is plenty to tell our own write from someone else's same-size edit. */
async function fingerprint(blob) {                 // a Blob / File, an ArrayBuffer or a byte view
    const buf = !(blob instanceof Blob), n = buf ? blob.byteLength : blob.size, M = 1 << 20;
    const part = n <= 64 * M ? blob : new Blob([blob.slice(0, M), blob.slice(Math.floor(n / 2) - M / 2, Math.floor(n / 2) + M / 2), blob.slice(n - M)]);
    const d = await crypto.subtle.digest('SHA-256', part instanceof Blob ? await part.arrayBuffer() : part);
    return [...new Uint8Array(d)].map(b => b.toString(16).padStart(2, '0')).join('');
}

async function saveInPlace(opts) {
    /* A delimiter override is itself a change worth writing, so it bypasses
       the "nothing to save" guard. */
    const delim = (opts && opts.delim) || null, enc = (opts && opts.enc) || null;
    const t = T(); if (!t) return false;
    if (!t.loaded) { uiAlert('The file is still loading.'); return false; }
    if (!delim && !enc && !t.modificationsLog.length) { setStats(`${t.name} | Nothing to save.`); return false; }
    const wenc = writeEnc(t, enc).enc;
    if (SINGLE_BYTE.includes(wenc) && !await confirmEncodable(t, t.allData, wenc)) { setStats('Save cancelled.'); return false; }
    if ((t.path || t.handle) && !await checkDisk(t)) return false;

    if (t.path) {                        // opened through the launcher
        let j = await srvWrite(t, t.path, !t.backedUp, delim, null, enc, t.checked);
        /* Changed between the check and the write: the server wrote nothing. Ask again
           (overwrite or reload), as if it had been seen before saving. */
        for (let k = 0; j && j.conflict && k < 3; k++) {
            if (!await checkDisk(t)) return false;
            j = await srvWrite(t, t.path, !t.backedUp, delim, null, enc, t.checked);
        }
        if (j && j.conflict) uiAlert(`"${t.name}" keeps changing on disk while it is being saved, so nothing was written.\n\nClose the program that writes it, then save again.`);
        if (!j || j.conflict) return false;
        t.backedUp = true; t.size = j.size; t.modificationsLog = [];
        markPristine(t);
        await refreshStamp(t);
        updateSaveBtn(); renderTabBar();
        doneMsg(`${t.name} | Saved in place — ${fmt(t.allData.length)} ${t.lang ? 'lines' : 'rows'} written`
            + (j.backup ? ` · backup ${j.backup}` : '') + '.');
        return true;
    }

    if (!t.handle) {
        uiAlert(FSA
            ? 'This tab is read-only — it was handed a copy of the file, not the file itself.\n\nRe-open it with "Open", or from your file manager, to write into it.'
            : 'Direct writing is unavailable here: the File System Access API needs a secure origin.\n\nOpen this page over http://localhost (run ./serve.sh) in Chromium or Brave, not as a file:// page.');
        return false;
    }
    if (!await ensureWritable(t.handle)) { setStats('Write permission refused.'); uiAlert(`Write access to "${t.name}" was not granted.`); return false; }

    try { await backupBeforeWrite(t); }
    catch (err) {
        if (!await uiConfirm(`Backup failed (${err.message || err}).\nOverwrite "${t.name}" anyway?`, { ok: 'Overwrite anyway', danger: true })) { setStats('Save cancelled.'); return false; }
    }

    const allColsIdx = t.headers.map((_, i) => i);
    if (!await writeToHandle(t, t.handle, t.allData, allColsIdx, delim, enc)) return false;

    t.modificationsLog = [];
    markPristine(t);
    try { t.size = (await t.handle.getFile()).size; } catch (e) { }
    await refreshStamp(t);
    updateSaveBtn(); renderTabBar();
    doneMsg(`${t.name} | Saved in place — ${fmt(t.allData.length)} ${t.lang ? 'lines' : 'rows'} written.`);
    return true;
}

/* ---------------------------------------------------------------
   EXTRACT
   The rows shown — every row when no filter is active, so it doubles
   as "duplicate this file" — become a file of their own, opened as a new tab;
   the source is not touched, so there is nothing to back up. The new
   tab is an ordinary one (path or handle): it can be edited and saved
   in place like any other.
----------------------------------------------------------------*/
function hasFilter(t) { return t.globalQuery !== '' || Object.keys(t.colFilters).length > 0 || Object.keys(t.valFilters).length > 0 || t.onlyIrregular || !!t.onlyDups || !!(t.rowMark && t.rowMark.only); }
/* foo-3.csv → foo-4.csv, v007.csv → v008.csv (padding kept), foo.csv → foo-2.csv */
function nextName(name) {
    const m = name.match(/^(.*?)(\.[^.]*)?$/);
    const stem = m[1], ext = m[2] || '.csv';
    const d = stem.match(/^(.*?)(\d+)$/);
    if (!d) return `${stem}-2${ext}`;
    return d[1] + String(BigInt(d[2]) + 1n).padStart(d[2].length, '0') + ext;
}

async function extractFiltered() {
    const t = T(); if (!t || !t.loaded) return;
    const rows = t.filteredData;
    if (!rows.length) { uiAlert('No row to copy.'); return; }
    const delim = currentDelim(t);

    if (t.path) {
        /* Beside the source, first free name in the nextName() sequence. */
        const dir = pathDir(t.path);
        let dest = null, name = t.name;
        for (let n = 0; n < 1000 && !dest; n++) {
            name = nextName(name);
            try { await srvStat(dir + name); } catch (e) { dest = dir + name; }
        }
        if (!dest) { uiAlert('No free file name found beside the source.'); return; }
        const j = await srvWrite(t, dest, false, delim, rows);
        if (!j) return;
        doneMsg(`${t.name} | ${fmt(rows.length)} rows extracted to ${baseName(dest)}.`);
        await addPathTabs([dest]);
        return;
    }

    if (!FSA) { uiAlert('Writing a new file needs the File System Access API.\n\nOpen this page over http://localhost (run ./serve.sh) in Chromium or Brave.'); return; }
    /* The picker needs the click's user activation: call it before any other await. */
    let handle;
    try {
        handle = await showSaveFilePicker({
            suggestedName: nextName(t.name), types: CSV_TYPES,
            startIn: t.dirHandle || t.handle || 'documents'
        });
    } catch (e) { return; }                     // picker dismissed
    if (!await writeToHandle(t, handle, rows, t.headers.map((_, i) => i), delim)) return;
    doneMsg(`${t.name} | ${fmt(rows.length)} rows extracted to ${handle.name}.`);
    await addHandles([handle], null);
}

function applySearchReplace() {
    const t = T(); if (!t) return;
    const fText = document.getElementById('sr-find').value;
    const rText = document.getElementById('sr-replace').value;
    if (!fText) return;

    const colSel = document.getElementById('sr-col').value, only = colSel === '' ? -1 : +colSel;
    const cs = document.getElementById('sr-case').checked;
    let re = null;
    if (document.getElementById('sr-regex').checked) {
        try { re = new RegExp(fText, cs ? 'g' : 'gi'); }
        catch (e) { uiAlert(`This regular expression is not valid.\n\n${e.message}`); return; }
        /* Tried apart first (40-…); once checked, the replace runs by itself. */
        const g = regexGate(t, [[only < 0 ? 'g' : 'c' + only, fText, cs ? 'g' : 'gi']], () => { if (T() === t) applySearchReplace(); });
        if (g) { if (g.pending) setStats(`${t.name} | Checking the regular expression before replacing…`); else uiAlert(`This regular expression is too slow for this file.\n\nIt ${g.error.replace(/^this regular expression /, '')}.`); return; }
    }
    /* Case ignored (the default, as in the filters): the text becomes a regex, its
       replacement a function so that a $ in it stays literal. */
    const swap = re ? v => v.replace(re, rText) : cs ? v => v.replaceAll(fText, rText)
        : v => v.replace(new RegExp(fText.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'), () => rText);

    let repCount = 0;
    const before = [], ed = rowEdits();   // before: [row, column], for the flash
    /* Plain text: a record whose raw text does not hold it has no cell that does — most
       rows are then skipped without being split. */
    const raw = !re && t.base ? (cs ? fText : fText.toLowerCase()) : null;
    visitRows(t, t.filteredData, row => {
        if (raw && !row.d && row.b >= 0 && (cs ? recordText(row.base, row.b) : recordText(row.base, row.b).toLowerCase()).indexOf(raw) < 0) return;
        row.data.forEach((val, cIdx) => {
            if (only >= 0 && cIdx !== only) return;
            if (val == null || val === '') return;
            const nv = swap(String(val));
            if (nv !== String(val)) { before.push([row, cIdx]); repCount++; }
        });
    });
    for (const [row, c] of before) ed.set(row, c, swap(String(row.data[c])));

    if (repCount) flash(before);
    if (repCount) t.modificationsLog.push({ id: '-', col: '(find & replace)', old: fText, new: rText + ` — ${repCount} cells`, what: `${fmt(repCount)} cells replaced`,
        undo: () => ed.undo() });
    updateSaveBtn(); renderTabBar();
    /* The replacement stays in memory: nothing touches the disk until Save. */
    document.getElementById('sr-count').innerText = repCount
        ? `${fmt(repCount)} cells replaced — not written yet, use Save.`
        : 'No match in the filtered rows.';
    render();
    updateStats();
}

/* Two lists of fields with the same values, in the same order. */
function sameFields(a, b) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
}
