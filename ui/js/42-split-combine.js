/* ---------------------------------------------------------------
   SPLIT INTO FILES / COMBINE FILES
   Split: the rows shown, one file per value of a column (one per agency,
   per month…), written in a new folder beside the source — a folder of its
   own, so no file already there is ever overwritten. Each file is written
   by streamCSV like a save: same delimiter, encoding and line breaks, the
   untouched rows copied as their bytes.
   Combine: open tabs into one new file, columns matched by name (in any
   order, case and accents ignored by default), whatever each file's
   delimiter or encoding — each tab has already been decoded by its own.
   Neither touches the tabs it reads: nothing to undo.
----------------------------------------------------------------*/
const SPLIT_ASK = 100;          // files beyond which the split asks first
const SPLIT_MAX = 5000;         // …and beyond which it refuses: a column of ids, not of categories
const SPLIT_SLICE = 50000;      // rows grouped between two yields
let splitF = null;              // the split dialog: { t, col, groups, run }
let combine = null;             // the combine dialog: { first, on: Set of tab ids, info: Map(id → {rows, filtered}) }

/* A dialog's markup, added to the page the first time it opens: viewer.htm is sent at every
   load (and measured: 6 KB of dialogs made it 5 % slower to serve), these are opened now and then. */
function modalOnce(id, html) {
    if (!document.getElementById(id)) document.body.insertAdjacentHTML('beforeend', html);
}
function splitDialog() { modalOnce('modal-splitf', `<div id="modal-splitf" class="modal-box">
    <h4>Split into files</h4>
    <span class="fld-label" id="sf-where"></span>
    <div class="row2" style="margin-top: 8px;">
        <label class="fld-label" for="sf-col" style="margin: 0;">One file per value of</label>
        <select id="sf-col" class="bs-input bs-select" onchange="splitFilesRefresh()"></select>
    </div>
    <div class="opts">
        <label class="col-label"><input type="radio" name="sf-match" value="exact" checked onchange="splitFilesRefresh()"> Exact values</label>
        <label class="col-label"><input type="radio" name="sf-match" value="loose" onchange="splitFilesRefresh()"> Ignoring case and surrounding spaces</label>
        <label class="col-label" title="No accents, case or symbols"><input type="radio" name="sf-match" value="slug" onchange="splitFilesRefresh()"> Slugified</label>
    </div>
    <div class="stat-line" id="sf-stats"></div>
    <div class="pv-wrap"><table class="pv" id="sf-pv"></table></div>
    <div class="modal-actions" style="margin-top: 16px;">
        <button class="btn btn-outline" onclick="splitF = null; closeAllModals()">Cancel</button>
        <button class="btn" id="sf-go" onclick="applySplitFiles()">Write the files</button>
    </div>
</div>`); }
function combineDialog() { modalOnce('modal-combine', `<div id="modal-combine" class="modal-box">
    <h4>Combine files</h4>
    <span class="fld-label">The open tabs ticked, one after the other in one new file. Columns are matched by their title, in any order; a file without a column leaves it empty. Each file keeps its own delimiter and encoding until then.</span>
    <div class="pv-wrap"><table class="pv cb" id="cb-tabs"></table></div>
    <div class="row2" style="margin-top: 8px;">
        <button class="btn btn-outline" onclick="combineMore()" title="Or drop a whole folder on the window: each of its files opens as a tab">Open files…</button>
    </div>
    <div class="opts">
        <label class="col-label"><input type="radio" name="cb-match" value="slug" checked onchange="combineRefresh()"> Titles matched ignoring case, accents and punctuation</label>
        <label class="col-label"><input type="radio" name="cb-match" value="exact" onchange="combineRefresh()"> Titles matched exactly</label>
    </div>
    <div class="opts">
        <label class="col-label"><input type="checkbox" id="cb-common" onchange="combineRefresh()"> Only the columns every file has</label>
        <label class="col-label"><input type="checkbox" id="cb-source" onchange="combineRefresh()"> Add a column with each row's file name</label>
    </div>
    <div class="stat-line" id="cb-stats"></div>
    <div class="pv-wrap"><table class="pv cb" id="cb-pv"></table></div>
    <div class="modal-actions" style="margin-top: 16px;">
        <button class="btn btn-outline" onclick="combine = null; closeAllModals()">Cancel</button>
        <button class="btn" id="cb-go" onclick="applyCombine()">Write the combined file</button>
    </div>
</div>`); }

