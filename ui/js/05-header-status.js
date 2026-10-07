/* ---------------------------------------------------------------
   HEADER DETECTION
   Some CSV files start straight with data (amounts, dates, ids) and
   have no header line at all. Consuming that line as a header would
   silently drop a record, so we detect it and generate a numbered
   header (0, 1, 2 ...) instead, keeping every line as data.
   The result is always overridable through the "Header" selector.
----------------------------------------------------------------*/
function isNumericLike(v) {
    const s = String(v == null ? '' : v).trim();
    if (!s) return false;
    const c = s.replace(/[\s\u00a0]/g, '').replace(/^[+-]/, '').replace(/[€$£%]/g, '');
    if (!c) return false;
    return /^\d+(?:[.,]\d+)*$/.test(c) || /^\d+(?:[.,]\d+)?[eE][+-]?\d+$/.test(c);
}
function isDateLike(v) {
    const s = String(v == null ? '' : v).trim();
    return /^\d{1,4}[-\/.]\d{1,2}[-\/.]\d{1,4}(?:[ T]\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$/.test(s);   // ISO timestamps too: 2025-12-10T17:26:15.000Z
}

function cellType(v) {
    const s = String(v == null ? '' : v).trim();
    if (!s) return '';
    /* A number or a date starts with a digit, a sign, a decimal mark or a currency:
       anything else is text at once, without the heavier tests (12 M cells: 1.7 s → 0.4 s). */
    if (!/^[\d+\-.,€$£]/.test(s)) return 't';
    if (isNumericLike(s)) return 'n';
    if (isDateLike(s)) return 'd';
    return 't';
}

/* Dominant type of column i across the sample rows (null if no clear majority) */
function columnConsensus(sample, i) {
    const counts = {}; let total = 0;
    for (const row of sample) {
        const ty = cellType(row[i]);
        if (!ty) continue;
        counts[ty] = (counts[ty] || 0) + 1; total++;
    }
    if (!total) return null;
    let best = null;
    for (const k in counts) if (best === null || counts[k] > counts[best]) best = k;
    return counts[best] / total >= 0.6 ? best : null;
}

/* true => row0 is a real header line ; false => it is already data.

   Primary rule, always applied: compare the TYPE of every cell of line 1
   (text / number / date) with the type of the same column on the next
   HEADER_SAMPLE lines. A label sitting on top of a column of numbers or
   dates is a header; a number or a date sitting on line 1 is a record.
   The other heuristics only break ties when types say nothing (a file
   that is text everywhere). */
const HEADER_SAMPLE = 5;

function looksLikeHeader(row0, sample) {
    if (!row0 || !row0.length) return true;
    const cells = row0.map(c => String(c == null ? '' : c).trim());
    const filled = cells.filter(c => c !== '');
    if (!filled.length) return false;                          // empty line
    sample = (sample || []).slice(0, HEADER_SAMPLE);

    if (!sample.length) {                                      // single line file: shape only
        const numeric = filled.filter(c => cellType(c) !== 't').length;
        return numeric / filled.length <= 0.34;
    }

    let headerVotes = 0, dataVotes = 0;
    for (let i = 0; i < cells.length; i++) {
        const ty = cellType(cells[i]), cons = columnConsensus(sample, i);
        if (!ty || !cons) continue;
        if (ty === 't' && cons !== 't') headerVotes++;   // label above a column of numbers/dates
        else if (ty !== 't') dataVotes++;                // an amount/date on line 1 => a record
    }
    if (headerVotes || dataVotes) return headerVotes >= dataVotes;

    /* Types are identical everywhere (text above text): fall back on shape. */
    if (filled.length * 2 < cells.length) return false;                                // mostly empty
    if (new Set(filled.map(c => c.toLowerCase())).size < filled.length) return false;  // duplicate labels
    if (filled.some(c => c.length > 60)) return false;                                 // sentences, not labels
    for (let i = 0; i < cells.length; i++) {          // a label rarely repeats as a value of its own column
        if (!cells[i]) continue;
        const v = cells[i].toLowerCase();
        if (sample.some(r => String(r[i] == null ? '' : r[i]).trim().toLowerCase() === v)) return false;
    }
    return true;
}

function numberedHeaders(n) { return Array.from({ length: n }, (_, i) => String(i)); }

/* What the current mode asks for on this tab: true => generated 0,1,2… header */
function wantsSynthetic(t, mode) {
    if (t.lang) return true;                  // a text file: line 1 is a line
    if (mode === 'first') return false;
    if (mode === 'index') return true;
    const firstLine = t.syntheticHeader ? (t.allData[0] ? t.allData[0].data : []) : t.headers;
    const from = t.syntheticHeader ? 1 : 0;
    return !looksLikeHeader(firstLine, t.allData.slice(from, from + HEADER_SAMPLE).map(r => r.data));
}

/* Switch a loaded tab between "first line is the header" and a generated
   0,1,2… header, in place: no re-read, the first line is never lost. */
function convertHeader(t, synth) {
    if (!t.loaded || synth === t.syntheticHeader) return false;
    if (synth) {                                   // give the header line back to the data
        const hdr = t.headers.slice();
        const row = newRow(t, hdr); row.src = t.headerSrc;
        t.allData.unshift(row);
        t.headerSrc = null;
        t.headers = numberedHeaders(hdr.length);
        t.modificationsLog.forEach(l => { if (typeof l.id === 'number') l.id++; });
    } else {                                       // promote the first data line to header
        const row = t.allData.shift();
        if (row) t.headers = row.data.slice();
        t.headerSrc = row ? row.src : null;
        t.modificationsLog.forEach(l => { if (typeof l.id === 'number') l.id--; });
    }
    t.allData.forEach((r, i) => r.id = i + 1);
    t.syntheticHeader = synth;
    t.rowCount = t.allData.length;
    return true;
}

function applyHeaderMode(mode) {
    const t = T();
    if (!t) { parseDefaults.headerMode = mode; refreshParseOpts(); return; }
    t.headerMode = mode;
    refreshParseOpts();
    if (!t.loaded) return;
    convertHeader(t, wantsSynthetic(t, mode));
    refreshParseOpts();                   // the status bar shows the outcome: titles or not
    renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
}

/* Delimiter and header share one dropdown to save toolbar room: the button
   shows a summary, the menu one group per setting with the active choice
   checked on the right. */
const PARSE_DELIMS = [['', 'Auto'], [';', 'Semicolon'], [',', 'Comma'], ['\t', 'Tab'], ['|', 'Pipe'], ['\n', 'Raw text — one line per row']];   // \n: a text file (33-…), no delimiter looked for
const PARSE_HEADERS = [['auto', 'Auto'], ['first', 'Titles on first line'], ['index', 'No titles (numbered)']];
const parseDefaults = { delimiter: '', headerMode: 'auto', encoding: '' };   // used while no tab is open
function delimShown(d) { return `<span class="dk">${d === '\t' ? '\\t' : esc(d)}</span>`; }   // HTML
const CHECK_SVG = '<svg class="chk" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m3 8.5 3.5 3.5L13 4.5"/></svg>';

/* The reading options live in the status bar: one pill per setting, each
   opening its own menu above it; the line-ending pill toggles instead. */
function refreshParseOpts() {
    renderStatusFormat(T());
    const m = document.getElementById('sb-menu');
    if (m.classList.contains('open')) m.innerHTML = sbMenuHtml(m.dataset.kind);   // a pick re-renders the open menu
}

/* Each choice carries a framed badge on the right, all the same width:
   in the setting's own colour (as in the status bar) when it is the one
   in use, plain otherwise. */
const ENC_BADGE = { 'utf-8': ['UTF-8', 'e-utf8'], 'windows-1252': ['1252', 'e-1252'], 'iso-8859-1': ['8859-1', 'e-1252'],
    'iso-8859-15': ['8859-15', 'e-1252'], 'macintosh': ['MAC', 'e-mac'], 'utf-16le': ['UTF-16', 'e-utf16'] };
const DELIM_BADGE = { '': ['auto', 'c-auto'], ';': [';', 'd-semi'], ',': [',', 'd-comma'], '\t': ['⇥', 'd-tab'], '|': ['|', 'd-other'], '\n': ['¶', 'd-raw'] };
const HEAD_BADGE = { auto: ['auto', 'c-auto'], first: ['L1', 'c-head'], index: ['0 1 2', 'c-head'] };
function sbMenuHtml(kind) {
    const t = T(), s = t || parseDefaults;
    const item = (k, i, label, on, [badge, cls]) =>
        `<div class="dd-item sb-opt${on ? ' on' : ''}" onclick="pickParse('${k}', ${i})"><span>${esc(label)}</span><span class="sb-badge${on ? ' ' + cls : ''}">${esc(badge)}</span></div>`;
    if (kind === 'd') {
        const det = t && t.detectedDelim ? { ';': 'semicolon', ',': 'comma', '\t': 'tab', '|': 'pipe', '\n': 'raw text' }[t.detectedDelim] || t.detectedDelim : '';
        return '<div class="dd-head">Delimiter — re-reads the file</div>'
            + PARSE_DELIMS.map(([v, l], i) => item('d', i, v === '' && det ? `Auto (${det})` : l, v === s.delimiter, DELIM_BADGE[v])).join('');
    }
    if (kind === 'e') {
        const det = t && t.detectedEnc ? encName(t.detectedEnc) + (t.bom && t.detectedEnc === 'utf-8' ? ' BOM' : '') : '';
        return '<div class="dd-head">Encoding — re-reads the file</div>'
            + item('e', -1, det ? `Auto (${det})` : 'Auto', s.encoding === '', ['auto', 'c-auto'])
            + ENCODINGS.map(([v, l], i) => item('e', i, l, v === s.encoding, ENC_BADGE[v])).join('');
    }
    if (kind === 't') return '<div class="dd-head">Theme</div>' + THEMES.map(([id, label]) =>
        `<div class="dd-item theme-opt${id === currentTheme() ? ' on' : ''}" onclick="setTheme('${id}')"><span class="lbl"><span class="swatch" style="${swatchCss(id)}"></span>${esc(label)}</span>${CHECK_SVG}</div>`).join('');
    return '<div class="dd-head">Line 1</div>' + PARSE_HEADERS.map(([v, l], i) => item('h', i, l, v === s.headerMode, HEAD_BADGE[v])).join('');
}

function openSbMenu(e, kind) {
    e.stopPropagation();
    const m = document.getElementById('sb-menu'), pill = e.currentTarget;
    const again = m.classList.contains('open') && m.dataset.kind === kind;
    closeDDs();
    if (again) return;
    m.dataset.kind = kind;
    m.innerHTML = sbMenuHtml(kind);
    m.classList.add('open'); pill.classList.add('open');
    const r = pill.getBoundingClientRect(), w = m.offsetWidth;
    m.style.left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8)) + 'px';
    m.style.bottom = (window.innerHeight - r.top + 6) + 'px';
}

