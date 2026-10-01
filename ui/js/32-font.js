/* ---------------------------------------------------------------
   MONOSPACE FONT
   The font of the cells, formulas and codes, picked in a dialog (palette:
   "Monospace font…") and kept in localStorage csvfab-mono. It goes first
   in --mono through --mono-pick (set before the first paint by
   viewer.htm too); the default stack stays behind it as the fallback.
   Each candidate is tested in this very browser (fontUsable(): a text
   measured in it against the generic families), since a font installed
   on the system may still not be the one Chromium draws — fontconfig can
   substitute another. Changing the font measures the cells' advance again
   (cellAdv) and scales every tab's column widths by the change, so manual
   widths keep their proportions.
----------------------------------------------------------------*/
const MONO_FONTS = ['MonoLisa', 'MonolisaNerd', 'Cascadia Code', 'Cascadia Mono', 'JetBrains Mono', 'JetBrainsMono Nerd Font', 'Fira Code', 'Fira Mono',
    'Source Code Pro', 'IBM Plex Mono', 'Hack', 'Iosevka', 'Roboto Mono', 'Ubuntu Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Noto Sans Mono',
    'SF Mono', 'Menlo', 'Monaco', 'Consolas', 'Courier New'];

function monoPick() { try { return localStorage.getItem('csvfab-mono') || ''; } catch (e) { return ''; } }
/* Whether this browser draws a family as a monospace text font: drawn at all (a sample in it, with
   serif behind, measures otherwise than serif alone — a family it lacks falls back), and fixed-width
   for Latin letters (ten i as wide as ten W), which leaves out emoji and icon fonts and proportional
   variants that fontconfig also lists as "mono". Widths cannot tell one monospace font from another
   of the same advance, so a substitution by the system is not detectable here. */
function fontUsable(name) {
    const c = fontUsable.ctx || (fontUsable.ctx = document.createElement('canvas').getContext('2d'));
    const w = (f, s) => { c.font = `20px ${f}`; return c.measureText(s).width; }, f = `"${name}", serif`;
    if (w(f, 'mmmmmmmmmmlli10O@#WwQq') === w('serif', 'mmmmmmmmmmlli10O@#WwQq')) return false;
    return Math.abs(w(f, 'iiiiiiiiii') - w(f, 'WWWWWWWWWW')) < 0.5;
}
function setMonoFont(name) {
    const before = cellAdv(), h = document.documentElement;
    if (name) h.style.setProperty('--mono-pick', name === 'monospace' ? 'monospace' : `"${name.replace(/["\\]/g, '')}"`);
    else h.style.removeProperty('--mono-pick');
    try { if (name) localStorage.setItem('csvfab-mono', name); else localStorage.removeItem('csvfab-mono'); } catch (e) { }
    cellAdvPx = 0; cellCtx = null;                     // measured again in the new font
    const k = cellAdv() / before;
    if (Math.abs(k - 1) > 0.005) for (const t of tabs) {
        for (const c of Object.keys(t.colWidths)) t.colWidths[c] = Math.max(60, Math.min(900, Math.round((t.colWidths[c] - 21) * k + 21)));   // the text part scales, padding and border do not
    }
    applyColStyles(); render();
    renderFontDialog();
}
/* The installed monospace families: from the server (fc-list), else — Windows, macOS without
   fontconfig — from the browser's Local Font Access on request (it asks the user first), each
   family measured to keep the fixed-width ones. Merged with MONO_FONTS; only what this browser
   can draw is listed. */