/* A value as a file name on every system: no / \ : * ? " < > | nor control characters,
   no trailing dot or space (Windows drops them), not too long. */
function safeFileName(s) {
    const v = String(s).replace(/[\\/:*?"<>|\u0000-\u001F\u007F]/g, '_').replace(/\s+/g, ' ').trim().replace(/[. ]+$/, '');
    return v.length > 80 ? v.slice(0, 80).trim() : v;
}

/* ---- Split ---------------------------------------------------------- */
function openSplitFiles(col) {
    const t = T(); if (!t || !t.loaded) return;
    if (!t.headers.length) return;
    splitDialog();
    const sel = document.getElementById('sf-col');
    sel.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    sel.value = col != null ? col : splitGuess(t);
    splitF = { t };
    document.getElementById('sf-where').textContent = t.path
        ? `In a new folder beside ${t.name}, one file per value — the rows shown${hasFilter(t) ? ' (filtered)' : ''}, with the title line.`
        : 'In a new folder of the folder you pick, one file per value — the rows shown, with the title line.';
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-splitf').style.display = 'block';
    splitFilesRefresh();
}
/* A column of categories: the first with 2 to 50 values over the first rows. */
function splitGuess(t) {
    const rows = t.filteredData.slice(0, 5000);
    for (let c = 0; c < t.headers.length; c++) {
        const seen = new Set();
        for (const r of rows) { seen.add(cellStr(cellOf(r, c))); if (seen.size > 50) break; }
        if (seen.size >= 2 && seen.size <= 50) return c;
    }
    return 0;
}
function splitNorm() { return LK_NORM[document.querySelector('input[name="sf-match"]:checked').value]; }

/* groups: [{ value, rows, name }] in order of first appearance; counted in slices, the newest run wins. */
async function splitFilesRefresh() {
    const S = splitF; if (!S) return;
    const t = S.t, col = +document.getElementById('sf-col').value, norm = splitNorm(), run = S.run = {};
    const stats = document.getElementById('sf-stats'), go = document.getElementById('sf-go');
    go.disabled = true; S.groups = null; S.col = col;
    stats.textContent = 'Counting the values…';
    document.getElementById('sf-pv').innerHTML = '';
    const rows = t.filteredData, map = new Map(), groups = [];
    for (let i0 = 0; i0 < rows.length; i0 += SPLIT_SLICE) {
        visitRows(t, rows.slice(i0, i0 + SPLIT_SLICE), r => {
            const v = cellStr(r.data[col]), k = norm(v);
            let g = map.get(k);
            if (!g) { map.set(k, g = { value: v, rows: [] }); groups.push(g); }
            g.rows.push(r);
        });
        if (rows.length > SPLIT_SLICE) { stats.textContent = `Counting the values… ${Math.round(Math.min(1, (i0 + SPLIT_SLICE) / rows.length) * 100)} %`; await new Promise(r => setTimeout(r, 0)); }
        if (splitF !== S || S.run !== run) return;
    }
    /* File names: the value made safe; two values giving one name (Paris / PARIS on a disk that
       ignores case, a / and a _) get (2), (3)… rather than one overwriting the other. */
    const stem = safeFileName(stemOf(t.name)) || 'part', ext = csvExt(t), used = new Set();
    for (const g of groups) {
        const base = `${stem} - ${safeFileName(g.value) || '(empty)'}`;
        let name = base + ext;
        for (let n = 2; used.has(name.toLowerCase()); n++) name = `${base} (${n})${ext}`;
        used.add(name.toLowerCase()); g.name = name;
    }
    S.groups = groups;
    const n = groups.length;
    stats.innerHTML = n > SPLIT_MAX
        ? `<span class="warn"><b>${fmt(n)}</b> different values: more than ${fmt(SPLIT_MAX)} files. Pick a column of categories (a city, a status, a month), not of identifiers.</span>`
        : `<b>${fmt(n)}</b> file${n === 1 ? '' : 's'} from ${fmt(rows.length)} ${hasFilter(t) ? 'filtered ' : ''}rows`;
    document.getElementById('sf-pv').innerHTML = '<tr><th>Value</th><th class="k">Rows</th><th>File</th></tr>'
        + groups.slice(0, 10).map(g => `<tr><td>${g.value === '' ? '<span class="muted">empty</span>' : esc(g.value)}</td><td class="k">${fmt(g.rows.length)}</td><td>${esc(g.name)}</td></tr>`).join('')
        + (n > 10 ? `<tr><td colspan="3" class="muted">…and ${fmt(n - 10)} more</td></tr>` : '');
    go.disabled = !n || n > SPLIT_MAX;
}

async function applySplitFiles() {
    const S = splitF; if (!S || !S.groups) return;
    const t = S.t, groups = S.groups, col = S.col, delim = currentDelim(t), cols = t.headers.map((_, i) => i);
    const folder = `${safeFileName(stemOf(t.name)) || 'split'} - by ${safeFileName(t.headers[col]) || 'column ' + (col + 1)}`;
    if (!t.path && !FSA) { uiAlert('Writing new files needs the File System Access API.\n\nOpen this page over http://localhost (run ./serve.sh) in Chromium or Brave.'); return; }
    /* The folder picker needs the click's user activation: before any other await. */
    let parent = null;
    if (!t.path) {
        try { parent = await showDirectoryPicker({ id: 'split', mode: 'readwrite', startIn: t.dirHandle || t.handle || 'documents' }); }
        catch (e) { return; }
    }
    if (groups.length > SPLIT_ASK && !await uiConfirm(`Write ${fmt(groups.length)} files?\n\nOne per value of “${t.headers[col]}”, in the folder “${folder}”.`, { ok: `Write ${fmt(groups.length)} files` })) return;
    closeAllModals();
    splitF = null;
    let where, written = 0;
    try {
        if (t.path) {
            const dir = pathDir(t.path);
            for (let n = 1; !where && n < 1000; n++) {
                const p = dir + folder + (n > 1 ? ` (${n})` : '');
                const r = await srvFetch(`/api/mkdir?path=${encodeURIComponent(p)}`, { method: 'POST' });
                if (r.ok) where = p; else if (r.status !== 409) throw new Error((await r.json().catch(() => ({}))).error || `HTTP ${r.status}`);
            }
            if (!where) throw new Error('no free folder name beside the file');
            const sep = pathDir(t.path).slice(-1);         // the server's own separator
            startProgress();
            for (const g of groups) {
                setStats(`${t.name} | Writing ${fmt(written + 1)} of ${fmt(groups.length)}: ${g.name}…`);
                const chunks = [];
                await streamCSV(t, g.rows, cols, c => chunks.push(c), delim);
                const r = await srvFetch(srvFileUrl(where + sep + g.name), { method: 'PUT', body: new Blob(chunks, { type: 'text/csv' }) });
                if (!r.ok) throw new Error(`${g.name}: ${(await r.json().catch(() => ({}))).error || 'HTTP ' + r.status}`);
                written++;
            }
            endProgress();
        } else {
            let dir = null;
            for (let n = 1; !dir && n < 1000; n++) {
                const name = folder + (n > 1 ? ` (${n})` : '');
                try { await parent.getDirectoryHandle(name); } catch (e) { if (e.name === 'NotFoundError') { dir = await parent.getDirectoryHandle(name, { create: true }); where = name; } else throw e; }
            }
            if (!dir) throw new Error('no free folder name there');
            for (const g of groups) {
                const fh = await dir.getFileHandle(g.name, { create: true });
                if (!await writeToHandle(t, fh, g.rows, cols, delim)) return;
                written++;
            }
        }
    } catch (err) {
        endProgress();
        uiAlert(`The split stopped: ${err.message || err}\n\n${fmt(written)} of ${fmt(groups.length)} files were written${where ? ` in “${baseName(where)}”` : ''}. ${t.name} itself is untouched.`);
        return;
    }
    doneMsg(`${t.name} | ${fmt(written)} files written in the folder “${baseName(where)}”, one per value of ${t.headers[col]}.`);
}

/* ---- Combine -------------------------------------------------------- */
const combinable = x => x.delimiter !== '\n' && !(x.base && x.base.lines);
function openCombine() {
    const t = T(); if (!t || !t.loaded) return;
    combineDialog();
    combine = { first: t, on: new Set(tabs.filter(combinable).map(x => x.id)), info: new Map(), run: null };
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-combine').style.display = 'block';
    combineRefresh();
}
function combineToggle(id, on) { if (on) combine.on.add(id); else combine.on.delete(id); combineRefresh(); }
async function combineMore() {
    const before = new Set(tabs.map(x => x.id));
    await openFiles();
    if (!combine) return;
    for (const x of tabs) if (!before.has(x.id) && combinable(x)) combine.on.add(x.id);
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-combine').style.display = 'block';
    combineRefresh();
}
/* The tabs in their order, the first one (the active tab when the dialog opened) leading:
   its columns, delimiter and line breaks shape the result. */
function combineTabs() {
    const C = combine, list = tabs.filter(x => C.on.has(x.id) && combinable(x));
    return list.includes(C.first) ? [C.first, ...list.filter(x => x !== C.first)] : list;
}
/* The rows a tab gives: those shown when it is filtered, else every row (read back if released). */
async function combineRows(x) { return x.loaded && hasFilter(x) ? x.filteredData : await tabRows(x); }

/* out: [{ name, from: Map(tab id → column) }]. Names matched as written, or by their slug (case,
   accents, spaces and punctuation ignored); a name twice in one file matches its own rank in the
   others. A file without a title line lines up by position, and so does everything when the
   first one has none. */
function combineColumns(list, loose, onlyCommon) {
    const key = loose ? h => slugify(String(h)) || String(h) : h => String(h);
    const out = [], byKey = new Map(), pos = !!list[0].syntheticHeader;
    for (const x of list) {
        const rank = new Map();
        x.headers.forEach((h, c) => {
            if (pos || x.syntheticHeader) {
                if (!out[c]) out[c] = { name: list[0].syntheticHeader ? String(c + 1) : String(h), from: new Map() };
                out[c].from.set(x.id, c);
                return;
            }
            const k0 = key(h), r = (rank.get(k0) || 0) + 1, k = k0 + '\u0001' + r;
            rank.set(k0, r);
            let o = byKey.get(k);
            if (!o) { o = { name: String(h), from: new Map() }; byKey.set(k, o); out.push(o); }
            o.from.set(x.id, c);
        });
    }
    const all = out.filter(Boolean);
    return onlyCommon ? all.filter(o => o.from.size === list.length) : all;
}
/* What the result is written in: the first file's delimiter, line breaks and encoding — or
   UTF-8 when the files do not share one, since UTF-8 holds every character of all of them. */
function combineEnc(list) {
    const e = currentEnc(list[0]);
    return list.every(x => currentEnc(x) === e) ? writeEnc(list[0]) : { enc: 'utf-8', bom: !!list[0].bom && currentEnc(list[0]) === 'utf-8' };
}

async function combineRefresh() {
    const C = combine; if (!C) return;
    const run = C.run = {}, list = combineTabs();
    const tl = document.getElementById('cb-tabs'), stats = document.getElementById('cb-stats'), pv = document.getElementById('cb-pv'), go = document.getElementById('cb-go');
    go.disabled = true;
    const tabLine = x => {
        const i = C.info.get(x.id);
        const what = !i ? '<span class="muted">reading…</span>' : i.error ? `<span class="warn">${esc(i.error)}</span>`
            : `${fmt(i.rows.length)} row${i.rows.length === 1 ? '' : 's'}${i.filtered ? ' shown (filtered)' : ''} · ${fmt(x.headers.length)} columns · ${esc(delimName(currentDelim(x)))} · ${esc(encName(currentEnc(x)))}`;
        return `<tr><td><label><input type="checkbox"${C.on.has(x.id) ? ' checked' : ''} onchange="combineToggle(${x.id}, this.checked)"> ${esc(x.name)}</label>${x === C.first ? ' <span class="muted">(first: its layout leads)</span>' : ''}</td><td>${C.on.has(x.id) ? what : ''}</td></tr>`;
    };
    const draw = () => { tl.innerHTML = [C.first, ...tabs.filter(x => x !== C.first && combinable(x))].map(tabLine).join(''); };
    draw();
    for (const x of list) {
        if (C.info.has(x.id) && C.info.get(x.id).rows === (x.loaded && hasFilter(x) ? x.filteredData : x.allData)) continue;
        const rows = await combineRows(x);
        if (combine !== C || C.run !== run) return;
        C.info.set(x.id, rows ? { rows, filtered: x.loaded && hasFilter(x) } : { error: 'cannot be read' });
        draw();
    }
    const ok = list.filter(x => C.info.get(x.id) && C.info.get(x.id).rows);
    if (ok.length < 2) {
        stats.innerHTML = 'Tick at least two files. Open the others first (Open files…, or drop a whole folder on the window): every open tab is listed here.';
        pv.innerHTML = ''; return;
    }
    const loose = document.querySelector('input[name="cb-match"]:checked').value === 'slug';
    const common = document.getElementById('cb-common').checked, source = document.getElementById('cb-source').checked;
    const out = combineColumns(ok, loose, common), we = combineEnc(ok);
    C.plan = { list: ok, out, we, source };
    const rows = ok.reduce((a, x) => a + C.info.get(x.id).rows.length, 0);
    stats.innerHTML = `<b>${fmt(rows)}</b> rows from ${fmt(ok.length)} files → <b>${fmt(out.length + (source ? 1 : 0))}</b> columns, written as ${esc(delimName(currentDelim(ok[0])))} · ${esc(encName(we.enc))}`
        + (ok[0].syntheticHeader ? ' · <span class="muted">no title line in the first file: columns matched by position</span>' : '');
    pv.innerHTML = '<tr><th>Column</th><th>Found in</th></tr>' + out.map(o => {
        const miss = ok.filter(x => !o.from.has(x.id));
        const names = ok.map(x => o.from.has(x.id) ? x.headers[o.from.get(x.id)] : null).filter(h => h != null && h !== o.name);
        return `<tr><td>${esc(o.name)}${names.length ? ` <span class="muted">= ${[...new Set(names)].map(esc).join(', ')}</span>` : ''}</td><td>${miss.length ? `<span class="warn">${fmt(ok.length - miss.length)} of ${fmt(ok.length)}</span> <span class="muted">— empty for ${miss.map(x => esc(x.name)).join(', ')}</span>` : `all ${fmt(ok.length)}`}</td></tr>`;
    }).join('') + (source ? '<tr><td><i>source file</i></td><td class="muted">the name of the file each row comes from</td></tr>' : '');
    go.disabled = !out.length;
}
function delimName(d) { return d === '\t' ? 'tab' : d === ';' ? 'semicolon' : d === ',' ? 'comma' : d === '|' ? 'pipe' : `“${d}”`; }

async function applyCombine() {
    const C = combine; if (!C || !C.plan) return;
    const { list, out, we, source } = C.plan, first = list[0], delim = currentDelim(first), eol = first.detectedEol || '\n';
    const stem = stemOf(first.name), ext = csvExt(first);
    /* The save picker needs the click's user activation: before any other await. */
    let handle = null, dest = null;
    if (!first.path) {
        if (!FSA) { uiAlert('Writing a new file needs the File System Access API.\n\nOpen this page over http://localhost (run ./serve.sh) in Chromium or Brave.'); return; }
        try { handle = await showSaveFilePicker({ suggestedName: `${stem} - combined${ext}`, types: CSV_TYPES, startIn: first.dirHandle || first.handle || 'documents' }); }
        catch (e) { return; }
    } else {
        const dir = pathDir(first.path);
        for (let n = 1; n < 1000 && !dest; n++) {
            const p = dir + `${stem} - combined${n > 1 ? ` (${n})` : ''}${ext}`;
            try { await srvStat(p); } catch (e) { dest = p; }
        }
        if (!dest) { uiAlert('No free file name found beside the first file.'); return; }
    }
    closeAllModals();
    combine = null;
    const encode = encoderFor(we.enc);
    const quote = v => { const s = v == null ? '' : String(v); return (s.includes(delim) || s.includes('"') || s.includes('\n') || s.includes('\r')) ? `"${s.replace(/"/g, '""')}"` : s; };
    let srcName = 'source file';
    while (out.some(o => o.name === srcName)) srcName += ' ';
    const total = list.reduce((a, x) => a + C.info.get(x.id).rows.length, 0);
    /* Written slice by slice to a sink, as streamCSV does: never the whole file as one string. */
    const write = async sink => {
        if (we.bom) await sink(bomFor(we.enc));
        if (!first.syntheticHeader) await sink(encode([...out.map(o => o.name), ...(source ? [srcName] : [])].map(quote).join(delim) + eol));
        let done = 0, lastYield = performance.now();
        for (const x of list) {
            const rows = C.info.get(x.id).rows, at = out.map(o => o.from.has(x.id) ? o.from.get(x.id) : -1), tail = source ? delim + quote(x.name) : '';
            for (let i = 0; i < rows.length; i += 10000) {
                let str = '';
                visitRows(x, rows.slice(i, i + 10000), r => { const d = r.data; str += at.map(c => c < 0 ? '' : quote(d[c])).join(delim) + tail + eol; });
                await sink(encode(str));
                done += Math.min(10000, rows.length - i);
                if (performance.now() - lastYield > 30) { setProgress(done / total); await new Promise(r => setTimeout(r, 0)); lastYield = performance.now(); }
            }
        }
    };
    startProgress();
    setStats(`Combining ${fmt(list.length)} files…`);
    try {
        if (dest) {
            const chunks = [];
            await write(c => chunks.push(c));
            const r = await srvFetch(srvFileUrl(dest), { method: 'PUT', body: new Blob(chunks, { type: 'text/csv' }) });
            if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || `HTTP ${r.status}`);
        } else {
            const w = await handle.createWritable();
            try { await write(c => w.write(c)); await w.close(); } catch (e) { try { await w.abort(); } catch (_) { } throw e; }
        }
    } catch (err) {
        endProgress();
        uiAlert(`Could not write the combined file:\n${err.message || err}\n\nThe files combined are untouched.`);
        return;
    }
    endProgress();
    doneMsg(`${first.name} | ${fmt(total)} rows of ${fmt(list.length)} files combined into ${dest ? baseName(dest) : handle.name}.`);
    if (dest) await addPathTabs([dest]); else await addHandles([handle], null);
}
