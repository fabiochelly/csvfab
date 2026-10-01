// --- Moteur de Surlignage ---
function buildSafePattern(str, useSlug) {
    let esc = str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    if (useSlug) {
        esc = esc.replace(/a/gi, '[aàáâãäå]')
            .replace(/e/gi, '[eèéêë]')
            .replace(/i/gi, '[iìíîï]')
            .replace(/o/gi, '[oòóôõö]')
            .replace(/u/gi, '[uùúûü]')
            .replace(/c/gi, '[cç]')
            .replace(/n/gi, '[nñ]');
    }
    return esc;
}

/* Every word of a simple filter (queryTerms(), 15-…) as one alternation, the longest first so
   that "dup" does not stop "dupont" from being marked whole. Kept per query: highlightCell()
   runs for every cell drawn. */
const hlPatCache = new Map();
function termsPattern(q, useSlug) {
    const k = (useSlug ? '1' : '0') + q;
    let p = hlPatCache.get(k);
    if (p == null) {
        p = queryTerms(q).sort((a, b) => b.length - a.length).map(s => buildSafePattern(s, useSlug)).join('|');
        if (hlPatCache.size > 200) hlPatCache.clear();
        hlPatCache.set(k, p);
    }
    return p;
}

/* A cell is one line high: a line break shows as a discreet ↵. */
function showBreaks(html) { return html.indexOf('\n') < 0 ? html : html.replace(/\r?\n/g, '<span class="nl">↵</span>'); }
function highlightCell(text, cIdx, hl) {
    if (text === null || text === undefined) return '';
    let out = String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    if (!hl || (hl.reverse && !hl.find)) return out;

    let patterns = [];
    const colVal = hl.reverse ? '' : hl.colFilters[cIdx];   // inverted filters: nothing of theirs to highlight
    if (colVal) patterns.push(hl.isRegex ? colVal : termsPattern(colVal, hl.useSlug));
    if (hl.globalQuery && !hl.reverse) patterns.push(hl.isRegex ? hl.globalQuery : termsPattern(hl.globalQuery, hl.useSlug));
    patterns = patterns.filter(Boolean);

    if (patterns.length > 0) {
        try {
            let combinedRegex = new RegExp(`(${patterns.join('|')})`, 'gi');
            out = out.replace(combinedRegex, '<mark>$1</mark>');
        } catch (e) { }
    }
    /* The find field of the replace bar (29-…): its own mark, its own scope and case. */
    if (hl.find && (hl.findCol < 0 || hl.findCol === cIdx)) {
        try { out = out.replace(new RegExp(`(${hl.find})`, hl.findCase ? 'g' : 'gi'), '<mark class="fd">$1</mark>'); } catch (e) { }
    }
    return out;
}

// --- Row Management ---
function duplicateRow(id) {
    const t = T(); if (!t) return;
    const index = t.allData.findIndex(r => r.id === id);
    if (index === -1) return;
    const copy = newRow(t, t.allData[index].data.slice());
    t.allData.splice(index + 1, 0, copy);
    t.allData.forEach((r, idx) => r.id = idx + 1);
    t.rowCount = t.allData.length;
    t.modificationsLog.forEach(log => { if (typeof log.id === 'number' && log.id > id) log.id++; });
    t.modificationsLog.push({ id: index + 2, col: '---', old: '---', new: 'Row duplicated', what: 'row duplicated', undo: t => {
        t.allData = t.allData.filter(r => r !== copy);   // a new array, never a splice: redo restores the one recorded (15-…)
    } });
    updateSaveBtn(); applyFilters(); renderTabBar();
}