async function openFontDialog() {
    closeDDs();
    const st = openFontDialog;
    st.q = '';
    if (!st.mono && SRV) await fetchFonts();
    document.getElementById('font-q').value = '';
    renderFontDialog();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-font').style.display = 'block';
    document.getElementById('font-q').focus();
}
async function fetchFonts() {
    const st = openFontDialog;
    try { const r = await srvFetch('/api/fonts'), j = r.ok ? await r.json() : {}; st.mono = j.mono || []; st.catalog = j.catalog || []; }
    catch (e) { st.mono = []; st.catalog = []; }
}
async function listLocalFonts() {
    try {
        const all = await window.queryLocalFonts(), fams = [...new Set(all.map(f => f.family))];
        const mono = fams.filter(fontUsable);
        openFontDialog.mono = mono.sort((a, b) => a.localeCompare(b));
    } catch (e) { setStats(`The installed fonts could not be listed: ${e.message || e}`); }
    renderFontDialog();
}
const FONT_SENTENCE = 'The quick brown fox jumps over the lazy dog.';
const FONT_SYMBOLS = '0O Il1 {}[]()<> => != 12 345,67 € @#%& éàçô';
function renderFontDialog() {
    const box = document.getElementById('font-list'); if (!box) return;
    const st = openFontDialog, cur = monoPick(), q = removeAccents((st.q || '').trim().toLowerCase());
    const card = (val, label, family, note) => `<div class="font-card${val === cur ? ' on' : ''}" data-v="${esc(val)}" onclick="setMonoFont(this.dataset.v)" role="radio" aria-checked="${val === cur}" tabindex="0" onkeydown="if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setMonoFont(this.dataset.v); }">`
        + `<div class="fc-head"><span class="fc-name">${esc(label)}</span>${note ? `<span class="fc-note">${esc(note)}</span>` : ''}<span class="fc-check"></span></div>`
        + `<div class="fc-sample" style="font-family: ${esc(family)}"><div>${esc(FONT_SENTENCE)}</div><div class="fc-sym">${esc(FONT_SYMBOLS)}</div></div></div>`;
    const names = [...new Set([...(st.mono || []), ...MONO_FONTS, ...(cur && cur !== 'monospace' ? [cur] : [])])];
    const usable = names.filter(fontUsable).sort((a, b) => a.localeCompare(b, 'en', { sensitivity: 'base' }));
    const match = label => !q || removeAccents(label.toLowerCase()).includes(q);
    const fixed = [['', 'Default', 'var(--mono-default)', 'MonoLisa, Cascadia Code, Monaco, JetBrains Mono, Consolas — the first found'], ['monospace', 'System monospace', 'monospace', 'the system\'s own']];
    const html = fixed.filter(f => match(f[1])).map(f => card(...f)).join('')
        + usable.filter(match).map(f => card(f, f, `"${f}", monospace`, '')).join('');
    /* Free fonts not drawn here yet, installed on request by the server (FONT_CATALOG in server.py). */
    const offer = (st.catalog || []).filter(c => !usable.includes(c.family) && (match(c.name) || match(c.note || '')));
    const shop = offer.map(c => `<div class="font-card shop" data-id="${esc(c.id)}">`
        + `<div class="fc-head"><span class="fc-name">${esc(c.name)}</span><span class="fc-note">${esc(c.note || '')} · ${esc(c.license)}</span>`
        + `<button class="btn btn-outline fc-install" onclick="installFont('${esc(c.id)}', this)" title="From github.com/${esc(c.repo)}, for your account only">Install</button></div></div>`).join('');
    box.innerHTML = (html || '<div class="cmdk-empty">No installed font matches.</div>')
        + (shop ? `<div class="font-sec">Free fonts to install<span>downloaded from their official repositories, for your user account only</span></div>${shop}` : '');
    const more = document.getElementById('font-more');
    more.style.display = !(st.mono && st.mono.length) && 'queryLocalFonts' in window ? '' : 'none';
    const on = box.querySelector('.font-card.on'); if (on && !q) on.scrollIntoView({ block: 'nearest' });
}
/* Install a catalogue font through the server, then use it once this browser draws it (a running
   Chromium picks new fonts up after fontconfig's cache is updated, which the server does; failing
   that, the next start will). */
async function installFont(id, btn) {
    const st = openFontDialog, c = (st.catalog || []).find(x => x.id === id); if (!c) return;
    btn.disabled = true; btn.innerHTML = '<span class="fc-spin"></span>Installing…';
    try {
        const r = await srvFetch(`/api/font-install?id=${encodeURIComponent(id)}`, { method: 'POST' }), j = await r.json().catch(() => ({}));
        if (!r.ok || !j.ok) throw new Error(j.error || `HTTP ${r.status}`);
        await fetchFonts();
        let seen = fontUsable(c.family);
        for (let k = 0; !seen && k < 10; k++) { await new Promise(res => setTimeout(res, 500)); seen = fontUsable(c.family); }
        if (seen) { setMonoFont(c.family); toast(`${c.name} ${j.version || ''} installed (${j.files} files) and chosen.`, { kind: 'ok' }); return; }
        /* The usual case: a running Chromium only sees the fonts it found at its start. The choice is
           kept now, and applies at the next start — offered at once. */
        try { localStorage.setItem('csvfab-mono', c.family); } catch (e) { }
        renderFontDialog();
        if (await uiConfirm(`${c.name} is installed.\n\nIt will be used once csvfab restarts: the browser only sees the fonts it found when it started. Restart now? The files open come back.`, { ok: 'Restart', cancel: 'Later' })) relaunchApp();
        else toast(`${c.name} will be used at the next start.`, { kind: 'ok' });
    } catch (e) {
        btn.disabled = false; btn.textContent = 'Install';
        toast(`${c.name} could not be installed: ${e.message || e}`, { kind: 'warn', ms: 9000 });
    }
}
/* Close the window and have the server open a new one, the files open by path coming back as tabs
   (server.py relaunch_after_close()). A file opened through the picker cannot be reopened by
   path, and edits not saved are lost: both are said first. */
async function relaunchApp() {
    const dirty = tabs.filter(isDirty), byHandle = tabs.filter(t => !t.path).length;
    const notes = [dirty.length && `${dirty.length === 1 ? `"${dirty[0].name}" has` : `${dirty.length} files have`} unsaved edits, which will be lost.`,
        byHandle && `${byHandle} file${byHandle > 1 ? 's' : ''} opened through the file picker will not come back (open ${byHandle > 1 ? 'them' : 'it'} from Recent).`].filter(Boolean);
    if (notes.length && !await uiConfirm(`Restart csvfab?\n\n${notes.join(' ')}`, { ok: dirty.length ? 'Restart without saving' : 'Restart', danger: !!dirty.length })) return;
    try {
        const r = await srvFetch('/api/relaunch', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ paths: tabs.filter(t => t.path).map(t => t.path) }) });
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
    } catch (e) { uiAlert(`csvfab could not restart itself.\n\n${e.message || e}`); return; }
    quitting = true;
    window.close();                        // the --app window may close itself; the server opens the next one
    setTimeout(() => { if (!window.closed) { quitting = false; uiAlert('This window cannot close itself here.\n\nClose it: csvfab opens again by itself.'); } }, 400);
}
