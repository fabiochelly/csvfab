/* ---------------------------------------------------------------
   ANONYMISE
   A copy of a CRM export can be shared for a test or a demo once the
   people in it cannot be recognised. Each column gets a treatment,
   guessed from its name and values and changeable: names become other
   names with the same initial, taken from short lists; an e-mail keeps
   its shape (user part treated like a name, domain swapped for a
   reserved one); a phone number keeps its country code and first two
   digits; a postal code keeps its department; a date is shifted by up
   to a year; anything else can be scrambled letter for letter, its
   shape kept. Every replacement is derived from a hash of the value,
   so the same value gives the same result in every row, column and
   file — duplicates stay duplicates and joins still hold.
----------------------------------------------------------------*/
const FIRST_NAMES = ('Alice Antoine Aurélie Adrien Anaïs Alain Benoît Brigitte Bastien Béatrice Bruno Camille Clément Céline Christophe Chloé '
    + 'David Delphine Damien Dominique Émilie Éric Elsa Étienne Florence Fabien François Fanny Gaëlle Guillaume Gérard Gabrielle Hélène Hugo Henri '
    + 'Inès Isabelle Ismaël Julie Julien Jeanne Jérôme Karine Kevin Kamel Laura Lucas Laurent Léa Marie Mathieu Manon Maxime Nathalie Nicolas Nadia Noé '
    + 'Olivier Océane Omar Pauline Pierre Philippe Patricia Quentin Quitterie Romain Rachel Rémi Roxane Sophie Samuel Sandrine Simon Thomas Théa Thierry '
    + 'Ulysse Ursula Valérie Vincent Victor Véronique William Wendy Xavier Xénia Yann Yasmine Yves Zoé Zacharie').split(' ');
const LAST_NAMES = ('Andrieu Aubert Arnaud Allard Bernard Blanc Bonnet Brun Chevalier Colin Caron Clément Dubois Durand Dumont Delorme Étienne Evrard '
    + 'Fontaine Faure Fournier Garnier Girard Guérin Henry Hubert Hamon Imbert Israël Joly Jacquet Klein Keller Lambert Lefèvre Leroy Lemoine '
    + 'Martin Morel Mercier Masson Noël Nguyen Nicolas Olivier Ollivier Perrin Petit Picard Pons Quéré Quintin Roux Renard Rolland Rey '
    + 'Simon Schmitt Sauvage Thomas Tessier Toussaint Urbain Vidal Vasseur Vincent Weber Wagner Xavier Yvon Zimmermann').split(' ');
const COMPANIES = ['Alpha Conseil', 'Atelier Nord', 'Bleu Horizon', 'Boréal Services', 'Cap Ouest', 'Corail Industrie', 'Delta Logistique', 'Dune Digital',
    'Écume Studio', 'Élan Distribution', 'Fabrique du Sud', 'Forge Numérique', 'Granit Bâtiment', 'Galaxie Media', 'Horizon Immo', 'Héliotrope',
    'Indigo Formation', 'Ilot Vert', 'Jade Solutions', 'Juniper Tech', 'Kaolin Design', 'Korrigan Transport', 'Lumen Énergie', 'Lagune Traiteur',
    'Méridien Assurances', 'Mosaïque Web', 'Nacre Cosmétique', 'Nordik Outillage', 'Onyx Sécurité', 'Orée Paysages', 'Prisme Audit', 'Pollen Agri',
    'Quartz Optique', 'Quai des Arts', 'Rivage Voyages', 'Racine Bio', 'Sillage Marine', 'Sextant Ingénierie', 'Tilleul Santé', 'Topaze Finance',
    'Ubac Montagne', 'Unisson Événements', 'Vallon Menuiserie', 'Vertige Sport', 'Wagon Éditions', 'Westline Textile', 'Xénon Éclairage', 'Yucca Jardins',
    'Zénith Consulting', 'Zéphyr Aéro'];
