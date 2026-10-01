/* ---------------------------------------------------------------
   FILE SYSTEM ACCESS
   When the page runs in a secure context that exposes the File
   System Access API (Chromium over https:// or http://localhost —
   NOT file://, whose origin is opaque), opening a file yields a
   handle instead of a snapshot: the same handle re-reads fresh
   bytes on demand and, once the user grants readwrite, streams the
   edited rows straight back over the source file.
   Everything degrades to the old <input type=file> + download path
   when the API is missing.
----------------------------------------------------------------*/
const FSA = typeof window.showOpenFilePicker === 'function';
const CSV_TYPES = [{ description: 'CSV / text tables', accept: { 'text/csv': ['.csv', '.tsv', '.txt'] } }];   // .txt opens as raw text unless switched to a table (33-…)
/* Opening also takes workbooks and JSON, converted to a CSV beside them (27-import.js); saving never offers those. */
const OPEN_TYPES = CSV_TYPES.concat([{ description: 'Excel workbook / JSON / SQLite database (converted to CSV)', accept: {
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': ['.xlsx', '.xlsm'], 'application/json': ['.json', '.jsonl', '.ndjson'], 'application/vnd.sqlite3': ['.sqlite', '.sqlite3', '.db', '.db3'] } }]);
let backupDir = null;        // FileSystemDirectoryHandle used for .bak copies
let backupOptOut = false;    // user declined to pick one for this session

/* ---------------------------------------------------------------
   LOCAL BRIDGE (desktop app mode)
   Nothing can hand a page a path picked on the command line: the
   File System Access API only ever yields a handle through a picker
   or a drop. So when server.py serves this page it substitutes its
   own token below, and a tab born from an "Open with" carries
   neither handle nor File — just t.path, an absolute path the server
   reads and rewrites on its behalf. Third kind of tab, alongside
   t.handle (writable) and t.file (read-only snapshot).
----------------------------------------------------------------*/
const SRV_TOKEN = window.CSVFAB_TOKEN || '';          // set by viewer.htm, where server.py injects it
const SRV = !!SRV_TOKEN && SRV_TOKEN !== '__' + 'CSVE_TOKEN__';   // split so the server misses this one

function srvFetch(route, opts) {
    const o = opts || {};
    o.headers = Object.assign({ 'X-Csv-Token': SRV_TOKEN }, o.headers || {});
    return fetch(route, o);
}
function srvFileUrl(path) { return `/api/file?path=${encodeURIComponent(path)}`; }
async function srvStat(path) {
    const r = await srvFetch(`/api/stat?path=${encodeURIComponent(path)}`);
    if (!r.ok) throw new Error(`unreadable (HTTP ${r.status})`);
    return r.json();
}

/* Handles cannot be JSON-serialised, but they survive in IndexedDB, so the
   backup folder only has to be chosen once (re-granted with one click). */
function idbOpen() {
    return new Promise((res, rej) => {
        const r = indexedDB.open('csv-editor', 1);   // name kept from before csvfab: it keys the remembered backup folder
        r.onupgradeneeded = () => r.result.createObjectStore('kv');
        r.onsuccess = () => res(r.result);
        r.onerror = () => rej(r.error);
    });
}
async function idbGet(k) {
    try {
        const db = await idbOpen();
        return await new Promise(res => {
            const q = db.transaction('kv').objectStore('kv').get(k);
            q.onsuccess = () => res(q.result || null); q.onerror = () => res(null);
        });
    } catch (e) { return null; }
}
async function idbSet(k, v) {
    try {
        const db = await idbOpen();
        await new Promise(res => {
            const tx = db.transaction('kv', 'readwrite');
            tx.objectStore('kv').put(v, k);
            tx.oncomplete = tx.onerror = () => res();
        });
    } catch (e) { /* private mode: backup folder simply won't persist */ }
}

/* Fresh bytes for a tab: a handle re-reads from disk, a plain File is the
   snapshot taken when it was dropped / picked. */
async function tabFile(t, onProgress) {
    if (t.path) {
        const r = await srvFetch(srvFileUrl(t.path));
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const total = Number(r.headers.get('Content-Length') || 0);
        if (!onProgress || !total || !r.body) return new File([await r.blob()], t.name);
        /* Drain the body by hand so the bar moves while the bytes travel:
           the parser's own progress only starts once they are all here. */
        const reader = r.body.getReader();
        const parts = [];
        let seen = 0;
        for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            parts.push(value); seen += value.length; onProgress(seen / total);
        }
        return new File(parts, t.name);
    }
    return t.handle ? await t.handle.getFile() : t.file;
}

/* The same bytes as one ArrayBuffer, for the row store (and the File when
   there is one, for its modification time). A bridge response is read
   straight into a buffer of its Content-Length: no Blob, no second copy. */
async function tabBytes(t, onProgress) {
    if (t.path) {
        const r = await srvFetch(srvFileUrl(t.path));
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const total = Number(r.headers.get('Content-Length') || 0);
        if (!total || !r.body) return { bytes: await r.arrayBuffer(), file: null };
        const out = new Uint8Array(total), reader = r.body.getReader();
        let seen = 0;
        for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            if (seen + value.length > total) throw new Error('the file grew while it was read');
            out.set(value, seen); seen += value.length;
            if (onProgress) onProgress(seen / total);
        }
        return { bytes: seen === total ? out.buffer : out.buffer.slice(0, seen), file: null };
    }
    const file = t.handle ? await t.handle.getFile() : t.file;
    return { bytes: await file.arrayBuffer(), file };
}

async function hasPerm(handle, mode) {
    if (!handle || !handle.queryPermission) return true;
    return await handle.queryPermission({ mode }) === 'granted';
}
/* Must be called from a user gesture (a click) or Chrome rejects it. */
async function ensureWritable(handle) {
    if (!handle || !handle.requestPermission) return false;
    if (await handle.queryPermission({ mode: 'readwrite' }) === 'granted') return true;
    try { return await handle.requestPermission({ mode: 'readwrite' }) === 'granted'; }
    catch (e) { return false; }
}

/* --- Loading --- */
function addTabs(entries) {   // entries: [{name, size, file?, handle?, dirHandle?, path?}]
    if (!entries.length) return;
    /* A new file starts from the defaults (Auto unless set with no file open) — never from
       the active tab's choices: another tab read as Mac Roman must not make this one Mac Roman. */
    const cur = parseDefaults;
    const delim = cur.delimiter, hmode = cur.headerMode, encoding = cur.encoding;
    let first = null;
    entries.forEach(e => {
        const t = newTab(e);
        t.handle = e.handle || null;
        t.dirHandle = e.dirHandle || null;
        t.path = e.path || null;
        t.delimiter = e.raw || !isTableName(e.name) ? '\n' : delim; t.headerMode = hmode; t.encoding = encoding;   // not a table (a log, code, a JSON opened as text): raw text, 33-…
        tabs.push(t);
        if (!first) first = t;
    });
    renderTabBar();
    activateTab(first.id);                // only this one gets parsed now
    recordRecent(entries);
    document.getElementById('loader').value = '';
}

async function processFiles(fileList) {   // read-only path: <input type=file>, legacy drop
    const entries = [];
    for (const f of Array.from(fileList)) {
        const raw = await importAsText(f.name); if (raw === null) continue;
        if (importKind(f.name) && !raw) { const e = await importFile(f); if (e) entries.push(e); }   // a workbook or JSON: its CSV, as a copy
        else entries.push({ file: f, name: f.name, size: f.size });
    }
    addTabs(entries);
}

/* Paths queued by the launcher. An already-open path is focused rather than
   opened twice, so re-launching the same file from yazi just switches tab. */
async function addPathTabs(paths) {
    const entries = [];
    for (let p of paths) {
        const raw = await importAsText(baseName(p)); if (raw === null) continue;
        if (importKind(baseName(p)) && !raw) { p = await importPath(p); if (!p) continue; }   // a workbook or JSON: the CSV written beside it
        const known = tabs.find(t => t.path === p);
        if (known) { activateTab(known.id); continue; }
        try {
            const st = await srvStat(p);
            entries.push({ path: p, name: st.name, size: st.size });
        } catch (err) {
            setStats(`Cannot open ${p} — ${err.message || err}`);
        }
    }
    addTabs(entries);
}

async function addHandles(handles, dirHandle) {
    const entries = [];
    for (const h of handles) {
        const raw = await importAsText(h.name); if (raw === null) continue;
        if (importKind(h.name) && !raw) { const e = await importHandle(h, dirHandle); if (e) entries.push(e); continue; }
        try { const f = await h.getFile(); entries.push({ file: f, handle: h, dirHandle, name: f.name, size: f.size }); }
        catch (e) { console.warn('cannot read', h.name, e); }
    }
    addTabs(entries);
}

async function openFiles() {
    if (!FSA) { document.getElementById('loader').click(); return; }
    let handles;
    try { handles = await showOpenFilePicker({ multiple: true, types: OPEN_TYPES, excludeAcceptAllOption: false }); }
    catch (e) { return; }                       // picker dismissed
    await addHandles(handles, null);
}