/* Line endings are a write setting, not a read one: flipping them is an
   edit (Save turns yellow, Ctrl+Z reverts) and the next save uses them. */
function toggleEol(e) {
    e.stopPropagation();
    const t = T(); if (!t || !t.loaded) return;
    const old = t.detectedEol, eol = old === '\r\n' ? '\n' : '\r\n', name = eol === '\r\n' ? 'CRLF' : 'LF';
    t.detectedEol = eol;
    t.modificationsLog.push({ id: '-', col: '---', old: 'line endings', new: name, what: `line endings set to ${name}`, undo: t => { t.detectedEol = old; } });
    updateSaveBtn(); renderTabBar(); refreshParseOpts();
    setStats(`${t.name} | Line endings set to ${name} — written at the next Save.`);
}

/* The status bar's format pills: what the active tab is read with — or,
   with no file loaded, the defaults the next one will be read with. */
const DELIM_PILL = { ';': ['d-semi', 'Semicolon'], ',': ['d-comma', 'Comma'], '\t': ['d-tab', 'Tab'], '|': ['d-other', 'Pipe'], '\n': ['d-raw', 'Raw text'] };
function renderStatusFormat(t) {
    const box = document.getElementById('sb-fmt');
    const s = t || parseDefaults, loaded = !!(t && t.loaded);
    const autoTag = on => on ? '<span class="auto">auto</span>' : '';
    const dl = s.delimiter || (loaded && t.detectedDelim) || '';
    const [dcls, dname0] = dl ? (DELIM_PILL[dl] || ['d-other', 'Delimiter']) : ['neutral', 'Delimiter'];
    const text = dl === '\n', dname = text && loaded && t.lang ? `Raw text · ${esc(t.lang.label)}` : dname0;
    textPos(t);
    const enc = s.encoding || (loaded && t.detectedEnc) || '';
    const ecls = !enc ? '' : enc === 'macintosh' ? 'e-mac' : SINGLE_BYTE.includes(enc) ? 'e-1252' : enc.startsWith('utf-16') ? 'e-utf16' : 'e-utf8';
    const head = loaded ? (t.syntheticHeader ? 'No titles' : 'Titles on line 1')
        : { auto: 'Titles', first: 'Titles on line 1', index: 'No titles' }[s.headerMode];
    const pill = (cls, html, title, click) => `<span class="sb-pill ${cls}" onclick="${click}" title="${esc(title)}">${html}</span>`;
    box.innerHTML = '<span class="sb-sep"></span>'
        + pill(dcls, `<span class="cap">${!dl ? '?' : dl === '\t' ? '⇥' : text ? '¶' : esc(dl)}</span>${dname}${autoTag(!s.delimiter)}`, text ? 'Raw text · or read as a table' : 'Delimiter', "openSbMenu(event, 'd')")
        + pill(ecls, `${enc ? esc(encName(enc)) + (loaded && t.bom && enc === 'utf-8' ? ' BOM' : '') : 'Encoding'}${autoTag(!s.encoding)}`, 'Encoding', "openSbMenu(event, 'e')")
        + (text ? '' : pill('', `${head}${autoTag(s.headerMode === 'auto')}`, 'Header line', "openSbMenu(event, 'h')"))
        + (text && loaded && (t.path || t.handle) ? pill(t.tail ? 'd-live' + (t.tailPaused ? ' paused' : '') : '', t.tail ? (t.tailPaused === 'edits' ? 'Paused' : 'Live') : 'Follow',
            t.tail ? (t.tailPaused === 'edits' ? 'Following paused while edits are pending · click: stop' : 'New lines appear as they are written · click: stop') : 'Follow the end of the file, as tail -f', 'toggleTail(event)') : '')
        + (loaded ? pill('', t.detectedEol === '\r\n' ? 'CRLF' : t.detectedEol === '\r' ? 'CR' : 'LF',
            'Line endings · click: switch', 'toggleEol(event)') : '')
        + `<span class="sb-spk${spkOn() ? '' : ' off'}" onclick="toggleSparklines()" title="Column charts ${spkOn() ? 'on' : 'off'}"></span>`
        + `<span class="sb-kcol${typeColorsOn() ? '' : ' off'}" onclick="toggleTypeColors()" title="Type colours ${typeColorsOn() ? 'on' : 'off'}"></span>`
        + `<span class="sb-theme" onclick="openSbMenu(event, 't')" title="Theme: ${esc((THEMES.find(x => x[0] === currentTheme()) || [0, ''])[1])}" style="${swatchCss(currentTheme())}"></span>`;
}

