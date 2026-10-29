# Icons

The web app's SVG icons, one file each, drawn by `app/src/icons.rs` (`Icon::*`, kebab-case file = PascalCase variant). Taken verbatim from the sources below, except: the CSS a class gave them (`svg.ic` app.css:1367, `.tgl svg.tg` app.css:657, the toast/menu wrappers of the JS icon maps) is written as attributes; `currentColor` and the CSS-mask colours are `#fff`, so a tint gives the colour; `xmlns`, `width` and `height` added; `class`/`style` dropped. Sizes and colours: `docs/look.md`.

Not icons, left out: the welcome background noise (app.css:1096) and the sparkline images (47-sparklines.js:346, 365), built from data.

| name | from | used for |
|---|---|---|
| `accent-e` | viewer.htm:611 (the text “e”, Inter SemiBold 17.5 at x 9 middle, y 16.2, as a path) | search toggle “ignore accents”: the letter, 18 px; paint `accent-mark` over it |
| `accent-mark` | viewer.htm:611 (path.acc, stroke 1.9) | search toggle “ignore accents”: the accent, `danger_txt` when on, `tgl_acc_off` when off |
| `anonymise` | viewer.htm:504 | menu: Anonymise… |
| `arrow-up` | ui/app.css:620 | header sort arrow, 12 px `prim`; descending = rotated 180° — native: `arrow()` |
| `bars` | ui/app.css:935 | charts-row gutter 15 px `txt_faint`; status bar charts toggle 16 px `cyan_txt` — native: `bars()` |
| `bolt` | viewer.htm:655 | find & replace: Apply, 14 px |
| `calendar` | ui/app.css:913 | header type badge “date”, 11 px `violet_txt` — native: `calendar()` |
| `card` | viewer.htm:456<br>ui/js/10-welcome.js:129 (ROW_ICONS.card) | menu: Row › Card; row ⋮ menu: Card |
| `chart` | viewer.htm:540 | menu: Show the column charts |
| `check` | viewer.htm:473 | menu: Schema › Validate |
| `checkbox` | ui/app.css:678 | checked checkbox, white on `prim` |
| `chevron-down` | ui/app.css:907 | header ▾ column menu, 13 px — native: `chevron_down()` |
| `chevron-down-bold` | ui/js/31-duplicates.js:475 | duplicates cell: next group, 14 px |
| `chevron-down-wide` | viewer.htm:635 | toolbar: Save options ▾, drawn 16 × 12 (so 12 px) |
| `chevron-left` | ui/js/11-column-panel.js:101 | column panel: Move left, 14 px |
| `chevron-right` | ui/js/11-column-panel.js:102 | column panel: Move right, 14 px |
| `chevron-up-bold` | ui/js/31-duplicates.js:475 | duplicates cell: previous group, 14 px |
| `clean` | viewer.htm:498 | menu: Clean up… |
| `code` | ui/js/10-welcome.js:130 (ROW_ICONS.raw) | row ⋮ menu: Raw line |
| `color-scale` | ui/js/11-column-panel.js:125 | column panel: Colour scale |
| `columns` | viewer.htm:518 | menu: Columns › |
| `combine-files` | viewer.htm:511 | menu: Combine files… |
| `command` | viewer.htm:446 | menu: Command palette… |
| `compare` | viewer.htm:584 | menu: Compare with another tab… |
| `convert` | viewer.htm:533 | menu: Convert formats… |
| `convert-column` | ui/js/11-column-panel.js:108 | column panel: Convert |
| `copy` | ui/js/10-welcome.js:127 (ROW_ICONS.dup)<br>ui/js/53-context-menu.js:20 (CELL_ICONS.copy) | row ⋮ menu: Duplicate; cell menu: Copy |
| `cross` | ui/app.css:178 | filter chip ×, 12 px in an 18 px circle — native: `cross()` |
| `cut` | ui/js/53-context-menu.js:29 (CELL_ICONS.cut) | cell menu: Cut |
| `dashed-square` | ui/js/53-context-menu.js:30 (CELL_ICONS.all)<br>ui/js/49-insights.js:40 (IN_ICONS.empty) | cell menu: Select all; insights: empty |
| `data-bars` | ui/js/11-column-panel.js:124 | column panel: Data bars |
| `discard` | viewer.htm:620 | toolbar: Discard the edits, 16 px |
| `dup-delete` | ui/js/31-duplicates.js:398 | duplicates bar: Delete, 13 px |
| `dup-merge` | ui/js/31-duplicates.js:399 | duplicates bar: Merge, 13 px |
| `duplicates` | viewer.htm:571 | menu: Duplicates… |
| `edit` | viewer.htm:557 | menu: Edit filtered rows… |
| `extract` | viewer.htm:625 | toolbar: Extract, 16 px (a mask cuts the file under the arrow: its #000/#fff are the mask, not colours) |
| `eye` | viewer.htm:549 | menu: Show or hide columns… |
| `eye-off` | viewer.htm:574 | menu: Delete hidden rows |
| `file` | viewer.htm:495 | menu: Transform the file › |
| `file-diff` | viewer.htm:487 | menu: Review changes… |
| `file-plus` | viewer.htm:472 | menu: Schema › Create… |
| `file-text` | ui/js/10-welcome.js:113 | welcome: a recent file, 18 px `ok` |
| `fill-blanks` | viewer.htm:564 | menu: Fill empty cells from above |
| `fill-down` | viewer.htm:561 | menu: Fill series down |
| `find` | viewer.htm:491 | menu: Find & replace |
| `font-check` | ui/app.css:751 | font picker card: the chosen one, 12 px white on `prim` |
| `formula` | viewer.htm:527 | menu: Computed column… |
| `freeze` | viewer.htm:537 | menu: Freeze the first column |
| `freeze-column` | ui/js/53-context-menu.js:33 (CELL_ICONS.freeze) | header menu: Freeze up to here |
| `funnel` | viewer.htm:546 | menu: Show the filter row |
| `funnel-clear` | ui/js/53-context-menu.js:31 (CELL_ICONS.clearf) | cell menu: Clear the filters |
| `funnel-filled` | ui/app.css:910 | header: column filtered (13 px `prim`); filter-row gutter (13 px `txt_faint`); status bar filter toggle (15 px `accent_2`) — native: `funnel(.., true, ..)` |
| `funnel-x` | ui/app.css:181 | filter chips: Remove every filter, 16 px `txt_mut` |
| `fx` | ui/app.css:1307 | the ƒx button left of the search (22 × 19), Functions buttons (20 × 17), fx picker (25 × 22) — native: `fx()` |
| `goto` | viewer.htm:455 | menu: Row › Go to… |
| `group-by` | viewer.htm:578 | menu: Group by… |
| `hamburger` | viewer.htm:442 | the ☰ menu button, 18 px — native: `hamburger()` |
| `heat` | ui/js/53-context-menu.js:32 (CELL_ICONS.heat) | header menu: Colour scale |
| `help` | viewer.htm:590 | menu: Help label |
| `hide` | ui/js/53-context-menu.js:34 (CELL_ICONS.hide) | header menu: Hide the column |
| `history` | viewer.htm:480 | menu: Edit label |
| `in-dep` | ui/js/49-insights.js:35 (IN_ICONS.dep) | insights: a dependency |
| `in-dup` | ui/js/49-insights.js:38 (IN_ICONS.dup) | insights: duplicates |
| `in-far` | ui/js/49-insights.js:37 (IN_ICONS.far) | insights: outliers |
| `in-key` | ui/js/49-insights.js:34 (IN_ICONS.key) | insights: a key column |
| `in-none` | ui/js/49-insights.js:126 | insights: nothing stands out, 18 px `ok`; same pixels as check (a path for the polyline) |
| `in-rel` | ui/js/49-insights.js:39 (IN_ICONS.rel) | insights: a relation |
| `in-shape` | ui/js/49-insights.js:36 (IN_ICONS.shape) | insights: a shape |
| `in-space` | ui/js/49-insights.js:41 (IN_ICONS.space) | insights: spaces |
| `info` | viewer.htm:593 | menu: Help › About… |
| `insert-after` | ui/js/53-context-menu.js:27 (CELL_ICONS.after) | header menu: Insert column after |
| `insert-before` | ui/js/53-context-menu.js:26 (CELL_ICONS.before) | header menu: Insert column before |
| `kebab` | ui/app.css:889 (.row-btn: radial-gradient dots 1.3 → 1.6 px, 4 × 4.5 px tiles, 4.25 px padding) | row ⋮ button, 18 × 22, the dots 4 × 13.5 |
| `keep` | ui/js/53-context-menu.js:23 (CELL_ICONS.keep) | cell menu: Keep only this value |
| `keyboard` | viewer.htm:592 | menu: Help › Shortcuts |
| `link` | viewer.htm:530 | menu: Look up from another tab… |
| `logo` | icons/csvfab.svg | welcome hero, the app logo — in colour: paint it with `Color32::WHITE` |
| `magnifier` | viewer.htm:414<br>viewer.htm:463 (as .ic) | command palette input, 18 px `txt_dim` — native: `magnifier()` (chrome.rs palette); menu: Look label |
| `magnifier-filter` | ui/app.css:871 | filter row input, 11 px #6e7681 — native: `magnifier()` with w ≥ 2.25 (grid.rs) |
| `map` | viewer.htm:465 | menu: Look › Map |
| `menu-check` | ui/js/05-header-status.js:142 | dropdown item ticked (✓), 14 px |
| `merge` | ui/js/11-column-panel.js:106<br>ui/js/53-context-menu.js:25 (CELL_ICONS.merge) | column panel: Merge; cell menu: merge |
| `merge-columns` | viewer.htm:524 | menu: Merge columns… |
| `merge-drop` | ui/app.css:709 | duplicates merge: a row dropped, 16 px `diff_txt` |
| `merge-keep` | ui/app.css:707 | duplicates merge: the row kept (row number cell), 16 px `txt_strong` |
| `noise` | viewer.htm:501 | menu: Remove noise words… |
| `not-equal` | viewer.htm:613 | search toggle “invert”, 18 px — native: `not_equal()` |
| `open` | viewer.htm:450 | menu: Open |
| `paste` | ui/js/53-context-menu.js:21 (CELL_ICONS.paste) | cell menu: Paste |
| `pivot` | viewer.htm:581 | menu: Pivot table… |
| `plus` | viewer.htm:425 (#tab-add: the text “+”; drawn in the set’s 24-unit stroke-2 style) | tab bar: open a file — native: `plus()` |
| `profile` | viewer.htm:460 | menu: File profile… |
| `quit` | viewer.htm:597 | menu: Quit (`danger_txt`) |
| `radio` | ui/app.css:679 | checked radio dot, white on `prim` |
| `redo` | viewer.htm:483 | menu: Edit › Redo |
| `regex` | viewer.htm:612 | search toggle “regex”, 18 px stroke 1.35 — native: `regex()` |
| `revert` | ui/js/53-context-menu.js:28 (CELL_ICONS.undo) | cell menu: Revert the cell |
| `row` | viewer.htm:453 | menu: Row label |
| `row-above` | ui/js/10-welcome.js:125 (ROW_ICONS.above) | row ⋮ menu: Insert above |
| `row-below` | ui/js/10-welcome.js:126 (ROW_ICONS.below) | row ⋮ menu: Insert below |
| `rows` | viewer.htm:554 | menu: Rows › |
| `save` | viewer.htm:630 | toolbar: Save, 16 px — native: `save()` |
| `schema` | viewer.htm:470 | menu: Schema label |
| `select-chevron` | ui/app.css:186 | the <select> arrow, 16 × 12 (so 12 px) #8b949e; same pixels as chevron-down-wide |
| `sigma` | ui/app.css:1011 | status bar totals toggle, 15 px `orange_txt` — native: `sigma()` |
| `sliders` | ui/js/05-header-status.js:311 | gutter of the header row: Manage columns, 16 px `txt` — native: `sliders()` |
| `sparkle` | viewer.htm:466<br>viewer.htm:567<br>ui/js/53-context-menu.js:22 (CELL_ICONS.magic) | menu: Look › Insights, Rows › Fill by example; cell menu: magic |
| `sparkles` | ui/js/48-fill-example.js:351<br>ui/js/49-insights.js:47 | fill by example; insights dialog title |
| `split` | ui/js/11-column-panel.js:105<br>ui/js/53-context-menu.js:24 (CELL_ICONS.split) | column panel: Split; cell menu: split |
| `split-column` | viewer.htm:521 | menu: Split column… |
| `split-files` | viewer.htm:508 | menu: Split into files… |
| `toast-danger` | ui/js/15-ui-undo-dialogs-filters.js:12 (TOAST_ICONS.danger) | toast “danger”, 16 px in the kind’s colour |
| `toast-info` | ui/js/15-ui-undo-dialogs-filters.js:14 (TOAST_ICONS.info) | toast “info”, 16 px in the kind’s colour |
| `toast-ok` | ui/js/15-ui-undo-dialogs-filters.js:11 (TOAST_ICONS.ok) | toast “ok”, 16 px in the kind’s colour — native: `check()` |
| `toast-warn` | ui/js/15-ui-undo-dialogs-filters.js:13 (TOAST_ICONS.warn) | toast “warn”, 16 px in the kind’s colour |
| `totals` | viewer.htm:543 | menu: Show the totals row |
| `trash` | ui/js/10-welcome.js:128 (ROW_ICONS.del) | row ⋮ menu: Delete |
| `undo` | viewer.htm:482 | menu: Edit › Undo |
| `upload` | ui/js/10-welcome.js:104 | welcome drop zone, 23 px `on_prim` in a 50 px `prim` circle (native draws `arrow()` there: use Icon::Upload) |
| `warning` | ui/js/49-insights.js:42 (IN_ICONS.warn) | insights: warning |
