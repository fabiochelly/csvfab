# The look of csvfab, zone by zone

What the web app draws (`viewer.htm`, `ui/app.css`, `ui/themes.css`), written for
the egui rewrite. Sizes are CSS px = egui points at zoom 1. Colours are fields of
`theme::Palette` (`p.surf_2`); `alpha(c, a)` and `mix(a, b, p)` are the helpers of
`theme.rs`; a CSS `rgba(var(--x-rgb), a)` is `alpha(p.x, a)`. Sizes that the code
needs are also in `theme::Metrics` (`m.row_h`); measured heights include the 1 px
rule that closes a band. Measured on Chromium at 1600 × 1000, GitHub Dark.

Fonts (`fonts.rs`): **mono** = `FontFamily::Monospace` (MonoLisa, Cascadia Code,
Monaco, JetBrains Mono, Consolas…), **ui** = `FontFamily::Proportional` (Inter,
the system UI font…), **ui 500** = `Name(fonts::UI_MEDIUM)`, **ui 600** =
`Name(fonts::UI_SEMIBOLD)`, **mono 600** = `Name(fonts::MONO_BOLD)`. The web page's
body font is the mono one: anything not said to be ui is mono.

Numbers shown to the user (row numbers, counts) are formatted French style: groups
of three separated by U+202F (narrow no-break space): `1 000 000`.

## Window, top to bottom

| zone | height | background |
|---|---|---|
| top band | 46 (`m.band_h`) | `bg_deep`, 1 px `brd` rule at its bottom |
| filter bar (a filter in force) | 34 (`m.chips_h`) | `mix(accent, surf, .05)`, 1 px `brd` rule at its bottom |
| find & replace bar (when open) | 50 + 1 | `bar_2`, 1 px `brd` bottom |
| grid header: titles | 33 (`m.header_h`) | `surf` |
| grid header: column charts (toggle) | 35 (`m.spk_h`): one block of 68 with the titles | gradient `surf` → `spk_bottom` |
| grid header: filter row (toggle) | 29 (`m.filter_h`) | `filter_bg` |
| rows | the density's: 28, 32 (default) or 38 (`m.row_h`) | stripes, see Grid |
| totals row (toggle) | as a row (`m.totals_h`) | `surf`, 1 px `brd_2` rule on top |
| horizontal scroll strip | 24 (`m.strip_w`) | `surf_2`, 1 px `brd` rule on top |
| status bar | 36 (`m.status_h`) | vertical gradient `status_a` → `status_b`, 1 px `status_brd` rule on top |

The vertical scroll strip is 24 px wide at the right edge, from the header's top to
the horizontal strip; the corner between the two strips is `surf_2`. Panel fill
behind everything: `bg`. Docked at the right of the grid, beside each other: the row card
and the column inspector.

## Top band

One band instead of a tab bar and a toolbar: 46 tall, `bg_deep`, padding 0 12, items
vertically centred, 8 apart (`m.band_gap`). Left to right: menu, tabs, then against the
right edge the search group, its toggles, Discard, Save.

**Narrow windows**: the right group and the toggles are measured first. The search field
keeps its 260 while the tabs can shrink; the tabs get what is left, at least 150 + 26. Under
that the field shrinks to 110, and Save loses its label below 200 of room for the field.

1. **Menu button** 32 × 32, radius 7: no fill at rest (hover / open: `surf_2`), a hamburger
   18 × 18 in `txt_mut` (hover / open `txt_hi`). Opens the main menu below it.
2. **Tabs**, 2 px apart:
   - **Tab**: 32 tall (`m.tab_h`), padding 0 8 0 10, gap 8 between its parts, max width 250
     (`m.tab_max_w`), radius 7 (`m.r_tab`), no border. No fill at rest, hover `surf_2` at
     .6; text ui 12.5, `txt_mut`.
   - **Active tab**: fill `surf_2`, text `txt_hi`.
   - Parts, left to right:
     1. dot 7 × 7, circle: `ok` (loaded); `prim` pulsing opacity 1 ↔ .25 over 1 s (loading);
        `danger` (could not be read).
     2. file name, ellipsis.
     3. meta: the row count, ui 11, `txt_dim` (`accent_2` on the active tab).
     4. dirty mark, a 6 px dot `warn`, only with pending edits.
     5. close ×: 16 × 16, radius 4, `txt_dim`; hover fill `danger`, cross `on_danger`.
   - After the tabs: **+** button 24 × 24, radius 5, `txt_mut`, 1 px transparent border; hover
     fill `surf_2`, `txt_strong`, border `brd`.
   - **Many tabs** (as the web's `overflow-x: auto`): the names shrink first (10 px steps,
     down to 60), then the tabs scroll sideways under the wheel, clipped at their room's
     right, the active one brought into view when it changes; a ▾ button (chevron 14,
     24 × 24 at the room's right end, + button's look) lists every tab (● before an unsaved
     one, ✓ on the active). Too little room for even the active tab: its name gives way,
     down to 16 px.
3. **Search group** (an input group, one frame):
   - ƒx button: 40 wide × 31, fill `addon_bg`, 1 px `brd`, radius 6 0 0 6; the ƒx
     glyph 22 × 19 in `txt_mut` (italic f joined to an x — draw it as a path, or
     "ƒx" in an italic serif 15 / 600). Hover: glyph `txt_hi`, border `prim`, fill
     `alpha(accent, .1)`.
   - field: 260 × 31 (`m.search_w`), fill `bg`, 1 px `brd`, radius 0 6 6 0,
     padding 6 × 12, ui 14 `txt_hi`, placeholder "Filter..." `txt_sub`. Expression
     mode: 340 wide, mono 13. Invalid regex / expression: border `danger`, 3 px ring
     `alpha(danger, .2)`.
   - focus: border `prim` and a 4 px ring `focus_ring` around the whole group
     (button included), the button's border `prim` too.