/* Numbers and dates coloured in the grid, on unless switched off here: a class on <html>
   (set before the first paint by viewer.htm), so the switch redraws nothing. */
function typeColorsOn() { return !document.documentElement.classList.contains('no-kcol'); }
function toggleTypeColors() {
    const on = !typeColorsOn();
    document.documentElement.classList.toggle('no-kcol', !on);
    try { localStorage.setItem('csvfab-type-colors', on ? '1' : '0'); } catch (e) { }
    renderStatusFormat(T());
}

/* Toolbar dropdowns (.dd): a button followed by its .dd-menu. One open at a time. */
function closeDDs() {
    document.querySelectorAll('.dd-menu.open').forEach(m => { m.classList.remove('open'); m.previousElementSibling.classList.remove('open'); });
    document.querySelectorAll('.sb-pill.open, .sb-theme.open, .row-btn.open').forEach(p => p.classList.remove('open'));
}
function toggleDD(e, btn) {
    e.stopPropagation();
    const menu = btn.nextElementSibling, open = !menu.classList.contains('open');
    closeDDs();
    menu.classList.toggle('open', open);
    btn.classList.toggle('open', open);
}
function pickParse(kind, i) {
    closeDDs();
    if (kind === 'd') setDelimiter(PARSE_DELIMS[i][0]);
    else if (kind === 'e') setEncoding(i < 0 ? '' : ENCODINGS[i][0]);
    else applyHeaderMode(PARSE_HEADERS[i][0]);
}
document.addEventListener('click', (e) => { if (!e.target.closest('.dd')) closeDDs(); });
refreshParseOpts();

