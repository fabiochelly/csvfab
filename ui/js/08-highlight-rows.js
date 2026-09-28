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

/* A cell is one line high: a line break shows as a discreet ↵. */
function showBreaks(html) { return html.indexOf('\n') < 0 ? html : html.replace(/\r?\n/g, '<span class="nl">↵</span>'); }
function highlightCell(text, cIdx, hl) {
    if (text === null || text === undefined) return '';
    let out = String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    if (!hl || hl.reverse) return out;

    let patterns = [];
    const colVal = hl.colFilters[cIdx];
    if (colVal) patterns.push(hl.isRegex ? colVal : buildSafePattern(colVal, hl.useSlug));
    if (hl.globalQuery) patterns.push(hl.isRegex ? hl.globalQuery : buildSafePattern(hl.globalQuery, hl.useSlug));

    if (patterns.length > 0) {
        try {
            let combinedRegex = new RegExp(`(${patterns.join('|')})`, 'gi');
            out = out.replace(combinedRegex, '<mark>$1</mark>');
        } catch (e) { }
    }
    return out;
}

// --- Row Management ---
function duplicateRow(id) {
    const t = T(); if (!t) return;
    const index = t.allData.findIndex(r => r.id === id);
    if (index === -1) return;
    const newRow = { id: 0, data: [...t.allData[index].data] };
    t.allData.splice(index + 1, 0, newRow);
    t.allData.forEach((r, idx) => r.id = idx + 1);
    t.rowCount = t.allData.length;
    t.modificationsLog.forEach(log => { if (typeof log.id === 'number' && log.id > id) log.id++; });
    t.modificationsLog.push({ id: index + 2, col: '---', old: '---', new: 'Row duplicated', what: 'row duplicated', undo: t => {
        t.allData.splice(t.allData.indexOf(newRow), 1);
    } });
    updateSaveBtn(); applyFilters(); renderTabBar();
}