const STREETS = ['rue des Lilas', 'avenue de la Gare', 'boulevard Victor Hugo', 'chemin des Vignes', 'allée des Peupliers', 'place du Marché', 'impasse des Roses',
    'rue de la Paix', 'rue Pasteur', 'avenue Jean Jaurès', 'rue des Écoles', 'route de Lyon', 'rue du Moulin', 'cours Mirabeau', 'quai des Brumes',
    'rue Émile Zola', 'rue Saint-Michel', 'avenue des Champs', 'chemin du Lavoir', "rue de l'Église", 'rue des Fontaines', 'boulevard de la République',
    'allée du Stade', 'rue du Port'];
const DOMAINS = ['example.com', 'example.org', 'example.net', 'mail.example', 'courrier.example', 'contact.example', 'demo.example', 'test.example'];   // reserved names: they never deliver
const STREET_TYPE = /^(rue|avenue|av\.?|boulevard|bd\.?|chemin|allée|allee|place|impasse|route|cours|quai|square|passage|voie|résidence|residence|lotissement|hameau|clos|sentier|promenade|esplanade|traverse|villa|mail)\b\.?/i;
const LEGAL_FORM = /\s*[-,]?\s*\b(SARL|SASU|SAS|SA|EURL|SCI|SCOP|SNC|SELARL|GIE|GmbH|AG|Ltd\.?|LLC|Inc\.?|Corp\.?|SRL|BV|NV|SL|SpA|Oy|AB|AS)\s*$/i;
const PARTICLES = new Set(['de', 'du', 'des', 'la', 'le', 'les', 'van', 'von', 'der', 'den', 'el', 'al', 'di', 'da', 'y', 'et', 'and', 'of', 'the', 'ben', 'bin', 'ibn', 'mc', 'mac', "d'", "l'"]);

/* FNV-1a, then a small PRNG seeded with it: "random" digits that are the same for the same value. */
function anonHash(s) { let h = 0x811c9dc5; for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193); } return h >>> 0; }
function anonRng(seed) { let a = seed || 1; return () => { a |= 0; a = a + 0x6D2B79F5 | 0; let x = Math.imul(a ^ a >>> 15, 1 | a); x = x + Math.imul(x ^ x >>> 7, 61 | x) ^ x; return ((x ^ x >>> 14) >>> 0) / 4294967296; }; }
const initialOf = w => (removeAccents(w.toLowerCase()).match(/[a-z]/) || [''])[0];
const buckets = new Map();
function bucket(list, letter) {
    let b = buckets.get(list);
    if (!b) { b = new Map(); for (const w of list) { const k = initialOf(w); if (!b.has(k)) b.set(k, []); b.get(k).push(w); } buckets.set(list, b); }
    return b.get(letter) || list;
}
/* The case of the original word: DUPONT → DURAND, Dupont → Durand, dupont → durand.
   (toUpperCase / toLowerCase rather than the 'fr' locale forms: same result, much faster.) */
function caseLike(src, w) {
    if (src === src.toUpperCase() && /\p{Lu}/u.test(src)) return w.toUpperCase();
    if (src[0] === src[0].toLowerCase()) return w.toLowerCase();
    return w;
}
function anonWord(w, list) {
    const c = bucket(list, initialOf(w));
    return caseLike(w, c[anonHash(removeAccents(w.toLowerCase())) % c.length]);
}
/* Names: each word replaced by one with the same initial — particles (de, van…)
   kept. kind: first / last / full (first word a first name, the rest a last name). */