// --- Header & Columns Management ---
/* Each column's kind — numbers, dates or text — from its first 400 rows
   (≥ 90 % of the non-empty cells, as for sorting), shown as an icon. */
function columnKinds(t) {
    const c = t.kindsCache;
    if (t.lang) return [''];                  // a text file: lines, not numbers nor dates
    if (c && sameStamp(c.stamp, dataStamp(t))) return c.kinds;   // renderHeader() runs after sorts, filters, undos: same rows, same kinds
    /* Each row split once, not once per column: after a sort the rows come from all over
       the file, the row cache kept evicting them, and 100 columns cost 40 000 splits (~300 ms). */
    const rows = t.allData.slice(0, 400).map(r => r.data);
    const kinds = t.headers.map((_, c) => {
        const k = { n: 0, d: 0, t: 0 }; let f = 0;
        for (const d of rows) { const ty = cellType(d[c]); if (ty) { k[ty]++; f++; } }
        return !f ? '' : k.n / f >= 0.9 ? 'n' : k.d / f >= 0.9 ? 'd' : 't';
    });
    t.kindsCache = { stamp: dataStamp(t), kinds };
    return kinds;
}
function typeIcon(k) {
    if (k === 'n') return '<span class="ty ty-n" title="Numbers">#</span>';
    if (k === 'd') return '<span class="ty ty-d" title="Dates"></span>';   // the calendar is a CSS mask: an SVG per title weighed on every header rebuild
    if (k === 't') return '<span class="ty ty-t" title="Text">Aa</span>';
    return '';
}

