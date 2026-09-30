/* ---------------------------------------------------------------
   SESSION
   The open files come back at the next start: the tabs (bridge paths,
   and file handles still readable — a handle whose permission lapsed
   is left to the Recent list, since asking for it needs a click), the
   active one, each tab's scroll position, hidden columns and widths.
   Nothing of the edits (they only live in memory, Save is the way) nor
   of the filters. Kept in IndexedDB under `session`, rewritten a
   moment after every tab change, width change and scroll, and at
   pagehide. A file handed over by the launcher while the session is
   being restored keeps the focus: the restored tabs then stay
   unloaded until clicked. Off with the palette's toggle (localStorage
   csvfab-restore = 0), which also drops the stored session.
   State hangs off the functions: renderTabBar() runs at boot before
   this file's top level, and a `let` here would still be in its TDZ.
----------------------------------------------------------------*/
function sessionOn() { try { return localStorage.getItem('csvfab-restore') !== '0'; } catch (e) { return true; } }
function toggleSessionRestore() {
    const on = !sessionOn();
    try { localStorage.setItem('csvfab-restore', on ? '1' : '0'); } catch (e) { }
    toast(on ? 'The files open at quit will be reopened at the next start.' : 'The next start opens an empty window.', { kind: 'info' });
    saveSession();
}
function saveSessionSoon() { clearTimeout(saveSessionSoon.timer); saveSessionSoon.timer = setTimeout(saveSession, 500); }
async function saveSession() {
    clearTimeout(saveSessionSoon.timer);
    if (!saveSession.ready) return;               // before the restore has run, a save would wipe the session being restored
    const t0 = T(); if (t0) t0.scrollTop = container.scrollTop;
    const items = tabs.filter(t => t.path || t.handle).map(t => ({
        kind: t.path ? 'path' : 'handle', path: t.path || null, handle: t.handle || null, dirHandle: t.dirHandle || null,
        name: t.name, size: t.size, scrollTop: t.scrollTop || 0, hidden: [...t.hiddenCols], widths: { ...t.colWidths }, active: t.id === activeTabId
    }));
    await idbSet('session', sessionOn() && items.length ? { at: Date.now(), items } : null);
}

async function restoreSession() {
    const s = sessionOn() ? await idbGet('session') : null;
    saveSession.ready = true;
    if (!s || !s.items || !s.items.length) return;
    const entries = []; let active = -1, denied = 0;
    for (const it of s.items) {
        const e = { name: it.name, size: it.size, scrollTop: it.scrollTop, hidden: it.hidden, widths: it.widths };
        if (it.kind === 'path') {
            if (!SRV || tabs.some(t => t.path === it.path)) continue;
            try { const st = await srvStat(it.path); e.path = it.path; e.name = st.name; e.size = st.size; } catch (err) { continue; }   // gone: not reopened
        } else {
            if (!it.handle) continue;
            try {
                if (await it.handle.queryPermission({ mode: 'read' }) !== 'granted') { denied++; continue; }
                const f = await it.handle.getFile(); e.file = f; e.handle = it.handle; e.dirHandle = it.dirHandle; e.name = f.name; e.size = f.size;
            } catch (err) { continue; }
        }
        entries.push(e);
        if (it.active) active = entries.length - 1;
    }
    /* A file already opened meanwhile (the launcher's) keeps the focus: the restored tabs wait unloaded. */
    if (entries.length) addTabs(entries, { quiet: true, active: tabs.length ? -1 : Math.max(0, active) });
    else saveSession();
    if (denied) toast(`${denied} file${denied > 1 ? 's' : ''} of the last session need${denied > 1 ? '' : 's'} access again — reopen ${denied > 1 ? 'them' : 'it'} from Recent.`, { kind: 'info', ms: 9000 });
}
restoreSession();

/* The scroll position, at most every 2 s while scrolling. */
container.addEventListener('scroll', () => {
    if (saveSession.scrollTimer) return;
    saveSession.scrollTimer = setTimeout(() => { saveSession.scrollTimer = 0; saveSession(); }, 2000);
});
window.addEventListener('pagehide', () => saveSession());