4. **Search toggles** (drawn by the user's design: neither their drawing nor their colours
   change): a frame 118 × 32, fill `bg_deep`, 1 px `brd`, radius 7, padding 0 3, toggles 2
   apart. Each toggle 26 × 28, a glyph 18 × 18 (stroke 1.35, round), no background ever:
   off `txt_dim`, hover `txt_hi`, on `accent_2` (Invert on: `orange_txt`). Glyphs: *ignore
   accents* "e" ui 600 17.5 with its acute accent stroke 1.9 in `tgl_acc_off` (on:
   `danger_txt`); *regex* a dot and an asterisk (`.*`); *invert* two horizontal lines crossed
   by a slash (≠); *expression* the ƒx glyph 20 × 17. Keyboard focus: 2 px ring
   `alpha(prim, .45)`, radius 4.
5. **Right group** (10 apart):
   - *Discard* (only with pending edits): outline button 34 × 30, padding 6 × 8, a ×
     16 × 16 stroke 2.4 in `danger`, border `alpha(danger, .5)`; hover fill
     `alpha(danger, .12)`, border `danger`.
   - joined group `[Save | ▾]`: outline buttons sharing a border (inner corners
     square, ▾ 1 px over Save). Icons 16 × 16 stroke 2. Extract is in ▾ (the user's
     choice: no toolbar button). *Save* with pending edits: text and border
     `warn` on Save and ▾, and a badge after "Save": the count, ui 11 bold, `on_warn`
     on `warn`, radius 12, padding 2 × 6. ▾: a chevron 16 × 12, padding 6 × 8.
6. **Progress bar**: 2 px at the band's bottom edge, `prim` with an 8 px `prim`
   glow, width = progress, fades out at the end.

## Filter bar

34 tall (`m.chips_h`), padding 0 12, under the top band and there only while a filter is in
force: one chip per filter, 6 apart, on one line; those that do not fit fold into a "+N" chip
(52 wide) listing them.

- chip: 24 + 2 px border tall (`m.chip_h` = 26), radius 12, padding 0 3 0 9,
  gap 4, ui 12 `txt_hi`, fill `chip_bg`, border `chip_brd`.
- text: **what** in ui 600 `txt_strong` + 2 px, then the value (cut at 28 chars
  with …). Inverted (the search and column-filter chips): border `orange_txt` (the
  Invert toggle's colour); the tooltip names the options (regex, accents and case,
  inverted).
- value filters (values hidden; a value or a chart part kept): `<column> ≠ v`,
  `<column> = v`, `<column> 3 values hidden`, `<column> 10 to 20` (the part, as its
  toast said it).
- ×: 18 × 18 circle, a cross 12 × 12 stroke 2 in `txt_mut`, hover `danger_txt`.
- Labels: `Search gmail`, `Expression {…}`, `<column> <text>` (column filter),
  value filters, irregular rows, duplicates, marked rows.
- with 2+ chips: a *clear all* icon after them, 24 × 24, a funnel with a × (16 px,
  stroke 2) in `txt_mut`, hover `danger_txt`.
- a new chip pops in (scale .96 → 1, opacity 0 → 1, 150 ms).

### Buttons and fields (everywhere)

- `.btn` (primary): fill `prim` (hover `prim_hover`), text `on_prim`, ui 500 13,
  padding 6 × 14 → 30 tall (`m.btn_h`), radius 6, icon + text 6 apart; disabled
  opacity .45. In egui: `Button::new(RichText::new(t).color(p.on_prim)).fill(p.prim)`.
- `.btn-outline` (the default egui Button after `theme::apply`): transparent, 1 px
  `brd`, text `txt`; hover fill `surf_2`, border `brd_2`.
- `.btn-warn`: fill `warn` (hover `warn_hover`), text `on_warn`. `.btn-danger`: fill
  `danger` (hover `danger_hover`), text `on_danger`.
- Field (`.bs-input`): fill `bg_deep`, 1 px `brd`, radius 6, padding 6 × 12, ui 14
  `txt_hi`, placeholder `txt_sub`; focus border `prim` + 4 px ring `focus_ring`.
- Select: a field with a chevron 16 × 12 (stroke 2, `#8b949e` on every theme) 12 px
  from the right, text padding-right 36.
- Checkbox / radio: 16 × 16, fill `bg_deep`, 1 px `brd_2`, radius 4 (radio: circle);
  hover border `txt_dim`; checked: fill and border `prim`, a white check (stroke 3 in
  a 20-unit box: `m6 10 3 3 6-6`) / a white dot r 2 of 8. Label 8 px after, ui 13
  `txt_hi`.

## Grid

### Geometry

- **Row-number gutter**: 92 wide (`m.gutter_w`) = 90 of cell + a 2 px `prim` rule
  on its right (`m.gutter_rule_w`), on every band (titles, charts, filters, rows,
  totals). Always pinned left. Fill `surf_2` (row hovered: `idx_hover`).
- **Columns**: auto width = widest value of the first 80 rows shown (mono advance ×
  chars) + 21 (2 × 10 padding + 1 border) + dot extra (13) or swatch extra (20),
  clamped to 60…480 (`m.col_min_w`, `m.col_auto_max`); fit on demand (double-click
  the resize handle) up to 900, the title's own width as the floor (title text in
  ui 600 12.5 + icon + number + 26 + 22).
- Every cell: 1 px `brd` rule on its right and at its bottom. Cell padding 10 each
  side (`m.cell_pad_x`). Text mono 13, vertically centred in the row above the rule,
  no ligatures. A value wider than the cell is cut with an ellipsis "…".
- **The column in hand** (`GridState::focus_col`, the last title clicked or the cursor's
  column, not the last column of a whole row selected): its title, chart and cells get a
  faint `alpha(accent, .07)`. A click on a title still sorts: nothing "selects first".

### Title line (33)

The title line and the charts are one block, 68 tall; no rule between them. The charts off,
the line stands alone, 33 with its rule. The parts below (type badge, sort arrow with its
grey ghost on hover, ▾) are the user's: not to be redrawn or dropped.

