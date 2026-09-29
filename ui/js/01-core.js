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
        modificationsLog: [], hiddenCols: new Set(), colWidths: {},
        delimiter: '', detectedDelim: '', detectedEol: '\n',
        encoding: '', detectedEnc: '', bom: false,   // encoding: '' = auto
        headerMode: 'auto', syntheticHeader: false,
        globalQuery: '', colFilters: {}, valFilters: {}, dataBars: {}, onlyIrregular: false, quoteErrors: 0, mojibake: false, dupSpec: null, onlyDups: false, rowMark: null,   // valFilters: {col: Set of EXCLUDED values}
        useRegex: false, useSlug: false, useReverse: false,
        scrollTop: 0, lastUsed: Date.now()
    };
}

function renderTabBar() {
    const bar = document.getElementById('tabs');
    bar.innerHTML = tabs.map(t => {
        const cls = t.id === activeTabId ? 'active' : (t.loading ? 'loading' : (t.loaded ? '' : 'unloaded'));
        const meta = t.loading ? 'loading…' : (t.rowCount ? fmt(t.rowCount) : '');
        const title = `${esc(t.name)} — ${(t.size / 1048576).toFixed(1)} MB`
            + (t.handle ? ' — writable (Save overwrites it)'
                : t.path ? ' — writable through the local bridge (Save overwrites it)'
                : ' — read-only copy')
            + (t.loaded ? ' — in RAM' : (t.loading ? ' — loading' : ' — released from RAM (re-read on click)'));
        return `<div class="tab ${cls}" onclick="activateTab(${t.id})" title="${title}">
            <span class="dot"></span>
            <span class="t-name">${esc(t.name)}</span>
            <span class="t-meta">${meta}</span>
            ${isDirty(t) ? `<span class="t-dirty" title="${t.modificationsLog.length} pending edits">●</span>` : ''}
            <span class="t-x" onclick="event.stopPropagation(); closeTab(${t.id})" title="Close tab">×</span>
        </div>`;
    }).join('');

    const loaded = tabs.filter(t => t.loaded);
    const bytes = loaded.reduce((s, t) => s + t.size, 0);
    document.getElementById('ram-txt').innerText = tabs.length
        ? `${loaded.length}/${tabs.length} in RAM ≈ ${(bytes / 1048576).toFixed(1)} MB ·` : '';
    emptyState.style.display = tabs.length ? 'none' : 'block';
    resizeContainer();
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
        renderHeader(); applyColStyles(); render();
        container.scrollTop = t.scrollTop; render();
        updateStats();
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
function unloadTab(t) { t.allData = []; t.filteredData = []; t.base = null; t.loaded = false; }

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
    const f = {};
    document.querySelectorAll('.f-in').forEach(i => { if (i.value) f[i.dataset.col] = i.value; });
    t.colFilters = f;
    t.scrollTop = container.scrollTop;
}

function restoreUIState(t) {
    document.getElementById('global-search').value = t.globalQuery;
    document.getElementById('use-regex').checked = t.useRegex;
    document.getElementById('use-slug').checked = t.useSlug;
    document.getElementById('use-reverse').checked = t.useReverse;
    refreshParseOpts();
}