function anonName(v, kind) {
    let k = 0, last = null;
    return v.replace(/\p{L}[\p{L}'’]*/gu, (w, off) => {
        const low = w.toLowerCase();
        if (PARTICLES.has(low) || PARTICLES.has(low.replace(/’/, "'")) || (w.length <= 2 && k > 0 && low === low.toLowerCase())) return w;
        const hyphen = off > 0 && v[off - 1] === '-' && last;   // Jean-Pierre: both halves are first names
        const list = hyphen ? last : kind === 'first' ? FIRST_NAMES : kind === 'last' ? LAST_NAMES : (k === 0 ? FIRST_NAMES : LAST_NAMES);
        if (!hyphen) k++;
        last = list;
        return anonWord(w.replace(/[’']$/, ''), list);
    });
}
function anonDigits(s, rnd) { return s.replace(/\d/g, () => String(Math.floor(rnd() * 10))); }
function anonEmail(v) {
    const m = v.trim().match(/^([^@\s]+)@([^@\s]+)$/);
    if (!m) return anonText(v);
    const rnd = anonRng(anonHash(v.toLowerCase()));
    const local = m[1].replace(/\p{L}+|\d+/gu, p => /\d/.test(p) ? anonDigits(p, rnd) : anonWord(p, FIRST_NAMES).toLowerCase());
    return `${removeAccents(local)}@${DOMAINS[anonHash(m[2].toLowerCase()) % DOMAINS.length]}`;
}
/* Country codes of one or two digits; the others take three. */
const CC2 = new Set('20 27 30 31 32 33 34 36 39 40 41 43 44 45 46 47 48 49 51 52 53 54 55 56 57 58 60 61 62 63 64 65 66 81 82 84 86 90 91 92 93 94 95 98'.split(' '));
function ccLen(d) { return d[0] === '1' || d[0] === '7' ? 1 : CC2.has(d.slice(0, 2)) ? 2 : 3; }
/* The country code (after + or 00) and the first two national digits stay;
   the other digits are drawn from the hash, the spacing and punctuation kept. */
function anonPhone(v) {
    const digits = v.replace(/\D/g, ''); if (digits.length < 4) return v;
    const rnd = anonRng(anonHash(digits));
    let intl = 0;
    if (/^\s*\+/.test(v)) intl = ccLen(digits);
    else if (/^\s*00\d/.test(v)) intl = 2 + ccLen(digits.slice(2));
    const keep = intl ? intl + 1 : 2;
    let k = 0;
    return v.replace(/\d/g, ch => k++ < keep ? ch : String(Math.floor(rnd() * 10)));
}
/* Postal codes: the department (first two characters, three for the overseas
   97x / 98x) stays, the rest is redrawn — digits as digits, letters as letters. */
function anonPostal(v) {
    const s = v.trim(); if (!s) return v;
    const rnd = anonRng(anonHash(s)), keep = /^9[78]\d{3}$/.test(s) ? 3 : 2;
    let k = 0;
    return v.replace(/[\p{L}\d]/gu, ch => k++ < keep ? ch : /\d/.test(ch) ? String(Math.floor(rnd() * 10)) : caseLike(ch, String.fromCharCode(97 + Math.floor(rnd() * 26))));
}
function anonCompany(v) {
    const m = v.match(LEGAL_FORM), base = m ? v.slice(0, m.index) : v, form = m ? v.slice(m.index) : '';
    const w = base.trim(); if (!w) return v;
    const c = bucket(COMPANIES, initialOf(w));
    return caseLike(w, c[anonHash(removeAccents(w.toLowerCase())) % c.length]) + form;
}
function anonAddress(v) {
    const s = v.trim(); if (!s) return v;
    const h = anonHash(removeAccents(s.toLowerCase()));
    const m = s.match(/^(\d+)\s*(?:bis|ter|[a-z])?\b\s*(.*)$/i);
    const rest = m ? m[2] : s, type = rest.match(STREET_TYPE);
    let street = STREETS[h % STREETS.length];
    if (type) street = type[0] + street.slice(street.indexOf(' '));   // the original kind of way (avenue, chemin…) kept
    if (rest === rest.toUpperCase() && /\p{Lu}/u.test(rest)) street = street.toUpperCase();   // 12 RUE DES LILAS stays in capitals
    return (m ? `${1 + h % 150} ` : '') + street;
}
/* Dates move by up to a year either way, in their own format. */
function anonDate(v) {
    const s = v.trim(), p = parseDateCell(s, false); if (!p) return null;
    const shift = anonHash(s) % 731 - 365, d = new Date(Date.UTC(p.y, p.mo - 1, p.d) + shift * 864e5);
    const y = d.getUTCFullYear(), mo = pad2(d.getUTCMonth() + 1), dd = pad2(d.getUTCDate());
    if (/^\d{4}/.test(s)) return `${y}-${mo}-${dd}${p.rest}`;
    const sep = (s.match(/^\d{1,2}([-\/.])/) || [0, '/'])[1], yy = /^\d{1,2}[-\/.]\d{1,2}[-\/.]\d{2}(?!\d)/.test(s) ? String(y).slice(-2) : y;
    return `${dd}${sep}${mo}${sep}${yy}${p.rest}`;
}
/* Letters for letters, digits for digits: the shape stays, the meaning goes. */
function anonText(v) {
    const rnd = anonRng(anonHash(v));
    return v.replace(/[\p{L}\d]/gu, ch => /\d/.test(ch) ? String(Math.floor(rnd() * 10)) : caseLike(ch, String.fromCharCode(97 + Math.floor(rnd() * 26))));
}
const ANON_MODES = [['keep', 'Keep as is'], ['first', 'First name'], ['last', 'Last name'], ['full', 'Full name'], ['email', 'E-mail'], ['phone', 'Phone number'],
    ['postal', 'Postal code'], ['address', 'Address'], ['company', 'Company'], ['date', 'Date, shifted'], ['text', 'Scramble, same shape'], ['blank', 'Clear']];
const ANON_FN = { first: v => anonName(v, 'first'), last: v => anonName(v, 'last'), full: v => anonName(v, 'full'), email: anonEmail, phone: anonPhone,
    postal: anonPostal, address: anonAddress, company: anonCompany, date: anonDate, text: anonText, blank: () => '' };

/* What a column most likely holds, from its title and a sample of its values. */
function anonGuess(t, c) {
    const h = slugify(t.headers[c]), vals = [];
    for (const r of t.allData) { const v = cellStr(cellOf(r, c)).trim(); if (v) vals.push(v); if (vals.length >= 300) break; }
    const share = f => vals.length ? vals.filter(f).length / vals.length : 0;
    const has = re => re.test(h);
    if (share(v => /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(v)) >= 0.6) return 'email';
    if (has(/\b(tel|telephone|phone|mobile|portable|fax|gsm|cell|cellulaire)\b/) && share(v => parsePhoneCell(v.replace(/\p{Cf}/gu, ''), '33')) >= 0.4) return 'phone';
    if (has(/\b(code postal|codepostal|cp|zip|zipcode|postcode|postal)\b/)) return 'postal';
    if (has(/\b(adresse|address|rue|street|voie|adr)\b/) && !has(/\b(mail|email|e mail|ip|web|site|url)\b/)) return 'address';
    if (has(/\b(societe|company|entreprise|organisation|organization|raison sociale|employeur|employer|firme|compte|account|client)\b/) && !has(/\b(id|numero|number|no|code|type)\b/)) return 'company';
    if (has(/\b(prenom|prenoms|firstname|first name|givenname|given)\b/)) return 'first';
    if (has(/\b(nom complet|fullname|full name|contact|interlocuteur|dirigeant|responsable|signataire|commercial|manager)\b/)) return 'full';
    if (has(/\b(nom|noms|lastname|last name|surname|family name|name|patronyme)\b/) && !has(/\b(fichier|file|societe|company|produit|product|projet|project|domaine|domain|user|utilisateur|login|campagne|rue|etablissement)\b/)) return 'last';
    if (has(/\b(iban|bic|rib|siret|siren|nir|secu|securite sociale|passeport|passport|cni|permis|licence|vat|tva|numero de compte)\b/)) return 'text';
    if (has(/\b(naissance|birth|birthday|dob|ne le|nee le)\b/) && share(v => parseDateCell(v, false)) >= 0.6) return 'date';
    if (has(/\b(login|user|username|identifiant|pseudo|nickname)\b/)) return 'text';
    return 'keep';
}

let anon = null;      // { t, modes: [mode per column] }
function openAnon() {
    const t = T(); if (!t || !t.loaded) return;
    anon = { t, modes: t.headers.map((_, c) => anonGuess(t, c)) };
    document.getElementById('an-shown').checked = false;
    renderAnon();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-anon').style.display = 'block';
}
function anonSample(t, c, mode) {
    const f = ANON_FN[mode]; if (!f) return '';
    let v = '';
    for (const r of t.allData) { v = cellStr(cellOf(r, c)).trim(); if (v) break; }
    if (!v) return '<span class="empty">empty column</span>';
    const nv = f(v), cut = s => esc(s.length > 34 ? s.slice(0, 33) + '…' : s);
    return `<span class="src" title="${esc(v)}">${cut(v)}</span> → ` + (nv === null ? '<span class="empty">not recognised, kept</span>' : nv === '' ? '<span class="empty">empty</span>' : `<span title="${esc(nv)}">${cut(nv)}</span>`);
}
function renderAnon() {
    const { t, modes } = anon;
    document.getElementById('an-list').innerHTML = t.headers.map((h, c) => {
        const on = modes[c] !== 'keep';
        return `<tr class="${on ? 'on' : 'off'}"><td class="pf-name" title="${esc(h)}">${esc(h)}</td>`
            + `<td><select class="bs-input bs-select an-mode" onchange="anon.modes[${c}] = this.value; renderAnon()">${ANON_MODES.map(([v, l]) => `<option value="${v}"${v === modes[c] ? ' selected' : ''}>${esc(l)}</option>`).join('')}</select></td>`
            + `<td class="an-eg">${on ? anonSample(t, c, modes[c]) : ''}</td></tr>`;
    }).join('');
    const n = modes.filter(m => m !== 'keep').length, rows = document.getElementById('an-shown').checked ? t.filteredData.length : t.allData.length;
    document.getElementById('an-stats').innerHTML = n ? `<b>${fmt(n)}</b> column${n === 1 ? '' : 's'} over <b>${fmt(rows)}</b> rows. Empty cells stay empty; a value the treatment does not recognise is kept.`
        : 'No column selected: pick a treatment for the columns that identify people.';
    document.getElementById('an-go').disabled = !n;
}
function anonAllKeep() { anon.modes = anon.modes.map(() => 'keep'); renderAnon(); }
function anonGuessAll() { const { t } = anon; anon.modes = t.headers.map((_, c) => anonGuess(t, c)); renderAnon(); }

async function applyAnon() {
    const t = T(); if (!t || !anon || anon.t !== t) return;
    const cols = anon.modes.map((m, c) => m !== 'keep' ? [c, ANON_FN[m]] : null).filter(Boolean);
    if (!cols.length) return;
    const rows = document.getElementById('an-shown').checked ? t.filteredData : t.allData;
    closeAllModals();
    const ed = rowEdits(); let cells = 0, kept = 0;
    const touched = [];
    /* A value's fake is the same everywhere, so it is computed once per distinct value
       per column: names, cities and companies repeat across a file (capped, for a
       column of unique e-mails). */
    const memo = cols.map(() => new Map());
    visitRows(t, rows, r => {
        const d = r.data;
        for (let k = 0; k < cols.length; k++) {
            const [c, f] = cols[k], old = cellStr(d[c]); if (!old.trim()) continue;
            const m = memo[k];
            let nv = m.get(old);
            if (nv === undefined) { nv = f(old); if (m.size < 100000) m.set(old, nv); }
            if (nv === null) { kept++; continue; }
            if (nv !== old) { ed.set(r, c, nv); cells++; if (touched.length < 5000) touched.push([r, c]); }
        }
    });
    if (!cells) { setStats(`${t.name} | Nothing to anonymise in these columns.`); return; }
    const names = cols.map(([c]) => t.headers[c]);
    t.modificationsLog.push({ id: '-', col: '---', old: 'anonymise', new: `${cells} cells`, what: `${fmt(cells)} cells anonymised in ${cols.length === 1 ? names[0] : `${cols.length} columns`}`, undo: () => ed.undo() });
    flash(touched);
    updateSaveBtn(); renderTabBar(); render();
    setStats(`${t.name} | ${fmt(cells)} cells anonymised in ${names.join(', ')}${kept ? ` · ${fmt(kept)} values not recognised and kept` : ''} — not written yet, use Save (or Save as… to keep the original).`);
}
