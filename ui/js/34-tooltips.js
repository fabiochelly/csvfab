/* ---------------------------------------------------------------
   TOOLTIPS
   Every title attribute, generated or static, shows at once in one
   discreet box (#tip) instead of the browser's own tooltip, which
   waits about a second and cannot be styled. One delegated handler:
   on hover, an element's title moves to data-t (so the native one
   never appears) and is shown under it — above when there is no room.
   A title set again later (el.title = …) wins over data-t at the next
   hover. Line breaks in a title (&#10;) are kept.
   The duplicates' Merge hint has its own, larger box (#dup-tip).
----------------------------------------------------------------*/
const tipBox = document.createElement('div'); tipBox.id = 'tip'; document.body.appendChild(tipBox);
let tipFor = null;
function tipHide() { tipFor = null; tipBox.classList.remove('open'); }
function tipShow(el) {
    const txt = el.dataset.t; if (!txt) return tipHide();
    tipFor = el;
    tipBox.textContent = txt;
    tipBox.classList.add('open');
    const r = el.getBoundingClientRect(), w = tipBox.offsetWidth, h = tipBox.offsetHeight;
    const x = Math.max(6, Math.min(r.left + r.width / 2 - w / 2, innerWidth - w - 6));
    const below = r.bottom + 6 + h <= innerHeight - 4;
    tipBox.style.left = x + 'px';
    tipBox.style.top = (below ? r.bottom + 6 : Math.max(4, r.top - 6 - h)) + 'px';
}
document.addEventListener('mouseover', e => {
    const el = e.target.closest && e.target.closest('[title], [data-t]');
    if (!el) return tipHide();
    if (el.hasAttribute('title')) { el.dataset.t = el.getAttribute('title'); el.removeAttribute('title'); }
    if (el !== tipFor) tipShow(el);
}, true);
document.addEventListener('mouseout', e => { if (tipFor && !(e.relatedTarget && tipFor.contains(e.relatedTarget))) tipHide(); }, true);
for (const ev of ['mousedown', 'wheel', 'keydown']) document.addEventListener(ev, tipHide, true);
window.addEventListener('blur', tipHide);
