// --- Boot ---
if (!FSA) {
    setStats('Ready — read-only mode (open this page over http://localhost to write files in place).');
} else {
    idbGet('backupDir').then(d => { if (d) backupDir = d; });
}

/* --- Files handed over by the launcher (yazi, .desktop, CLI) ---
   Same poll doubles as the liveness signal server.py watches: when the
   last window goes away nothing polls any more and the server exits, so
   closing the app leaves nothing running behind it. */
if (SRV) {
    setStats('Ready — desktop app mode: files opened from your file manager land here as tabs.');
    /* A long poll: the server holds the request until a path is queued (or
       ?wait= seconds), so a file opened while the app is running lands at
       once — it used to wait for the next of the 1.5 s polls. Not awaited:
       parsing a big file must not delay the next poll. */
    const drain = async () => {
        for (;;) {
            try {
                const r = await srvFetch('/api/pending?wait=8');
                if (!r.ok) throw new Error(r.status);
                const j = await r.json();
                if (j.paths && j.paths.length) addPathTabs(j.paths);
            } catch (e) { await new Promise(res => setTimeout(res, 1500)); }   // server stopped or restarting
        }
    };
    drain();
}

// --- Drag & Drop ---
let dragCounter = 0;
/* Only a drag carrying files is a file drop: dragging a column title is not. */
const carriesFiles = e => e.dataTransfer && [...e.dataTransfer.types].includes('Files');
window.addEventListener('dragenter', (e) => { if (!carriesFiles(e)) return; e.preventDefault(); dragCounter++; document.getElementById('drop-overlay').classList.add('active'); });
window.addEventListener('dragleave', (e) => { if (!carriesFiles(e)) return; e.preventDefault(); dragCounter--; if (dragCounter === 0) document.getElementById('drop-overlay').classList.remove('active'); });
window.addEventListener('dragover', (e) => e.preventDefault());
window.addEventListener('drop', (e) => {
    if (!carriesFiles(e)) return;
    e.preventDefault(); dragCounter = 0; document.getElementById('drop-overlay').classList.remove('active');
    /* getAsFileSystemHandle() must be called before we yield to the event
       loop — the DataTransfer is emptied as soon as the handler returns —
       so collect the promises synchronously, await them afterwards. */
    if (FSA && e.dataTransfer.items && e.dataTransfer.items.length && e.dataTransfer.items[0].getAsFileSystemHandle) {
        const pending = Array.from(e.dataTransfer.items)
            .filter(i => i.kind === 'file')
            .map(i => i.getAsFileSystemHandle());
        const files = Array.from(e.dataTransfer.files);
        Promise.all(pending).then(async handles => {
            handles = handles.filter(Boolean);
            const dirs = handles.filter(h => h.kind === 'directory');
            const fileHandles = handles.filter(h => h.kind === 'file');
            if (fileHandles.length) await addHandles(fileHandles, dirs[0] || null);
            for (const d of dirs) {
                const inner = [];
                for await (const [name, h] of d.entries())
                    if (h.kind === 'file' && /\.(csv|tsv|txt|log|xlsx|xlsm|json|jsonl|ndjson|sqlite|sqlite3|db3)$/i.test(name)) inner.push(h);
                inner.sort((a, b) => a.name.localeCompare(b.name));
                if (inner.length) await addHandles(inner, d);
            }
            if (!handles.length && files.length) processFiles(files);   // e.g. dropped from an archive
        }).catch(() => { if (files.length) processFiles(files); });
    } else if (e.dataTransfer.files.length > 0) {
        processFiles(e.dataTransfer.files);
    }
});

document.getElementById('loader').onchange = (e) => { if (e.target.files.length > 0) processFiles(e.target.files); };
async function setDelimiter(v) {
    const t = T();
    if (!t) { parseDefaults.delimiter = v; refreshParseOpts(); return; }
    if (v === t.delimiter) return;
    if (isDirty(t) && !await uiConfirm('Re-reading the file with a new delimiter will discard your pending edits. Continue?', { ok: 'Re-read', danger: true })) return;
    t.delimiter = v;
    reread(t);
}

async function setEncoding(v) {
    const t = T();
    if (!t) { parseDefaults.encoding = v; refreshParseOpts(); return; }
    if (v === t.encoding) return;
    if (isDirty(t) && !await uiConfirm('Re-reading the file with another encoding will discard your pending edits. Continue?', { ok: 'Re-read', danger: true })) return;
    t.encoding = v;
    reread(t);
}

function reread(t) {
    t.detectedDelim = ''; t.detectedEol = '\n';
    t.modificationsLog = []; t.headers = []; t.syntheticHeader = false; t.sort = null;
    t.hiddenCols.clear(); t.colWidths = {}; t.colFilters = {}; t.valFilters = {}; t.dataBars = {}; t.scrollTop = 0;
    updateSaveBtn(); refreshParseOpts(); parseTab(t);
}

document.getElementById('global-search').oninput = filterSoon;
document.getElementById('use-regex').onchange = applyFilters;
document.getElementById('use-slug').onchange = applyFilters;
document.getElementById('use-reverse').onchange = applyFilters;