function renderHeader() {
    const t = T(); if (!t) { thead.innerHTML = ''; return; }
    let hCells = `<th class="col-idx" onclick="openColManager()" title="Manage Columns" style="cursor:pointer;">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px; vertical-align: middle;"><line x1="4" y1="21" x2="4" y2="14"></line><line x1="4" y1="10" x2="4" y2="3"></line><line x1="12" y1="21" x2="12" y2="12"></line><line x1="12" y1="8" x2="12" y2="3"></line><line x1="20" y1="21" x2="20" y2="16"></line><line x1="20" y1="12" x2="20" y2="3"></line><line x1="1" y1="14" x2="7" y2="14"></line><line x1="9" y1="8" x2="15" y2="8"></line><line x1="17" y1="16" x2="23" y2="16"></line></svg>
    </th>`;
    let fCells = `<th class="col-idx"></th>`;

    const genCls = t.syntheticHeader ? ' gen-head' : '';
    const kinds = t.loaded ? columnKinds(t) : [];
    t.headers.forEach((h, i) => {
        const sk = t.sort ? t.sort.findIndex(k => k.col === i) : -1;
        const sortInd = sk < 0 ? '' : `<span class="sort-ind">${t.sort[sk].dir > 0 ? '▲' : '▼'}${t.sort.length > 1 ? `<sup>${sk + 1}</sup>` : ''}</span>`;
        hCells += `<th class="col-th${genCls}" data-col="${i}" ondragover="colDragOver(event)" ondragleave="this.classList.remove('drop-before', 'drop-after')" ondrop="colDrop(event)">
            <div class="col-title">
                <span class="col-name" draggable="true" onclick="titleClick(event, ${i})" ondragstart="colDragStart(event, ${i})" ondragend="colDragEnd()" title="Click: sort · again: reverse · Shift+click: sub-sort">${typeIcon(kinds[i])}${esc(h)}${t.syntheticHeader ? '' : `<span class="col-no">${i}</span>`}${sortInd}</span>
            </div>
            <span class="col-menu${t.valFilters[i] ? ' on' : ''}" onclick="openColPanel(event, ${i})" title="${t.valFilters[i] ? 'Filtered · ' : ''}Profile, filter"></span>
            <div class="resizer" data-col="${i}"></div>
        </th>`;
        fCells += `<th>${colFilterBox(i, t.colFilters[i])}</th>`;
    });
    /* The column charts' row (47-…) is kept when it is still the right one: only the titles and the filters are rebuilt. */
    if (spkRowKept(t)) { thead.rows[0].outerHTML = `<tr>${hCells}</tr>`; thead.lastElementChild.outerHTML = `<tr class="filter-row">${fCells}</tr>`; }
    else { thead.innerHTML = `<tr>${hCells}</tr>${spkRowHtml(t)}<tr class="filter-row">${fCells}</tr>`; spkRowMark(t); }
    if (srBar.style.display === 'flex') fillSRCols();
    document.querySelectorAll('.resizer').forEach(setupResizer);
    spkSoon(t);                               // the column charts (47-…): shown if known, else computed when idle
}

