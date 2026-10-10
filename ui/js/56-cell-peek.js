/* ---------------------------------------------------------------
   CELL PEEK
   Resting the pointer on a cell whose value does not show whole (cut
   by its ellipsis, or holding line breaks) shows the whole value in a
   frosted box under it, as the tooltips are: JSON indented, line
   breaks kept, a long text cut at PEEK_MAX characters. After PEEK_WAIT
   ms — at once, it would flash over every cell the pointer crosses.
   Gone on leaving the cell, a press, a key, a wheel or a scroll. A
   cell with its own tooltip (a mark, a difference) keeps it.
   Text only (textContent): the value never reaches the page as HTML.
----------------------------------------------------------------*/
const PEEK_WAIT = 450;            // ms of rest on a cell before its value shows
const PEEK_MAX = 4000;            // characters shown at most
const peekBox = document.createElement('div'); peekBox.id = 'cell-peek'; document.body.appendChild(peekBox);
let peekCell = null, peekTimer = 0;
function peekHide() { clearTimeout(peekTimer); peekTimer = 0; peekCell = null; peekBox.classList.remove('open'); }
/* The value as the peek shows it: JSON indented when it is an object or a list, else as is. */
function peekText(v) {
    let s = v == null ? '' : String(v);
    const c = s.trimStart()[0];
    if ((c === '{' || c === '[') && s.length < 200000) { try { const j = JSON.parse(s); if (j && typeof j === 'object') s = JSON.stringify(j, null, 2); } catch (e) { } }
    return s.length > PEEK_MAX ? s.slice(0, PEEK_MAX) + '…' : s;
}
function peekShow(cell) {
    const t = T(); if (!t || !t.loaded || !cell.isConnected) return;
    const row = cell.closest('.row[data-idx]'), r = row && t.filteredData[+row.dataset.idx];
    if (!r) return;
    const v = cellOf(r, +cell.dataset.c);
    if (v == null || v === '') return;
    peekBox.textContent = peekText(v);
    peekBox.classList.add('open');
    const b = cell.getBoundingClientRect(), w = peekBox.offsetWidth, h = peekBox.offsetHeight;
    const below = b.bottom + 6 + h <= innerHeight - 30;   // above the status bar
    peekBox.style.left = Math.max(6, Math.min(b.left, innerWidth - w - 6)) + 'px';
    peekBox.style.top = (below ? b.bottom + 6 : Math.max(6, b.top - 6 - h)) + 'px';
}
tbody.addEventListener('mouseover', e => {
    const cell = e.target.closest && e.target.closest('.cell[data-c]');
    if (cell === peekCell) return;
    peekHide();
    if (!cell || e.buttons || cell.dataset.t || cell.hasAttribute('title') || document.querySelector('.cell-editor')) return;
    /* Only a value that does not show whole: clipped (.ov, decided at drawing) and really overflowing, or with line breaks (↵). */
    if (!cell.classList.contains('ov')) return;
    if (cell.scrollWidth <= cell.clientWidth && !cell.querySelector('.nl')) return;
    peekCell = cell;                      // pending: moving over its marks does not start the wait again
    peekTimer = setTimeout(() => peekShow(cell), PEEK_WAIT);
});
tbody.addEventListener('mouseleave', peekHide);
for (const ev of ['mousedown', 'wheel', 'keydown']) document.addEventListener(ev, peekHide, true);
container.addEventListener('scroll', peekHide, { passive: true });
window.addEventListener('blur', peekHide);