- Cell: padding 0 10, fill `surf`, ui 600 12.5 `txt_hi`, letter-spacing .01em.
  Hover over the title text: `txt_strong`. Parts, left to right:
  1. **type icon**, 16 tall, min 16 wide (padding 0 3), radius 4, 7 px before the
     text, mono 700 10: numbers `#` in `cyan_txt` on `alpha(cyan, .13)`; dates a
     calendar 11 × 11 (stroke 1.8) in `violet_txt` on `alpha(violet, .13)`; text `Aa`
     in `txt_dim` on `alpha(gray, .16)`. Rich columns (links ↗, e-mails @, yes/no ✓,
     colours ◐, JSON {}) show their glyph in mono 700 `accent_2` on
     `alpha(accent, .14)`, the box as wide as "Aa".
  2. the title, ellipsis.
  3. **sort arrow** (sorted columns): 5 px after, an arrow 12 × 12 (up: ascending,
     down: descending; stroke 2.2) in `prim`; in a multi-key sort its rank after it as
     a superscript ui 600 9 (`↑1`, `↓2`). In a sorted title the column number gives
     way first, then the title; the icon and the arrow stay.
  4. **column number** (0-based), 6 px after, ui 10.5 `txt_faint`, tabular.
  The text stops 26 px before the right edge (`m.col_title_pad_r`).
- **▾ button**: 20 × 20 (`m.col_menu`), 7 px from the right edge, centred, radius 4; a
  chevron 13 × 13 (stroke 2) in `txt_dim`; hover / open fill `surf_3`, chevron
  `txt_hi`. With a value filter on the column: a filled funnel in `prim`. Opens the
  column panel.
- **Resize handle**: the last 5 px of the cell, full header height; hover fill `prim`.
  Cursor col-resize.
- Gutter cell of this row: a "sliders" icon 16 × 16 (stroke 2) in `txt`, centred
  (opens the columns manager), fill `surf_2`.
- Synthetic titles (file without a header line): italic, `txt_dim`, no number.
- Drag a title to move the column: drop marker = 3 px `prim` inset bar on the left or
  right of the target title; the dragged title at opacity .45. Within 32 px of the
  scrolling part's left or right edge (or past it) the columns scroll under the pointer,
  faster the closer (up to 900 px/s past the edge).

### Column charts (35, under the title line)

- Fill: vertical gradient `surf` → `spk_bottom`, carrying on from the title line. Each cell:
  padding 1 top, 10 sides, 7 bottom → a box 27 tall (`m.spk_box_h`).
- Gutter cell: a bar-chart icon 15 × 15 in `txt_faint` (hover `txt_mut`), toggles
  the band.
