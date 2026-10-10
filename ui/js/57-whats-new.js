/* ---------------------------------------------------------------
   WHAT'S NEW
   The features each version brought, newest first, from the About dialog and the palette. The
   notes are ui/releases.js, a script added on the first opening only: app.js is read at every
   launch, these few KB now and then. A release with features adds its entry there.
   --------------------------------------------------------------- */
let newsLoad = null;            // the promise of ui/releases.js, once asked

function releasesLoad() {
    return newsLoad || (newsLoad = new Promise((ok, ko) => {
        const s = document.createElement('script');
        s.src = 'ui/releases.js';
        s.onload = () => ok(window.CSVFAB_RELEASES || []);
        s.onerror = () => { newsLoad = null; s.remove(); ko(new Error('ui/releases.js')); };   // asked again next time
        document.head.appendChild(s);
    }));
}

function newsHtml(rel, cur) {
    const day = d => new Date(d + 'T12:00').toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric' });
    return rel.map(r => `<section class="nw-rel${r.v === cur ? ' nw-cur' : ''}">
        <div class="nw-when"><b>${esc(r.v)}</b><span>${esc(day(r.date))}</span>${r.v === cur ? '<i class="nw-tag">Your version</i>' : ''}${r.first ? '<i class="nw-tag nw-first">First release</i>' : ''}</div>
        <ul class="nw-items">${r.items.map(([what, how]) => `<li><b>${esc(what)}</b><span>${esc(how)}</span></li>`).join('')}</ul>
    </section>`).join('');
}

async function openWhatsNew() {
    closeDDs(); closeAllModals();
    modalOnce('modal-news', `<div id="modal-news" class="modal-box">
        <h4>What's new in csvfab</h4>
        <div class="nw-sub">The main features, version by version.</div>
        <div class="nw-list" id="nw-list"></div>
        <div class="modal-actions">
            <a class="nw-all" href="https://github.com/fabiochelly/csvfab/releases" target="_blank" rel="noopener noreferrer">Full release notes</a>
            <button class="btn btn-outline" onclick="openAbout()">About</button>
            <button class="btn" onclick="closeAllModals()">Close</button>
        </div>
    </div>`);
    const list = document.getElementById('nw-list');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-news').style.display = 'block';
    list.scrollTop = 0;
    if (!list.firstChild) list.innerHTML = '<div class="nw-wait">Loading…</div>';
    try {
        const [rel, cur] = await Promise.all([releasesLoad(),
            fetch('/api/ping').then(r => r.json()).then(j => j.version || '').catch(() => '')]);
        list.innerHTML = newsHtml(rel, cur);
    } catch (e) {
        list.innerHTML = '<div class="nw-wait">The release notes could not be read.</div>';
    }
}
