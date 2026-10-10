const ROW_H = 35;

/* ---------------------------------------------------------------
   TABS
   Only ONE table is ever mounted in the DOM: the active tab's rows
   are the only ones rendered (and only the visible slice of them).
   Each tab keeps its own state; the parsed rows of inactive tabs are
   released from RAM (LRU) and re-read from the File handle on demand.
----------------------------------------------------------------*/
let tabs = [];
let activeTabId = null;
let tabSeq = 0;
let lastCheckedCol = null;
/* Themes (palettes in the <style>), declared up here: the status bar uses them from the first render. */
const THEMES = [['dark', 'GitHub Dark'], ['tokyonight', 'Tokyo Night'], ['catppuccin', 'Catppuccin Mocha'], ['vscode', 'VS Code'],
    ['light', 'GitHub Light'], ['latte', 'Catppuccin Latte']];
const THEME_SWATCH = { dark: ['#0d1117', '#238636'], tokyonight: ['#15161e', '#7aa2f7'], catppuccin: ['#11111b', '#cba6f7'],
    vscode: ['#1a1b26', '#007acc'], light: ['#ffffff', '#0969da'], latte: ['#eff1f5', '#8839ef'] };
const swatchCss = n => `background: linear-gradient(135deg, ${THEME_SWATCH[n][0]} 50%, ${THEME_SWATCH[n][1]} 50%);`;
const flashCells = new Map();            // row object → Set of columns just changed (they flash once)
let sel = null;                           // range selection { tab, ar, ac (anchor), fr, fc (active cell) } — view row, column
let keepSel = false, selDragging = false;
let idxColW = 0;   // outer width of the row-number column, the same for every tab

const container = document.getElementById('view-container');
const tbody = document.getElementById('tbody');
const thead = document.getElementById('thead');
const progressBar = document.getElementById('progress-bar');
const srBar = document.getElementById('sr-bar');
const emptyState = document.getElementById('empty-state');