- **Number / date column** (`kc` = `num_txt` / `date_txt`):
  - the curve: from the box's top to 3 px above the gauge (20 tall).
    32 bins of the values, as a smooth monotone area (Fritsch–Carlson through the
    bins' centres, a non-empty bin at least 7 % tall): filled with a vertical gradient
    `kc` → `alpha(kc, .28)` at opacity .55, under a 1.6 px line in `kc` (the line's
    width in px, whatever the stretch). Whole picture at opacity .85, 1 on hover.
    Few distinct values: bars instead (one per value, 76 % of the slot wide, tops
    rounded).
  - the gauge: 4 tall (`m.spk_gauge_h`) at the box's bottom, radius 2, filled part
    `gauge_fill` (share of non-empty cells), rest `gauge_empty`, each part at least
    6 px. Hovered part: `ok` (empty part: `alpha(gray_hi, .55)`).
- **Text column**:
  - the ribbon: 4 tall at the bottom, radius 2: the 5 most frequent values (held
    twice or more) in `p.pill[0..5]` order, then the others `ribbon_other`, then the
    empty cells `ribbon_empty`; widths by share, each ≥ 6 px, 2 px gaps between parts.
  - the label above it, one line centred over the 21 px left, ui 500 11.5 `txt_sub`: if one
    value holds ≥ half of the others: a 6 px dot in its ribbon colour, 5 px, the value in ui
    600 `txt_hi` (≤ 70 % of the width, ellipsis), 5 px, its share ("26 %") in
    `txt_faint`. Otherwise, ui 500 10.5: "4 001+ values" (or "all distinct") in `txt_sub`, a
    2 px dot, "100 % filled" in `alpha(ok_rgb, .85)`; as the column narrows, "filled" goes,
    then the share. Nothing filled: "empty" in `txt_faint`.
- Under a filter: the whole file's curve stays faint behind (opacity .3), the rows
  shown drawn on top in full colour, at their own scale (their shape, however few);
  gauges, text ribbons and labels count the rows shown.
- A value filter set by clicking the chart: the picked bars' span gets a band
  `alpha(prim, .2)` over the curve's height.
- Hovering a bar / ribbon part: the rest of the box at .35 (.4 for the ribbon), the
  part in full colour; tooltip with the values and the count. Arrival animation:
  bars rise (scaleY 0 → 1) and ribbons unroll, 700 ms, 28 ms stagger per column.

### Filter row (29)

- Cell: padding 0 5, fill `filter_bg`.
- Box: 22 tall (`m.filter_box_h`), full cell width, radius 5, fill `bg`, 1 px
  transparent border; a magnifier 11 × 11 (stroke 2.5, colour `#6e7681` on every
  theme) 7 px from the left, centred; text ui 12 `txt_hi` from 24 px in, 8 px right
  padding; placeholder "Filter" `txt_faint`. Hover border `surf_3`. Focus border
  `prim` + 3 px ring `focus_ring_sm`. Holding a value: border `alpha(prim, .55)`, fill
  `alpha(prim, .1)` over `bg`. Not compiling (`>abc`): border `danger`, fill
  `mix(danger, bg, .08)`, the reason as its tooltip and in the status line ("Filter not
  applied — column "total": not a number: abc").
- Gutter cell: a funnel 13 × 13 in `txt_faint` (hover `txt_mut`), hides the row.

### Rows

**Density** (`theme::DENSITIES`, `Metrics::set_density`, remembered in the config): Compact
28, Normal 32 (default), Airy 38, the 1 px rule included; the totals row follows. The row on
top stays on top when it changes.

- **Stripes by record**: a row's fill is `row_alt` when its record id (the file row
  number shown in the gutter) is even, `row_bg` when odd — not by position on screen,
  so a sort or a filter keeps each record's shade. `p.row_fill(even, hovered,
  current)`.
- Hovered row: `row_hover_alt` / `row_hover_bg` (12 % accent, opaque).
- Row of the active cell: `row_cur_alt` / `row_cur_bg` (16 % accent).
- **Gutter cell**: the record id centred, mono 12 `txt_dim`, `accent_2` on a row selected
  whole (formatted `1 164`). On row
  hover a ⋮ button at its right (4 px in): 18 × 22, radius 4, three dots r 1.3 in
  `txt_mut` 4.5 apart; hover / open fill `surf_3`, dots `txt_strong` (row menu).
- Marks on the gutter cell: irregular row (field count ≠ titles): 3 px `warn` bar
  inside its left edge; row marked by review / compare: 3 px `warn` bar inside its
  right edge; duplicate groups: 3 px bar at its right edge alternating `violet_txt` /
  `cyan_txt` group by group, and a 2 px `alpha(violet, .5)` line at the top of a
  group's first row.
- **Cell text colours**: text `txt`; numbers right-aligned in the mono font's tabular
  digits, `num_txt`, the unit (€, $, £, ¥, %) in `txt_dim` (not at the raw level); dates
  `date_txt` (type colours can be switched off from the status bar: all `txt`);
  links / e-mails `accent_2` (underlined, 3 px offset, while Ctrl is held);
  JSON `txt_mut`; a cell whose column is lit by a chart hover: `txt_strong`, mono 600.
- **Categories** (a text column of ≤ 12 distinct values averaging ≤ 24 chars): a dot 7 × 7
  (`m.dot_d`) at the cell's 10 px padding, colour `c`, then 6 px on the text in ui 13 `txt`.
  `c` = `p.pill[k]` for the column's k-th most frequent value (k < 5), `pill_other`
  otherwise (`p.pill_colors(slot).0`). Lit by a chart hover: the dot 1 px wider, the text
  `txt_strong`. The raw colour level shows the plain value, as before.
- **Yes / no** columns: no text; a disc Ø 15 `prim` with a check (`on_prim`, two
  strokes 2 px) for yes; a ring Ø 15, 1 px `txt_faint`, for no; centred.
- **Colour** columns: a swatch disc r 5.5 at x = 17 (its colour, a 1 px
  `alpha(hi, .35)` ring), the text from 30 px in.
- **Search highlight** in cells (global search, column filters): the matched part's
  text `mark_txt` on `match_bg`, radius 3, 2 px horizontal padding. A find's matches:
  same with `find_bg`.
- Newline in a value: shown as `↵` in ui 11 `txt_dim`.
- **Selection** (range): every selected cell gets `sel_fill` over the row colour.
  The active cell: a 2 px inset frame `cursor` (`prim`). The range's bottom-right cell
  carries the fill handle: a 7 × 7 square `prim`, 1 px `bg` border, radius 1, in that
  cell's bottom-right corner (cursor crosshair). While dragging the handle, the target
  cells: `fill_preview` + 1 px inset `fill_preview_brd`.
- Marked cells (review / compare): `mark_fill`. Cells differing within a duplicate
  group: fill `danger_subtle`, text `diff_txt`. A cell just edited flashes
  `flash_fill` fading out over 1.2 s.
- **Frozen columns**: drawn right after the gutter, not scrolling; opaque: their
  background is the row's own (and `sel_frz_alt` / `sel_frz_bg` when selected,
  `mix(warn_rgb, row, .16)` when marked); the last frozen column's right rule is
  `frz_rule` (the colour only, still 1 px).
- **Sorted view with groups** (the first sort key changes value less often than every
  other row): a 1 px `group_line` at the top of each row whose sort value differs from
  the previous row's; and the **group tag** floating 4 px under the header, over the
  sort column (6 px inset each side, hidden when < 40 px): the value at the top of the
  view (or "(empty)"), ui 600 12 / 18 line `txt_strong`, padding 1 × 9, radius 10,
  fill `alpha(accent, .22)`, 1 px `alpha(accent, .45)`.
- Row-number drag: selects rows; dragging a row already selected moves it (the user's
  choice, as Excel; not 1.x). Drop marker 2 px `prim` line at the top / bottom of the
  target row; the dragged row at .45.
- Cell editor (double-click / typing): a box over the cell (grows with its content),
  padding 7 × 10, mono 13 / 19 line, text `txt_strong`, fill `bg`, 2 px `prim`
  border, radius 3, shadow 0 8 24 `rgba(0,0,0,.5)`; the cell's own text hidden.

### Totals row (a row's height, toggle)

- Stuck to the grid's bottom (right under the last row when all rows fit). Top rule
  1 px `brd_2`, fill `surf`, a shadow upwards (0 −4 12 `rgba(0,0,0,.18)`).
- Cell (the row less its rule): padding 0 10, right rule `brd`; left the function's name ("SUM",
  "AVG", "COUNT"…) ui 600 10 uppercase, letter-spacing .05em, `txt_dim`; right the
  value mono 600 13 `txt_strong` (`num_txt` / `date_txt` by the column's kind),
  ellipsis; at .45 opacity while being recounted. Hover fill `surf_2`. A cell with no
  function shows "Σ" ui 600 13 `txt_faint` on hover.
- Gutter cell: "Σ" ui 600 15 `txt_mut`, centred.

### Scroll strips (over the scrollbars)

- **Vertical**, 24 wide (`m.strip_w`) at the grid's right edge, from the header's top
  to the horizontal strip: fill `surf_2`, 1 px `brd` rule on its left.
  - thumb = the viewport: 2 px from each side (20 wide), at least 22 tall, radius 5,
    fill `thumb`, 1 px `thumb_brd`; strip hovered or dragged: `thumb_hot` /
    `thumb_hot_brd`. Click the track: jump; drag: scroll.
  - labels every 44 px, centred, ui 600 9 `txt_dim` (sorted view: `txt_mut`): sorted
    by text → initials (A, B, C…), by date → years, by number → compact values,
    otherwise row numbers.
  - marks (a canvas, left part): irregular rows `warn`, duplicate groups, marked rows,
    the selection's band (3 px `prim` at the left edge); find matches as ticks on the
    right edge.
  - tip while hovering / dragging, 8 px left of the strip: row number (and the sort
    value there), ui 12 `txt_hi`, fill `surf`, 1 px `brd_2`, radius 6, padding 4 × 9,
    shadow 0 6 20 `rgba(0,0,0,.4)`.
- **Horizontal**, 24 tall above the status bar, full width: fill `surf_2`, 1 px `brd`
  on top. One label per column, as wide as the column's share of the total width:
  1 px `brd` on its left, padding 0 3, ui 600 9 `txt_dim`, centred, line 23, ellipsis;
  the gutter's slot shows row numbers in ui 400 `txt_faint`; sorted columns in `prim`.
  Thumb: 2 px from top and bottom, same colours as the vertical one; selection band:
  3 px `prim` on top.

## Status bar (36)

The pills stay pills (the user's choice, not the first draft's plain text). Gradient
`status_a` → `status_b`, 1 px `status_brd` on top, padding 0 12 0 18, items
14 apart, ui 12 with tabular figures. Left to right:

1. **Message** (takes the room left, ellipsis): `status_fg`. E.g. `customers-1m.csv |
   5 × 3 selected · Sum 1 688 406 640 110 · Avg 337 681 328 022 · 5 numbers`,
   `customers-1m.csv | filtered`, `Ready.`.
2. **Chips** when relevant, padding 1 9 (with ×: 1 4 1 9), line 16, radius 10, ui 12:
   - irregular rows / garbled accents: text `warn`, fill `alpha(warn_rgb, .08)`
     (hover .16), border `alpha(warn_rgb, .45)`; on: fill `warn`, text `on_warn`.
   - duplicates: `violet_txt` on `alpha(violet, .08)`, border `alpha(violet, .45)`;
     on: fill `alpha(violet, .3)`, text `txt_strong`.
   - marked rows: as irregular, max 320 wide.
   - their × part: padding 0 5, radius 8, hover fill `alpha(hi, .2)`.
3. **Cursor position** (when a cell is active), `status_fg`, 10 px right margin.
4. **Counts**: `8 261 / 1 000 000 rows · 15 columns`: the shown count (only when a
   filter hides rows) ui 600 `stat_filt`, " / ", the total ui 600 `stat_total`, the
   rest `status_fg`; hidden columns "(2 hidden)" in `txt_dim`.
5. A separator 1 × 14 `status_sep` (2 px margins), then the **format pills**, 8 apart:
   - pill: 22 tall (`m.sb_pill_h`), radius 11, padding 0 11, gap 6, ui 500 11.5;
     neutral: text `txt_mut`, fill `alpha(gray, .12)`, border `alpha(gray, .25)`
     (hover .22 / .45); open (its menu shown): border `prim` + 3 px ring
     `focus_ring_sm`.
   - delimiter: a key cap first (mono 700 12, line 14, min 8 wide, padding 0 4, radius
     4, 1 px border in the pill's text colour at 45 %): `,` `;` `⇥` `|` `¶`, then the
     name (Comma, Semicolon, Tab, Pipe, Raw text) and "AUTO" when detected (ui 600 9.5,
     uppercase, letter-spacing .06em, opacity .6). Colours: comma `cyan_txt` on
     `alpha(cyan, .12)` border `alpha(cyan, .4)`; semicolon `violet_txt` / `violet`
     .14 / .4; tab `orange_txt` / `orange` .13 / .4; other `pink_txt` / `pink`;
     raw text `ok_txt` / `ok_rgb`.
   - encoding ("UTF-8 AUTO", "UTF-8 BOM"): UTF-8 `ok_txt` on `alpha(ok_rgb, .13)`;
     Windows-1252 / Latin `warn` on `alpha(warn_rgb, .12)`; Mac `cyan_txt`; UTF-16
     `accent_2` on `alpha(accent, .13)`, borders at .4.
   - "Titles on line 1 AUTO" / "No titles": neutral.
   - line endings "CRLF" / "LF" / "CR": neutral, click switches.
   - the pills' menus open upwards, above the clicked pill (`.dd-menu`, min 210).
6. **Density**, 14 after the pills (when 900 are left for it: the Density menu has it
   anyway): a segmented control 24 tall, radius 7, `alpha(gray, .12)`, padding 2, its three
   names ui 500 11.5 `txt_mut` (hover `txt`), each padding 0 8, 20 tall, radius 5; the one in
   force `surf_3`, `txt_hi`.
7. **Switches**, each 6 px after the previous (so 14 apart), scale 1.12 on hover, off
   = greyscale at .45 opacity:
   - column inspector: a window with a pane at its right, 16 × 16 in `accent_2`;
   - totals row: a Σ 15 × 15 (stroke 2) in `orange_txt`;
   - filter row: a filled funnel 15 × 15 in `accent_2`;
   - column charts: bars 16 × 16 in `cyan_txt`;
   - type colours: an 18 × 18 disc, conic gradient `danger_txt`, `orange_txt`, `warn`,
     `ok_txt`, `cyan_txt`, `prim`, `violet_txt`, `pink_txt`, back to `danger_txt`; a 1 px
     `alpha(hi, .3)` ring (hover .5, scale 1.1);
   - theme: an 18 × 18 disc split on the 135° diagonal, top-left `bg`, bottom-right
     `theme.swatch`, ring `alpha(hi, .3)`; open: 1 px `prim` + 4 px
     `alpha(prim, .25)` ring. Opens the theme menu (upwards).

## Menus

- **Frame** (`theme::menu_frame`): `surf`, 1 px `brd`, radius 8, padding 6, shadow 0 10
  30 `rgba(0,0,0,.5)`; opens 6 px below its button, at least as wide as it. Appears
  with scale .96 → 1 and fade, 140 ms.
- **Item**: padding 7 × 10 (30 tall), radius 5, ui 13 `txt_hi`, label left / shortcut
  right at least 24 apart; an icon 16 × 16 (stroke 2) in `txt_mut` 10 px before the
  label (hover: `txt_hi`); hover fill `surf_2`. Shortcut: ui 11.5 `txt_dim`
  ("Ctrl+O"). Disabled: opacity .4. Danger items (Quit, Delete the row): text and
  icon `danger_txt`, hover fill `alpha(danger, .14)`.
- **Section head**: padding 6 10 4, ui 600 11, uppercase, letter-spacing .04em,
  `txt_dim` ("EMAIL · ROW 965").
- **Separator**: 1 px `brd`, margins 5 vertical, 4 horizontal.
- **Submenu**: the item ends with "›" ui 12 `txt_dim` (hover `txt_hi`); its menu opens
  2 px to the right, its first item level with the parent item (top −7), min 240
  wide, while the pointer is on the item or the submenu.
- **Button row** (`Row: [Go to…|Card]`, `Edit: [Undo|Redo]`): the label (icon +
  text, as an item, padding-left 10) left; right a joined group: 1 px `brd`, radius 6;
  buttons padding 4 × 10, icon 14 `txt_mut` + text `txt_hi` 6 apart, 1 px `brd`
  between them; hover fill `surf_2` (icon `txt_hi`); disabled text and icon
  `txt_faint`.
- Selected entries (theme menu, options): a check 14 × 14 at the item's right
  (hidden when off); `.sb-opt.on`: text `txt_strong` ui 500.
- **Context menus** (cell, row ⋮, totals): the same frame, min width 230, at the
  pointer; first line a section head naming what was clicked.
- **Main menu** (from the menu button), in order: Command palette… (Ctrl+P) ·
  sep · Open (Ctrl+O) · Row [Go to… | Card] · File profile… · Look [Map | Insights] ·
  Schema [Create… | Validate…] · sep · Edit [Undo | Redo] · Review changes… · sep ·
  Find & replace (Ctrl+F) · Transform the file › · sep · Columns › · Rows › · sep ·
  Density › (Compact, Normal, Airy) · Column inspector ✓ · sep ·
  Help [Shortcuts | About…] · Quit (Ctrl+Q, danger).

## Column panel (▾ of a title)

300 wide, under the ▾ (`menu_frame` look, padding 6, ui 13 `txt_hi`):

- a tools row (wraps): small outline buttons padding 4 × 8, ui 12, icon-only ones
  min 26 wide (padding 4 × 6, icon 14); joined groups (move ‹ ›, split / merge,
  sort, + −); a toggled tool: border `prim`, text `accent_2`, fill `alpha(accent, .12)`;
  delete hover: `danger_txt`, border `alpha(danger, .6)`.
- profile block, padding 8 × 10, ui 12 `txt_mut`, line 1.6, values in ui 600 `txt_hi`:
  a type badge (padding 0 8, radius 9, ui 600 11, line 18, 1 px border: text
  `violet_txt` / `alpha(violet, .13)` / `.4`; numbers `ok_txt` / `ok_rgb`; dates
  `accent_2` / `accent`), "5 distinct values on 8 261", min / max lines (key 30 wide
  `txt_dim`). Numbers get a histogram: 44 tall, bars 1 apart, `prim` at .75 (1 on
  hover), tops radius 1.5.
- a search field (padding 5 × 10, ui 13), then the values list (max 320 tall,
  scrolls): rows padding 4 × 6, radius 4, checkbox + value + count; behind each row a
  bar of its share in `alpha(accent, .11)`; count ui 11.5 `txt_dim` right; hover fill
  `surf_2`; "(empty)" in italic `txt_mut`. First row "(Select all)" with the total.
- actions: 1 px `brd` line above, padding 8 4 2, right-aligned: Clear (outline),
  Apply (primary).

## Column inspector

The column panel, docked: at the right of the grid (left of the row card), 320 wide
(`m.inspector_w`), `surf`, 1 px `brd` along its left edge, padding 12 6 0 16. Off by default,
remembered in the config; its switch is in the status bar, also the main menu, its × closes it.
Off, one flag is read a frame, and nothing is kept. It needs 300 px left for the grid, else
it stands aside. It follows the column in hand; with none, the first column shown.

- head: the name ui 600 15 `txt_hi`, under it ui 12 `txt_mut`, "Numbers · 4 747 distinct ·
  3 545 of 5 000 rows" (the last only under a filter); a × 26 × 26, radius 6, right.
- key figures, three a line, 40 tall: a label ui 11.5 `txt_dim` over its value mono 12.5
  `txt_hi` (cut with …, in full as its tooltip): Filled, Distinct, Empty, then numbers Min,
  Median, Mean, Max, Sum; dates First, Median, Mean, Last; text Shortest, Longest. A number's
  median goes to the row closest to it, as in the panel.
- numbers and dates: the panel's histogram, 92 tall, its two ends mono 11 `txt_mut` under it.
- other values repeating: "Top values", the 8 most frequent (30 tall, 2 apart, radius 6,
  `surf_2`, a bar of its share `alpha(accent, .14)`, .32 when it is the filter in force; the
  category's dot, the value ui 13 `txt`, the count mono 12 `txt_mut` right), "and N more". A
  click sets the value filter a chart's part sets (same chip, same message), again: off.
- "Worth a look" (ui 500 11.5 `txt_dim`, as every head here): lines ui 12.5 `txt`, up to two
  lines, "Show" ui 500 12.5 `accent_2` at the right: empty cells, values not of the column's
  kind (both from the count, shown by a value filter), then up to 4 Insights cards naming
  the column first (outliers, values written otherwise…), shown as the dialog does. "Nothing
  stands out." when there is nothing.
- "Actions": the panel's tools row, and for numbers its data bars and colour scale.

Everything comes from jobs: the panel's count once the column has been in hand for 150 ms
(the dictionary cache of `colcache` under it, the last 4 counts kept while they hold under a
million values), the Insights report shared with its dialog, 1.5 s after the table last
changed. "Counting…" until the first count; the last count of a column stays up while the
next one is made.

## Row card (Ctrl+I)

Docked at the right of the grid (`egui::Panel::right`, 430 wide by default,
resizable 320–760), `surf`, 1 px `brd` along its left edge, padding 10 14 0 14:

- head 26 tall: "Row 1 234" (ui 600 14.5 `txt_hi`, "New row" for a row not in the
  file yet), then right-aligned small outline buttons 24–28 tall, radius 6: ▲ ▼ (Alt+↑ /
  Alt+↓), Copy (the row as "name: value" lines), Raw (a switch: on = border `prim`,
  fill `alpha(prim, .14)`, text `accent_2`), Why? (the row's diagnosis, beside the
  card), ×.
- under it, ui 11.5 `txt_dim`, parts joined by `txt_faint` " · ": "4 of 2 743 shown",
  "line / record 5 in the file" (`warn` "new row, not in the file yet"), `warn` "13
  fields for 12 columns", `accent_2` "edited"; then a 1 px `brd` rule across.
- Raw (when on): a box `bg_deep`, 1 px `brd`, radius 6, padding 8 10: a ui 11.5
  `txt_dim` line (bytes from–to, fields, the encoding read); the record in mono 12,
  delimiters bold `orange_txt` on `alpha(orange, .18)`, quotes bold `violet_txt`, line
  breaks ↵ and invisible characters (`U+00A0`…) mono 10.5 `warn` on `alpha(warn, .14)`;
  scrolls past 36 % of the card; a `warn` legend of the invisible characters found.
- a search field 30 tall ("Find a field…", names and values, accents ignored, "3 of
  85" at its right, × to clear) and an eye switch beside it hiding the empty fields.
- the fields, in their own scroll (solid bar 8, always shown): per column, the name
  (the column's type badge 16 tall as the header's, ui 12.5 `txt_mut`; the cursor's
  column ui 600 `accent_2` with a 3 px `prim` bar at the left and the line in
  `alpha(prim, .07)`; hidden: `txt_faint` and an eye-off; an edited cell: a 3 px
  `accent` dot), as wide as the widest name up to 38 % of the card; the value in mono
  12.5 (numbers `num_txt`, dates `date_txt`, links `accent_2`, as the grid), on a box
  radius 5 `bg_deep` (a shade lighter at rest), 1 px border on hover, `prim` and a 3 px
  `alpha(prim, .22)` ring while typed in; empty: "empty" in `txt_faint`. Rules
  `alpha(brd, .6)` between fields. Click a name: the cell in the grid; right click:
  show in the grid, copy value / name, hide or show the column.
- keys in a field: Enter writes and goes to the next field, Tab / Shift+Tab next /
  previous, Shift+Enter a line break, Escape gives the value back.
- no row: a 64 disc `alpha(prim, .10)` with the card icon in `accent_2`, "No row
  selected" (ui 600 15), a line `txt_dim`, a "Show the first row" button, the keys in
  `txt_faint`.

## Dialogs

- **Message dialog** (confirm / alert / prompt, `theme::dialog_frame`): width
  min(440, window − 32), centred, padding 22 22 18, radius 10, `surf`, 1 px `brd`,
  shadow 0 18 50 `rgba(0,0,0,.55)`. Message ui 600 14.5 `txt_hi`, line 1.45; detail
  ui 13 `txt_mut`, line 1.5, 8 below; a field 14 below (full width); buttons
  right-aligned, 8 apart, 20 below: Cancel outline, OK primary (or danger). Appears with
  scale .97 → 1, 140 ms.
- **Form dialogs** (`.modal-box`): same colours, radius 8, padding 20, max 80 % of the
  window tall (scrolls); title ui bold 16 (use ui 600) with 14 below; field labels ui 12 `txt_mut`, 5
  above the field; notes ui 12 `txt_mut` line 1.45; preview tables ui 12, cells
  padding 4 × 8, 1 px `brd` rules, sticky head `surf`. Widths: save / duplicates /
  bulk 380, clean 520, split / merge / formula… 560, combine 640.
- **Backdrop**: `backdrop` (`bg_deep` at .45) over the whole window; the web blurs
  what is behind (6 px) — without blur, use `alpha(bg_deep, .6)` to keep the dialog
  readable. Command palette and function picker backdrop: `alpha(bg_deep, .55)`.
- **Command palette** (Ctrl+P): 660 wide, 11 % from the top, radius 14, `surf`, 1 px
  `brd_2`, shadow 0 28 70 `rgba(0,0,0,.6)`; input row 52 tall (magnifier 18 `txt_dim`,
  ui 16 `txt_hi`, placeholder `txt_dim`), 1 px `brd` under; list padding 6, max 480
  tall: items padding 8 × 12, radius 8, ui 13.5 `txt`, the matched letters ui 700
  `accent_2`; selected: fill `alpha(accent, .14)`, text `txt_hi`, a 3 px `prim` bar at
  its left (7 px from top and bottom); group tag right ui 11 `txt_dim` on
  `alpha(gray, .15)` radius 9 padding 1 × 7; keys as caps mono 11 padding 1 × 5 radius 4,
  1 px `alpha(hi, .2)`, fill `alpha(hi, .06)`, `txt_mut`. Footer 1 px `brd` above,
  padding 8 × 16, ui 11.5 `txt_dim`.

## Toasts

Bottom-right, 16 from the right, 48 from the bottom (above the status bar), newest
at the bottom, 8 apart. Each: min 260, max 440 wide, padding 10 10 10 12, radius 9,
`surf`, 1 px `brd` and a 3 px left border in the kind's colour (`prim`; `ok`,
`danger`, `warn`), shadow 0 12 32 `rgba(0,0,0,.45)`, ui 13 `txt_hi` line 1.4, 10
apart: an icon 16 × 16 in the kind's colour, the message, an **Undo** button (ui 600
12.5 `accent_2`, fill `alpha(accent, .12)` hover .25, 1 px `alpha(accent, .35)`,
radius 6, padding 3 × 10) while it is the last edit, a × 20 × 20 radius 5 ui 15
`txt_dim` (hover fill `surf_2`, `txt_hi`). In: rise 12 px + fade, 260 ms with a slight
overshoot; out: slide 24 px right + fade, 200 ms.

## Tooltips

Few controls have one: a button without a label (its name, its shortcut), a value cut
short (in full), or a hint that says what the label cannot (what a status chip
counts, what a click on it does). Never a restatement of the label, nor how to click
(F1 lists that). Every hint shows once the pointer rests on its element for
`grid::TIP_DELAY` (0.3 s): egui's (`show_tooltips_only_when_still`) and the grid's and
charts' own (`grid::tip`); sliding on to the next one while one shows, at once. Under the element
(above when no room), 10 px away and clear of the pointer, centred on it, kept 6 px
inside the window. Box (`theme::tooltip_frame`): max 420 wide, padding 4 × 8, radius
5, `tip_fill` (the web blurs behind it, 14 px), 1 px `tip_stroke`, shadow 0 4 14
`rgba(0,0,0,.35)`, text ui 11.5 / 1.45 `tip_fg`, line breaks kept. egui's own
tooltips use the menu frame; draw these with `tooltip_frame` (an `Area` at the
computed position) to get the darker box.

**Cell peek** (resting on a cut cell): the same box, max 560 wide and 60 % of the
window tall, padding 7 × 10, mono 12.5 / 1.5, wrapped, fade-in 120 ms.

## Find & replace bar

50 tall + 1 px `brd`, `bar_2`, padding 0 20, items 15 apart: "Find & replace in the
**rows shown**:" ui 13; find field; "‹ n / N ›" (arrows 24 × 26 outline radius 5 ui 18,
the count ui 12 `txt_mut`, `warn` when none); replace field; column select (max 220);
Regex and Match case checkboxes ui 13; **Apply** (`.btn-warn`, a lightning icon 14);
the count ui 12 `txt_mut`. Invalid find: field border `danger`, "invalid" in `warn` as the
count, and the reason ("This regular expression is not valid: unclosed group") in ui 12
`warn` where the count of a replace goes (cut with …, in full as its tooltip). Regex
matches are tinted where they match, not the whole cell.

## Welcome screen (no file)

`bg` with a faint dot texture (`alpha(hi, .035)`); a centred column 580 wide, 8 % from
the top: logo 54 + "csvfab" ui 300 36 in a gradient `prim` → `violet` → `pink`, 44
below: a drop card (radius 20, padding 30 24 28, 1 px `alpha(hi, .14)` (hover .24),
fill `alpha(bg, .08)`): a 50 px `prim` disc with an upload arrow `on_prim`, "Drop files
to open them" ui 600 15.5 `txt_hi`, "CSV, TSV, Excel or JSON — or click to choose" ui
12.5 `txt_mut`; 28 below two keys buttons (34 tall, radius 8, padding 0 12, ui 13
`txt_mut` hover `txt_hi`, same glass border) "Command palette Ctrl+P", "Shortcuts F1"
(key caps mono 11.5, line 18, padding 0 5, radius 5); recent files card (radius 14,
rows padding 10 × 14 with a green file icon `ok_rgb`, name ui 500 13.5 `txt_hi`, path
ui 11.5 `txt_dim`, size right and when it was last opened under it (`txt_faint`), × on
hover; up to 12 as fit; a file no longer there: name `txt_faint`, "missing", a click says
so and forgets it; sizes looked up in the background, never on a frame); footer "open · filter · clean · rewrite in
place" ui 12, letter-spacing .04em, `txt_faint`, dots `alpha(accent, .6)`.

## Monospace font dialog

460 wide (`dialog_frame`): title ui 600 15, a line ui 12.5 `txt_dim`, the sample box
(`bg_deep`, radius 8, the sentence and the symbols in the font in use, mono 14), the search
field 32 tall, then the list (max 300 tall): rows radius 6, the selected one
`alpha(prim, .14)` with a check; Default and System monospace show a note ui 11.5
`txt_dim`; each installed family its name ui 13 `txt_hi` and, under it, a line in its own
face, 12.5 `txt_mut` ("0O Il1 {}[] => != 12 345,67 € éàç": the web's cards' sample), cut
before the check. The faces are mapped and added to the fonts while the dialog is open
(`fonts::set_samples`), dropped when it closes.

## Motion

Short and few: menus / panels pop (scale .96 → 1 + fade, 140 ms), dialogs 160 ms, a
tab's grid fades in 200 ms, chips pop 150 ms, toasts as above, edited cells flash. Fill by
example's ghosts come in as a wave down the rows (the web's `ffin`): each row 16 ms after
the one above, fading in and sliding 8 px from the left over 450 ms, ease-out
(`grid::GHOST_IN`). No
animation on hover colours of the toggles (they switch at once). Respect a
reduced-motion setting by skipping them.

## egui notes

- `theme::apply(ctx, theme, metrics)` once per theme change (it sets both of egui's
  styles); `fonts::install(ctx, pick)` once at startup (≈ 2.5 ms cached).
- Draw the grid with the painter, not widgets: per visible row one `rect_filled` for
  the row fill, then texts, then the 1 px rules. Put a 1 px rule on a pixel centre
  (`painter.round_to_pixel_center(x)`) so it stays crisp; fills on whole pixels.
- Fonts: cells `FontId::monospace(13.0)`; titles
  `FontId::new(12.5, FontFamily::Name(fonts::UI_SEMIBOLD.into()))`; chrome
  `FontId::proportional(…)`. A value's width for column sizing: chars × the advance of
  "0" in the mono font, measured once:
  `ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(13.0), '0'))`.
- Text vertical centring: `Align2::LEFT_CENTER` at the row's centre (y + (row_h − 1) / 2).
- Selection fill, marks and the column's tint are translucent: paint them after the row fill.