/* The filter row holds look-alike boxes, not inputs: 85 inputs were most of
   the header's layout, rebuilt after every sort, undo, tab switch… for a
   row rarely typed in. A click (or Tab from the previous box) swaps a box
   for a real input; leaving it swaps it back. t.colFilters holds the values. */
function colFilterBox(c, v) {
    return `<span class="bs-input f-in f-box${v ? ' has-value' : ''}" data-col="${c}" tabindex="0" onfocus="editColFilter(this)" title="Filter this column">${v ? esc(v) : '<span class="f-ph">Filter</span>'}</span>`;
}
function editColFilter(box) {
    const t = T(); if (!t) return;
    const c = +box.dataset.col, input = document.createElement('input');
    input.className = 'bs-input f-in' + (t.colFilters[c] ? ' has-value' : '');
    input.dataset.col = c; input.size = 1; input.placeholder = 'Filter'; input.spellcheck = false; input.autocomplete = 'off';
    input.value = t.colFilters[c] || '';
    box.replaceWith(input);
    input.focus(); input.setSelectionRange(input.value.length, input.value.length);
    input.oninput = () => {
        if (input.value) t.colFilters[c] = input.value; else delete t.colFilters[c];
        input.classList.toggle('has-value', input.value !== '');
        filterSoon();
    };
    input.onblur = () => { if (input.isConnected) input.outerHTML = colFilterBox(c, t.colFilters[c]); };
}