function T() { return tabs.find(t => t.id === activeTabId) || null; }
/* Bridge paths come from the server's OS: backslash-separated on Windows. */
function pathDir(p) { return p.slice(0, Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\')) + 1); }
function baseName(p) { return p.slice(pathDir(p).length); }
function isDirty(t) { return t.modificationsLog.length > 0; }
function esc(s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); }
function fmt(n) { return n.toLocaleString('fr-FR'); }

function newTab(src) {
    return {
        id: ++tabSeq, file: src.file || null, handle: null, dirHandle: null, path: null, backedUp: false,
        name: src.name, size: src.size || 0,
        loaded: false, loading: false, error: null,
        allData: [], filteredData: [], headers: [], rowCount: 0, base: null,   // base: the file's bytes and records (21-row-store)
        modificationsLog: [], redoStack: [], redoAt: 0, redoAnchor: undefined, hiddenCols: new Set(), colWidths: {},   // redoStack: the entries undone, replayable while the log's end is still redoAnchor (15-…)
        delimiter: '', detectedDelim: '', detectedEol: '\n',
        encoding: '', detectedEnc: '', bom: false,   // encoding: '' = auto
        headerMode: 'auto', syntheticHeader: false,
        globalQuery: '', colFilters: {}, valFilters: {}, dataBars: {}, totals: {}, frozen: 0, onlyIrregular: false, quoteErrors: 0, mojibake: false, dupSpec: null, onlyDups: false, rowMark: null,   // valFilters: {col: Set of EXCLUDED values}
        useRegex: false, useSlug: false, useReverse: false, useExpr: false, exprErr: '',   // useExpr: the search box is a formula (23-expr-filter)
        scrollTop: 0, lastUsed: Date.now()
    };
}

/* Once, before the next frame: renderTabBar() runs several times while a file opens, and each
   resizeContainer() read sizes, so laid the grid out again (5–9 ms each time once its rows were
   in) — the frame lays it out once. A change of the chrome's or the status bar's height is seen
   by their observer anyway (04-…), and the container's by the one that redraws the rows (15-…).
   Here, not in 04-…: renderTabBar() runs at boot from 03-…'s top level (a let there would not
   be initialised yet). */
let resizeQueued = 0;
function resizeSoon() {
    if (!resizeQueued) resizeQueued = requestAnimationFrame(() => { resizeQueued = 0; resizeContainer(); });
}

function renderTabBar() {
    const bar = document.getElementById('tabs');
    const html = tabs.map(t => {
        const cls = t.id === activeTabId ? 'active' : (t.loading ? 'loading' : (t.loaded ? '' : 'unloaded'));
        const meta = t.loading ? 'loading…' : (t.rowCount ? fmt(t.rowCount) : '');
        /* The full path on disk first (a bridge tab's; a picked file's path is never given to the page). */
        const title = esc(t.path || t.name) + '&#10;' + `${(t.size / 1048576).toFixed(1)} MB`
            + (t.handle || t.path ? '' : ' · read-only copy')
            + (t.loaded || t.loading ? '' : ' · released from RAM');
        return `<div class="tab ${cls}" onclick="activateTab(${t.id})" title="${title}">
            <span class="dot"></span>
            <span class="t-name">${esc(t.name)}</span>
            <span class="t-meta">${meta}</span>
            ${isDirty(t) ? `<span class="t-dirty" title="${t.modificationsLog.length} pending edits">●</span>` : ''}
            <span class="t-x" onclick="event.stopPropagation(); closeTab(${t.id})" title="Close tab">×</span>
        </div>`;
    }).join('');
    if (html !== bar._html) bar.innerHTML = bar._html = html;   // called several times per opening, mostly unchanged

    const loaded = tabs.filter(t => t.loaded);
    const bytes = loaded.reduce((s, t) => s + t.size, 0);
    document.getElementById('ram-txt').innerText = tabs.length
        ? `${loaded.length}/${tabs.length} in RAM ≈ ${(bytes / 1048576).toFixed(1)} MB ·` : '';
    emptyState.style.display = tabs.length ? 'none' : 'block';
    document.getElementById('grid-layer').style.display = tabs.length ? '' : 'none';
    resizeSoon();
}

function activateTab(id) {
    const t = tabs.find(x => x.id === id);
    if (!t) return;
    sel = null;
    container.classList.remove('tabfade'); void container.offsetWidth; container.classList.add('tabfade');   // replay the fade
    const prev = T();
    if (prev && prev !== t) collectUIState(prev);

    activeTabId = t.id;
    t.lastUsed = Date.now();
    restoreUIState(t);
    renderTabBar();

    if (t.loaded) {
        /* Drawn once, where the tab was: its scroll extent first (syncSpace, with the previous
           tab's rows gone — the previous extent could clamp the position), then its scroll, then
           the rows. It used to draw at the previous tab's position, scroll and draw again:
           two full redraws, ~11 ms of the ~36 a switch took. */
        renderHeader(); applyColStyles();
        tbody.innerHTML = ''; drawn = null; syncSpace(t);
        container.scrollTop = t.scrollTop; renderFirst();
        updateStats();
        if (t.filterPending) applyFilters();   // left while the filter workers were answering (44-…)
    } else {
        renderHeader(); applyColStyles(); tbody.innerHTML = '';   // drop the previous tab's rows at once
        updateStats();
        if (!t.loading) parseTab(t);
    }
    updateSaveBtn();
    evictIfNeeded();
}

async function closeTab(id) {
    const t = tabs.find(x => x.id === id);
    if (!t) return;
    if (isDirty(t) && !await uiConfirm(`"${t.name}" has ${t.modificationsLog.length} unsaved edits. Close anyway?`, { ok: 'Close without saving', danger: true })) return;
    const idx = tabs.indexOf(t);
    if (t.base) parRelease(t.base);
    t.allData = []; t.filteredData = []; t.base = null;
    tabs.splice(idx, 1);
    if (activeTabId === id) {
        activeTabId = null;
        if (tabs.length) activateTab(tabs[Math.min(idx, tabs.length - 1)].id);
        else { thead.innerHTML = ''; tbody.innerHTML = ''; applyColStyles(); updateSaveBtn(); refreshParseOpts(); updateCount(null); setStats('Ready.'); }
    }
    renderTabBar();
}

/* --- RAM management: release the least recently used clean tabs --- */
function unloadTab(t) { if (t.base) parRelease(t.base); t.allData = []; t.filteredData = []; t.base = null; t.loaded = false; t.tot = null; t.totDef = null; t.totDefWip = null; }   // tot holds rows, and rows their base

function evictIfNeeded() {
    const max = parseInt(document.getElementById('max-ram').value, 10);
    if (!max) return;
    let loadedCount = tabs.filter(t => t.loaded).length;
    const candidates = tabs
        .filter(t => t.loaded && t.id !== activeTabId && !isDirty(t))
        .sort((a, b) => a.lastUsed - b.lastUsed);
    for (const t of candidates) {
        if (loadedCount <= max) break;
        unloadTab(t); loadedCount--;
    }
    renderTabBar();
}
document.getElementById('max-ram').onchange = evictIfNeeded;

/* --- Per tab UI state --- */
function collectUIState(t) {
    t.globalQuery = document.getElementById('global-search').value;
    t.useRegex = document.getElementById('use-regex').checked;
    t.useSlug = document.getElementById('use-slug').checked;
    t.useReverse = document.getElementById('use-reverse').checked;
    t.useExpr = document.getElementById('use-expr').checked;
    t.scrollTop = container.scrollTop;
}

function restoreUIState(t) {
    document.getElementById('global-search').value = t.globalQuery;
    document.getElementById('use-regex').checked = t.useRegex;
    document.getElementById('use-slug').checked = t.useSlug;
    document.getElementById('use-reverse').checked = t.useReverse;
    document.getElementById('use-expr').checked = t.useExpr;
    updateExprUI();
    refreshParseOpts();
}