// --- Tab keyboard shortcuts ---
window.addEventListener('keydown', (e) => {
    if (!e.altKey || e.ctrlKey || e.metaKey || !tabs.length) return;
    const cur = tabs.findIndex(t => t.id === activeTabId);
    if (e.key === 'ArrowRight') { e.preventDefault(); activateTab(tabs[(cur + 1) % tabs.length].id); }
    else if (e.key === 'ArrowLeft') { e.preventDefault(); activateTab(tabs[(cur - 1 + tabs.length) % tabs.length].id); }
    else if (e.key >= '1' && e.key <= '9') {
        const i = parseInt(e.key, 10) - 1;
        if (tabs[i]) { e.preventDefault(); activateTab(tabs[i].id); }
    }
});

/* Ctrl+O open, Ctrl+Q quit — the browser's own meaning of both is useless here. */
window.addEventListener('keydown', (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === 'o') { e.preventDefault(); openFiles(); }
    else if (k === 'q') { e.preventDefault(); quitApp(); }
    else if (k === 'w') { e.preventDefault(); if (activeTabId != null) closeTab(activeTabId); }   // the active tab only (its own dialog if edits are pending); never the app — Chromium lets an --app window take the key
});
/* Themes: a data-theme attribute on <html> swaps the colour variables (see :root).
   Picked from the round two-colour swatch at the right end of the status bar. */
function currentTheme() { return document.documentElement.dataset.theme || 'dark'; }
function setTheme(name) {
    const h = document.documentElement;
    if (name === 'dark') delete h.dataset.theme; else h.dataset.theme = name;
    h.style.background = THEME_SWATCH[name][0];   // the pre-paint background set in viewer.htm follows the theme
    h.style.colorScheme = name === 'light' || name === 'latte' ? 'light' : 'dark';
    try { localStorage.setItem('csvfab-theme', name); } catch (e) { }
    refreshParseOpts();                       // the status bar swatch, and the theme menu if it is open
    if (T() && T().loaded) stripUpdate(T(), true);   // the strip's marks in the new palette
}

let quitting = false;
async function quitApp() {
    const dirty = tabs.filter(isDirty);
    if (dirty.length && !await uiConfirm(`${dirty.length === 1 ? `"${dirty[0].name}" has` : `${dirty.length} files have`} unsaved edits.\n\nQuitting drops them.`, { ok: 'Quit without saving', danger: true })) return;
    quitting = true;
    window.close();                    // allowed: the app window was opened with a single history entry
    setTimeout(() => { if (!window.closed) { quitting = false; uiAlert('This window cannot close itself here.\n\nClose it from your window manager.'); } }, 400);
}

window.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        const t = T(); if (!t) return;
        saveInPlace();
    }
});

/* The browser's own "Leave site?" prompt: the only thing a page may show when its window is
   closed from outside (the window manager, the title bar, Alt+F4) — no page can draw its own
   dialog there. Ctrl+Q and Ctrl+W go through quitApp() and its dialog instead. */
window.addEventListener('beforeunload', (e) => {
    if (!quitting && tabs.some(isDirty)) { e.preventDefault(); e.returnValue = ''; }
});

function removeAccents(str) { return str.normalize("NFD").replace(/[\u0300-\u036f]/g, ""); }
/* Letters NFD cannot split into base + accent. */
const SLUG_LIGATURES = { 'œ': 'oe', 'æ': 'ae', 'ß': 'ss', 'ø': 'o', 'đ': 'd', 'ł': 'l', 'ð': 'd', 'þ': 'th' };
function slugify(str) {
    return removeAccents(str.toLocaleLowerCase('fr')).replace(/[œæßøđłðþ]/g, c => SLUG_LIGATURES[c])
        .replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
}
function toggleSRBar() {
    srBar.style.display = srBar.style.display === 'flex' ? 'none' : 'flex';
    if (srBar.style.display === 'flex') { fillSRCols(); document.getElementById('sr-find').focus(); }
    resizeContainer();
    const t = T(); if (t && t.loaded) findRefresh(t);   // the find marks come and go with the bar (29-…)
}
/* The column list follows the active tab's headers. */
function fillSRCols() {
    const t = T(), sel = document.getElementById('sr-col'), prev = sel.value;
    sel.innerHTML = '<option value="">All columns</option>' + (t ? t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('') : '');
    if (prev !== '' && t && +prev < t.headers.length) sel.value = prev;
}
function resizeContainer() {
    const top = document.getElementById('chrome').offsetHeight, bottom = document.getElementById('statusbar').offsetHeight;
    container.style.height = `calc(100vh - ${top + bottom}px)`;
    /* The row card (22-row-card.js) docks on the right, between the chrome and the status bar; the grid narrows to make room. */
    const card = document.getElementById('row-card');
    container.style.width = card.classList.contains('open') ? 'calc(100vw - var(--card-w))' : '';
    card.style.top = top + 'px'; card.style.bottom = bottom + 'px';
    syncSpace(T());                       // the grid layer and the scroll extent follow the viewport
}
const chromeObserver = new ResizeObserver(resizeContainer);
chromeObserver.observe(document.getElementById('chrome'));      // the toolbar may wrap
chromeObserver.observe(document.getElementById('statusbar'));   // …and the status bar
