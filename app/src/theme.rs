//! The look of the web app, as data: every palette of `ui/themes.css`, the
//! sizes of `ui/app.css`, and how standard egui widgets are dressed in them.
//!
//! One `Palette` field per CSS variable (`--txt-hi` → `txt_hi`), resolved:
//! `var()` and `color-mix()` are computed here, so the app never mixes at
//! draw time. CSS `rgba(var(--x-rgb), a)` is `alpha(p.x, a)`; where the CSS
//! triple equals the plain colour (`--bg-rgb`, `--bg-deep-rgb`, `--prim-rgb`,
//! `--danger-rgb`, `--orange-rgb`) there is no second field. The triples
//! without a plain twin, or that differ from it, are fields of their own:
//! `accent` (`--accent-rgb`), `warn_rgb`, `ok_rgb`, `violet`, `cyan`, `pink`,
//! `gray`, `gray_hi`, `hi`. docs/look.md says where each one is used.

use eframe::egui;
use egui::epaint::{CornerRadius, Margin, Shadow, Stroke};
use egui::style::{ScrollStyle, WidgetVisuals};
use egui::{Color32, FontFamily, FontId, TextStyle, vec2};
use std::sync::LazyLock;

/// `c` at opacity `a` (CSS `rgba(r, g, b, a)`, or `color-mix(c a, transparent)`).
pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a * 255.0).round() as u8)
}

/// CSS `color-mix(in srgb, a p, b)`: `p` of `a`, the rest of `b`, both opaque.
pub fn mix(a: Color32, b: Color32, p: f32) -> Color32 {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let m = |i: usize| (a[i] as f32 * p + b[i] as f32 * (1.0 - p)).round() as u8;
    Color32::from_rgb(m(0), m(1), m(2))
}

const fn h(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Palette {
    // Backgrounds, darkest to lightest on the dark themes.
    pub bg: Color32,
    pub bg_deep: Color32,
    pub surf: Color32,
    pub surf_2: Color32,
    pub surf_3: Color32,
    pub bar_2: Color32,
    pub filter_bg: Color32,
    /// A button joined to a field (the ƒx before the search box).
    pub addon_bg: Color32,
    pub brd: Color32,
    pub brd_2: Color32,
    pub scroll: Color32,
    // Text, strongest to faintest: strong > hi > txt > mut > sub > dim > faint.
    pub txt: Color32,
    pub txt_hi: Color32,
    pub txt_strong: Color32,
    pub txt_mut: Color32,
    pub txt_sub: Color32,
    pub txt_dim: Color32,
    pub txt_faint: Color32,
    pub tip_bg: Color32,
    pub tip_fg: Color32,
    pub tip_brd: Color32,
    pub prim: Color32,
    pub prim_hover: Color32,
    pub on_prim: Color32,
    /// `--accent-rgb`: the tints of selection, hover, active row.
    pub accent: Color32,
    pub accent_2: Color32,
    pub warn: Color32,
    pub warn_hover: Color32,
    /// `--warn-rgb`: not always `warn` (GitHub Light's is lighter).
    pub warn_rgb: Color32,
    pub on_warn: Color32,
    pub mark_txt: Color32,
    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_txt: Color32,
    pub on_danger: Color32,
    pub ok: Color32,
    /// `--ok-rgb`: darker than `ok` on GitHub Dark.
    pub ok_rgb: Color32,
    pub ok_txt: Color32,
    pub ok_subtle: Color32,
    pub ok_border_subtle: Color32,
    pub ok_emph: Color32,
    pub danger_subtle: Color32,
    pub danger_border_subtle: Color32,
    pub danger_emph: Color32,
    pub diff_txt: Color32,
    pub orange: Color32,
    pub orange_txt: Color32,
    pub violet: Color32,
    pub violet_txt: Color32,
    pub cyan: Color32,
    pub cyan_txt: Color32,
    pub pink: Color32,
    pub pink_txt: Color32,
    pub gray: Color32,
    pub gray_hi: Color32,
    /// The overlay colour (white on dark themes, near black on light ones).
    pub hi: Color32,
    pub menu_bg_hover: Color32,
    /// The status bar: a vertical gradient from `status_a` to `status_b`.
    pub status_a: Color32,
    pub status_b: Color32,
    pub status_brd: Color32,
    pub status_sep: Color32,
    pub status_fg: Color32,
    pub stat_filt: Color32,
    pub stat_total: Color32,
    pub idx_hover: Color32,
    /// Rows of an odd record id (`--row-bg`, `bg` where the theme has none).
    pub row_bg: Color32,
    /// Rows of an even record id.
    pub row_alt: Color32,
    /// Unused by the grid since the accent hover (kept: the CSS still defines it).
    pub row_hover: Color32,
    // Syntax colours of the raw text mode.
    pub sy_kw: Color32,
    pub sy_ctl: Color32,
    pub sy_str: Color32,
    pub sy_num: Color32,
    pub sy_com: Color32,
    pub sy_fn: Color32,
    pub sy_type: Color32,
    pub sy_var: Color32,
    pub sy_prop: Color32,
    pub sy_re: Color32,
    pub sy_tag: Color32,
    pub sy_attr: Color32,
    pub sy_esc: Color32,
    pub sy_const: Color32,
    pub sy_err: Color32,
    pub sy_wrn: Color32,
    pub sy_inf: Color32,
    pub sy_dbg: Color32,

    // ---- Derived (computed by `derived()`): the mixes app.css makes ----
    /// Hovered row, opaque: 12 % accent into `row_bg` / `row_alt`.
    pub row_hover_bg: Color32,
    pub row_hover_alt: Color32,
    /// The active cell's row, opaque: 16 % accent.
    pub row_cur_bg: Color32,
    pub row_cur_alt: Color32,
    /// Selected cells, translucent (over the row colour): accent at .18.
    pub sel_fill: Color32,
    /// Selected cells of a frozen column, opaque: 18 % accent into the row colour.
    pub sel_frz_bg: Color32,
    pub sel_frz_alt: Color32,
    /// The active cell's 2 px inset frame, the fill handle: `prim`.
    pub cursor: Color32,
    /// Fill-drag preview: accent .08, its 1 px frame accent .6.
    pub fill_preview: Color32,
    pub fill_preview_brd: Color32,
    /// Marked cells (review, compare): warn .16; a cell just edited flashes at .38.
    pub mark_fill: Color32,
    pub flash_fill: Color32,
    /// Highlight of a filter's match in a cell (`mark`): warn .25 under `mark_txt`; a find's: ok .3.
    pub match_bg: Color32,
    pub find_bg: Color32,
    /// The rule after the last frozen column, the line between groups of a sorted view.
    pub frz_rule: Color32,
    pub group_line: Color32,
    /// Numbers and dates in the grid (type colours on).
    pub num_txt: Color32,
    pub date_txt: Color32,
    /// Category pills, in the column chart's ribbon order; `pill_other` for the rest.
    pub pill: [Color32; 5],
    pub pill_other: Color32,
    /// The column chart band: a vertical gradient `surf` → `spk_bottom`.
    pub spk_bottom: Color32,
    /// Chart gauge: filled, empty; ribbon: other values, empty cells.
    pub gauge_fill: Color32,
    pub gauge_empty: Color32,
    pub ribbon_other: Color32,
    pub ribbon_empty: Color32,
    /// Scroll strip thumb: fill and 1 px frame, then on hover / drag.
    pub thumb: Color32,
    pub thumb_brd: Color32,
    pub thumb_hot: Color32,
    pub thumb_hot_brd: Color32,
    /// Focus ring around a field (4 px outside its border), and the lighter 3 px one of small fields.
    pub focus_ring: Color32,
    pub focus_ring_sm: Color32,
    /// Filter chips: prim .12 fill, .4 border.
    pub chip_bg: Color32,
    pub chip_brd: Color32,
    /// Tooltip box: `tip_bg` at .72, frame `tip_brd` at .7.
    pub tip_fill: Color32,
    pub tip_stroke: Color32,
    /// The accent of the "ignore accents" toggle while off.
    pub tgl_acc_off: Color32,
    /// Dim layer behind dialogs: `bg_deep` at .45.
    pub backdrop: Color32,
}

impl Palette {
    /// The colours app.css mixes from the base ones.
    fn derived(mut self) -> Self {
        let a = self.accent;
        self.row_hover_bg = mix(a, self.row_bg, 0.12);
        self.row_hover_alt = mix(a, self.row_alt, 0.12);
        self.row_cur_bg = mix(a, self.row_bg, 0.16);
        self.row_cur_alt = mix(a, self.row_alt, 0.16);
        self.sel_fill = alpha(a, 0.18);
        self.sel_frz_bg = mix(a, self.row_bg, 0.18);
        self.sel_frz_alt = mix(a, self.row_alt, 0.18);
        self.cursor = self.prim;
        self.fill_preview = alpha(a, 0.08);
        self.fill_preview_brd = alpha(a, 0.6);
        self.mark_fill = alpha(self.warn_rgb, 0.16);
        self.flash_fill = alpha(self.warn_rgb, 0.38);
        self.match_bg = alpha(self.warn_rgb, 0.25);
        self.find_bg = alpha(self.ok_rgb, 0.3);
        self.frz_rule = alpha(a, 0.6);
        self.group_line = alpha(a, 0.55);
        self.num_txt = self.cyan_txt;
        self.date_txt = self.violet_txt;
        self.pill = [self.accent_2, self.pink_txt, self.orange_txt, self.ok_txt, self.warn];
        self.pill_other = self.gray_hi;
        self.spk_bottom = mix(self.surf, self.bg, 0.55);
        self.gauge_fill = alpha(self.ok_rgb, 0.45);
        self.gauge_empty = alpha(self.gray, 0.22);
        self.ribbon_other = alpha(self.gray, 0.42);
        self.ribbon_empty = alpha(self.gray, 0.16);
        self.thumb = alpha(self.gray, 0.28);
        self.thumb_brd = alpha(self.hi, 0.18);
        self.thumb_hot = alpha(a, 0.35);
        self.thumb_hot_brd = alpha(a, 0.7);
        self.focus_ring = alpha(self.prim, 0.25);
        self.focus_ring_sm = alpha(self.prim, 0.22);
        self.chip_bg = alpha(self.prim, 0.12);
        self.chip_brd = alpha(self.prim, 0.4);
        self.tip_fill = alpha(self.tip_bg, 0.72);
        self.tip_stroke = alpha(self.tip_brd, 0.7);
        self.tgl_acc_off = mix(self.danger_txt, self.txt_dim, 0.45);
        self.backdrop = alpha(self.bg_deep, 0.45);
        self
    }

    /// A row's background: `even` is the record id's parity (not the view's), as the web app stripes.
    pub fn row_fill(&self, even: bool, hovered: bool, current: bool) -> Color32 {
        match (current, hovered, even) {
            (true, _, true) => self.row_cur_alt,
            (true, _, false) => self.row_cur_bg,
            (false, true, true) => self.row_hover_alt,
            (false, true, false) => self.row_hover_bg,
            (false, false, true) => self.row_alt,
            (false, false, false) => self.row_bg,
        }
    }

    /// A pill's text colour and fill (the fill is its colour at .17).
    pub fn pill_colors(&self, slot: Option<usize>) -> (Color32, Color32) {
        let c = slot.and_then(|i| self.pill.get(i).copied()).unwrap_or(self.pill_other);
        (c, alpha(c, 0.17))
    }
}

pub struct Theme {
    /// The web app's id (its localStorage `csvfab-theme`): dark, tokyonight, catppuccin, vscode, light, latte.
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub p: Palette,
    /// The status bar's swatch: two halves on a 135° diagonal, `bg` then this.
    pub swatch: Color32,
}

pub const DEFAULT_THEME: &str = "dark";

/// How much the grid draws beyond the values (the status bar's colour wheel), saved in the
/// config: `ALL`; `NO_TYPES`, numbers and dates in the text's colour; `RAW`, the values as plain
/// text — no type colours, pills, checks, swatches, links, colour scales nor data bars.
/// Global: it only changes how cells are drawn.
pub const ALL: u8 = 2;
pub const NO_TYPES: u8 = 1;
pub const RAW: u8 = 0;
static LOOK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(ALL);

pub fn look() -> u8 {
    LOOK.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_look(l: u8) {
    LOOK.store(l.min(ALL), std::sync::atomic::Ordering::Relaxed);
}

pub fn type_colors() -> bool {
    look() == ALL
}

pub fn raw() -> bool {
    look() == RAW
}

/// Every theme, in the web app's menu order.
pub fn themes() -> &'static [Theme] {
    static THEMES: LazyLock<Vec<Theme>> = LazyLock::new(|| {
        let [dark, light, tokyonight, catppuccin, vscode, latte] = palettes();
        let t = |id, name, dark, p, swatch| Theme { id, name, dark, p, swatch: h(swatch) };
        vec![
            t("dark", "GitHub Dark", true, dark, 0x238636),
            t("tokyonight", "Tokyo Night", true, tokyonight, 0x7aa2f7),
            t("catppuccin", "Catppuccin Mocha", true, catppuccin, 0xcba6f7),
            t("vscode", "VS Code", true, vscode, 0x007acc),
            t("light", "GitHub Light", false, light, 0x0969da),
            t("latte", "Catppuccin Latte", false, latte, 0x8839ef),
        ]
    });
    &THEMES
}

/// The theme of that id, GitHub Dark for an unknown one.
pub fn by_id(id: &str) -> &'static Theme {
    let all = themes();
    all.iter().find(|t| t.id == id).unwrap_or(&all[0])
}

/// Generated from ui/themes.css (each block over the :root one, `--addon-bg` mixed, `--row-bg` defaulted).
fn palettes() -> [Palette; 6] {
    [
        // dark
        Palette {
            bg: h(0x0d1117), bg_deep: h(0x010409), surf: h(0x161b22), surf_2: h(0x21262d),
            surf_3: h(0x30363d), bar_2: h(0x1c2128), filter_bg: h(0x12171e), brd: h(0x30363d),
            brd_2: h(0x484f58), scroll: h(0x3d444d), txt: h(0xc9d1d9), txt_hi: h(0xe6edf3),
            txt_strong: h(0xffffff), txt_mut: h(0x8b949e), txt_sub: h(0x7d8590), txt_dim: h(0x6e7681),
            txt_faint: h(0x545d68), tip_bg: h(0x0b0e13), tip_fg: h(0xe6edf3), tip_brd: h(0x3d444d),
            prim: h(0x0d6efd), prim_hover: h(0x0b5ed7), on_prim: h(0xffffff), accent_2: h(0x79c0ff),
            warn: h(0xe3b341), warn_hover: h(0xd2a43a), on_warn: h(0x000000), mark_txt: h(0xffdf70),
            danger: h(0xf85149), danger_hover: h(0xda3633), danger_txt: h(0xff7b72), on_danger: h(0xffffff),
            ok: h(0x3fb950), ok_txt: h(0x7ee2a8), ok_subtle: h(0x051b11), ok_border_subtle: h(0x0f5132),
            ok_emph: h(0x75b798), danger_subtle: h(0x2c0b0e), danger_border_subtle: h(0x842029), danger_emph: h(0xea868f),
            diff_txt: h(0xea868f), orange: h(0xf0883e), orange_txt: h(0xffa657), violet_txt: h(0xd2a8ff),
            cyan_txt: h(0x56d4dd), pink_txt: h(0xff9bce), menu_bg_hover: h(0x2ea043),
            status_a: h(0x0b0f15), status_b: h(0x06090d), status_brd: h(0x30363d),
            status_sep: h(0x30363d), status_fg: h(0x8b949e), stat_filt: h(0xf0883e), stat_total: h(0x3fb950),
            idx_hover: h(0x2d333b), row_alt: h(0x121820), row_hover: h(0x1f242c), sy_kw: h(0xff7b72),
            sy_ctl: h(0xff7b72), sy_str: h(0xa5d6ff), sy_num: h(0x79c0ff), sy_com: h(0x8b949e),
            sy_fn: h(0xd2a8ff), sy_type: h(0xffa657), sy_var: h(0xffa657), sy_prop: h(0x79c0ff),
            sy_re: h(0x7ee787), sy_tag: h(0x7ee787), sy_attr: h(0x79c0ff), sy_esc: h(0x79c0ff),
            sy_const: h(0x79c0ff), sy_err: h(0xf85149), sy_wrn: h(0xd29922), sy_inf: h(0x58a6ff),
            sy_dbg: h(0x6e7681), accent: h(0x388bfd), warn_rgb: h(0xe3b341), ok_rgb: h(0x2ea043),
            violet: h(0xa371f7), cyan: h(0x39c5cf), pink: h(0xf778ba), gray: h(0x6e7681),
            gray_hi: h(0xadbac7), hi: h(0xffffff), row_bg: h(0x0d1117), addon_bg: h(0x0a0e14),
            ..Default::default()
        }
        .derived(),
        // light
        Palette {
            bg: h(0xffffff), bg_deep: h(0xf6f8fa), surf: h(0xf6f8fa), surf_2: h(0xeaeef2),
            surf_3: h(0xd0d7de), bar_2: h(0xf6f8fa), filter_bg: h(0xf6f8fa), brd: h(0xd0d7de),
            brd_2: h(0xafb8c1), scroll: h(0xc1c8cf), txt: h(0x1f2328), txt_hi: h(0x1f2328),
            txt_strong: h(0x000000), txt_mut: h(0x59636e), txt_sub: h(0x6e7781), txt_dim: h(0x6e7781),
            txt_faint: h(0x8c959f), tip_bg: h(0x1f2328), tip_fg: h(0xf6f8fa), tip_brd: h(0x1f2328),
            prim: h(0x0969da), prim_hover: h(0x0860ca), on_prim: h(0xffffff), accent_2: h(0x0969da),
            warn: h(0xbf8700), warn_hover: h(0xa37200), on_warn: h(0xffffff), mark_txt: h(0x7d4e00),
            danger: h(0xcf222e), danger_hover: h(0xa40e26), danger_txt: h(0xcf222e), on_danger: h(0xffffff),
            ok: h(0x1a7f37), ok_txt: h(0x1a7f37), ok_subtle: h(0xd1e7dd), ok_border_subtle: h(0xa3cfbb),
            ok_emph: h(0x0a3622), danger_subtle: h(0xf8d7da), danger_border_subtle: h(0xf1aeb5), danger_emph: h(0x58151c),
            diff_txt: h(0xb02a37), orange: h(0xbc4c00), orange_txt: h(0xbc4c00), violet_txt: h(0x8250df),
            cyan_txt: h(0x1b7c83), pink_txt: h(0xbf3989), menu_bg_hover: h(0x1a7f37),
            status_a: h(0xf6f8fa), status_b: h(0xf6f8fa), status_brd: h(0xd0d7de),
            status_sep: h(0xd0d7de), status_fg: h(0x59636e), stat_filt: h(0xbc4c00), stat_total: h(0x1a7f37),
            idx_hover: h(0xdde3ea), row_alt: h(0xf6f8fa), row_hover: h(0xeaf1fb), sy_kw: h(0xcf222e),
            sy_ctl: h(0xcf222e), sy_str: h(0x0a3069), sy_num: h(0x0550ae), sy_com: h(0x6e7781),
            sy_fn: h(0x8250df), sy_type: h(0x953800), sy_var: h(0x953800), sy_prop: h(0x0550ae),
            sy_re: h(0x116329), sy_tag: h(0x116329), sy_attr: h(0x0550ae), sy_esc: h(0x0550ae),
            sy_const: h(0x0550ae), sy_err: h(0xcf222e), sy_wrn: h(0x9a6700), sy_inf: h(0x0969da),
            sy_dbg: h(0x6e7781), accent: h(0x0969da), warn_rgb: h(0xd4a72c), ok_rgb: h(0x1a7f37),
            violet: h(0x8250df), cyan: h(0x1b7c83), pink: h(0xbf3989), gray: h(0x6e7781),
            gray_hi: h(0x57606a), hi: h(0x1f2328), row_bg: h(0xffffff), addon_bg: h(0xeaeef2),
            ..Default::default()
        }
        .derived(),
        // tokyonight
        Palette {
            bg: h(0x15161e), bg_deep: h(0x101014), surf: h(0x1a1b26), surf_2: h(0x232433),
            surf_3: h(0x2f334d), bar_2: h(0x1a1b26), filter_bg: h(0x171824), brd: h(0x232433),
            brd_2: h(0x3b4261), scroll: h(0x3b4261), txt: h(0xc0caf5), txt_hi: h(0xc0caf5),
            txt_strong: h(0xe3e8ff), txt_mut: h(0xa9b1d6), txt_sub: h(0x737aa2), txt_dim: h(0x565f89),
            txt_faint: h(0x444b6a), tip_bg: h(0x0f1018), tip_fg: h(0xc0caf5), tip_brd: h(0x3b4261),
            prim: h(0x7aa2f7), prim_hover: h(0x8db0fa), on_prim: h(0x101014), accent_2: h(0x7dcfff),
            warn: h(0xe0af68), warn_hover: h(0xd6a25a), on_warn: h(0x101014), mark_txt: h(0xe0af68),
            danger: h(0xf7768e), danger_hover: h(0xff8ea2), danger_txt: h(0xf7768e), on_danger: h(0x101014),
            ok: h(0x9ece6a), ok_txt: h(0x9ece6a), ok_subtle: h(0x051b11), ok_border_subtle: h(0x0f5132),
            ok_emph: h(0x75b798), danger_subtle: h(0x2c0b0e), danger_border_subtle: h(0x842029), danger_emph: h(0xea868f),
            diff_txt: h(0xea868f), orange: h(0xff9e64), orange_txt: h(0xff9e64), violet_txt: h(0xbb9af7),
            cyan_txt: h(0x7dcfff), pink_txt: h(0xff75b5), menu_bg_hover: h(0x8db0fa),
            status_a: h(0x121218), status_b: h(0x0e0e13), status_brd: h(0x232433),
            status_sep: h(0x2f334d), status_fg: h(0x737aa2), stat_filt: h(0xff9e64), stat_total: h(0x9ece6a),
            idx_hover: h(0x292e42), row_alt: h(0x181a25), row_hover: h(0x232639), sy_kw: h(0x9d7cd8),
            sy_ctl: h(0xbb9af7), sy_str: h(0x9ece6a), sy_num: h(0xff9e64), sy_com: h(0x565f89),
            sy_fn: h(0x7aa2f7), sy_type: h(0x2ac3de), sy_var: h(0xe0af68), sy_prop: h(0x73daca),
            sy_re: h(0xb4f9f8), sy_tag: h(0xf7768e), sy_attr: h(0xbb9af7), sy_esc: h(0x89ddff),
            sy_const: h(0xff9e64), sy_err: h(0xf7768e), sy_wrn: h(0xe0af68), sy_inf: h(0x7aa2f7),
            sy_dbg: h(0x565f89), accent: h(0x7aa2f7), warn_rgb: h(0xe0af68), ok_rgb: h(0x9ece6a),
            violet: h(0xbb9af7), cyan: h(0x7dcfff), pink: h(0xff75b5), gray: h(0x565f89),
            gray_hi: h(0xa9b1d6), hi: h(0xc0caf5), row_bg: h(0x0f1016), addon_bg: h(0x14151c),
            ..Default::default()
        }
        .derived(),
        // catppuccin
        Palette {
            bg: h(0x11111b), bg_deep: h(0x0b0b12), surf: h(0x181825), surf_2: h(0x1e1e2e),
            surf_3: h(0x313244), bar_2: h(0x181825), filter_bg: h(0x151522), brd: h(0x232334),
            brd_2: h(0x45475a), scroll: h(0x45475a), txt: h(0xcdd6f4), txt_hi: h(0xcdd6f4),
            txt_strong: h(0xeef1ff), txt_mut: h(0xa6adc8), txt_sub: h(0x9399b2), txt_dim: h(0x7f849c),
            txt_faint: h(0x585b70), tip_bg: h(0x0e0e16), tip_fg: h(0xcdd6f4), tip_brd: h(0x45475a),
            prim: h(0x89b4fa), prim_hover: h(0xa0c3fb), on_prim: h(0x11111b), accent_2: h(0x89dceb),
            warn: h(0xf9e2af), warn_hover: h(0xf5d78f), on_warn: h(0x11111b), mark_txt: h(0xf9e2af),
            danger: h(0xf38ba8), danger_hover: h(0xf5a0b8), danger_txt: h(0xf38ba8), on_danger: h(0x11111b),
            ok: h(0xa6e3a1), ok_txt: h(0xa6e3a1), ok_subtle: h(0x051b11), ok_border_subtle: h(0x0f5132),
            ok_emph: h(0x75b798), danger_subtle: h(0x2c0b0e), danger_border_subtle: h(0x842029), danger_emph: h(0xea868f),
            diff_txt: h(0xea868f), orange: h(0xfab387), orange_txt: h(0xfab387), violet_txt: h(0xcba6f7),
            cyan_txt: h(0x94e2d5), pink_txt: h(0xf5c2e7), menu_bg_hover: h(0xd6bbf9),
            status_a: h(0x0e0e17), status_b: h(0x0b0b12), status_brd: h(0x232334),
            status_sep: h(0x313244), status_fg: h(0x9399b2), stat_filt: h(0xfab387), stat_total: h(0xa6e3a1),
            idx_hover: h(0x2a2b3c), row_alt: h(0x141421), row_hover: h(0x1f1f30), sy_kw: h(0xcba6f7),
            sy_ctl: h(0xcba6f7), sy_str: h(0xa6e3a1), sy_num: h(0xfab387), sy_com: h(0x9399b2),
            sy_fn: h(0x89b4fa), sy_type: h(0xf9e2af), sy_var: h(0xf2cdcd), sy_prop: h(0xb4befe),
            sy_re: h(0xf5c2e7), sy_tag: h(0x89b4fa), sy_attr: h(0xf9e2af), sy_esc: h(0xf5c2e7),
            sy_const: h(0xfab387), sy_err: h(0xf38ba8), sy_wrn: h(0xf9e2af), sy_inf: h(0x89b4fa),
            sy_dbg: h(0x7f849c), accent: h(0x89b4fa), warn_rgb: h(0xf9e2af), ok_rgb: h(0xa6e3a1),
            violet: h(0xcba6f7), cyan: h(0x94e2d5), pink: h(0xf5c2e7), gray: h(0x6c7086),
            gray_hi: h(0xa6adc8), hi: h(0xcdd6f4), row_bg: h(0x11111b), addon_bg: h(0x11111b),
            ..Default::default()
        }
        .derived(),
        // vscode
        Palette {
            bg: h(0x1a1b26), bg_deep: h(0x16161e), surf: h(0x16161e), surf_2: h(0x1e202e),
            surf_3: h(0x292e42), bar_2: h(0x16161e), filter_bg: h(0x14141b), brd: h(0x202330),
            brd_2: h(0x363b54), scroll: h(0x363b54), txt: h(0xa9b1d6), txt_hi: h(0xc0caf5),
            txt_strong: h(0xdfe3f5), txt_mut: h(0x787c99), txt_sub: h(0x737aa2), txt_dim: h(0x545c7e),
            txt_faint: h(0x3b4261), tip_bg: h(0x0d0d12), tip_fg: h(0xc0caf5), tip_brd: h(0x3b4261),
            prim: h(0x3d59a1), prim_hover: h(0x4a68b8), on_prim: h(0xffffff), accent_2: h(0x7dcfff),
            warn: h(0xe0af68), warn_hover: h(0xd6a25a), on_warn: h(0x16161e), mark_txt: h(0xe0af68),
            danger: h(0xf7768e), danger_hover: h(0xff8ea2), danger_txt: h(0xf7768e), on_danger: h(0x16161e),
            ok: h(0x9ece6a), ok_txt: h(0x9ece6a), ok_subtle: h(0x051b11), ok_border_subtle: h(0x0f5132),
            ok_emph: h(0x75b798), danger_subtle: h(0x2c0b0e), danger_border_subtle: h(0x842029), danger_emph: h(0xea868f),
            diff_txt: h(0xea868f), orange: h(0xff9e64), orange_txt: h(0xff9e64), violet_txt: h(0xbb9af7),
            cyan_txt: h(0x2ac3de), pink_txt: h(0xff75b5), menu_bg_hover: h(0x1a8ad4),
            status_a: h(0x16161e), status_b: h(0x16161e), status_brd: h(0x101014),
            status_sep: h(0x292e42), status_fg: h(0x787c99), stat_filt: h(0xff9e64), stat_total: h(0x9ece6a),
            idx_hover: h(0x1e202e), row_alt: h(0x1c1d29), row_hover: h(0x1e202e), sy_kw: h(0x9d7cd8),
            sy_ctl: h(0xbb9af7), sy_str: h(0x9ece6a), sy_num: h(0xff9e64), sy_com: h(0x565f89),
            sy_fn: h(0x7aa2f7), sy_type: h(0x2ac3de), sy_var: h(0xe0af68), sy_prop: h(0x73daca),
            sy_re: h(0xb4f9f8), sy_tag: h(0xf7768e), sy_attr: h(0xbb9af7), sy_esc: h(0x89ddff),
            sy_const: h(0xff9e64), sy_err: h(0xf7768e), sy_wrn: h(0xe0af68), sy_inf: h(0x7aa2f7),
            sy_dbg: h(0x565f89), accent: h(0x7aa2f7), warn_rgb: h(0xe0af68), ok_rgb: h(0x9ece6a),
            violet: h(0xbb9af7), cyan: h(0x2ac3de), pink: h(0xff75b5), gray: h(0x545c7e),
            gray_hi: h(0xa9b1d6), hi: h(0xc0caf5), row_bg: h(0x1a1b26), addon_bg: h(0x1a1b26),
            ..Default::default()
        }
        .derived(),
        // latte
        Palette {
            bg: h(0xeff1f5), bg_deep: h(0xe6e9ef), surf: h(0xe6e9ef), surf_2: h(0xdce0e8),
            surf_3: h(0xccd0da), bar_2: h(0xe6e9ef), filter_bg: h(0xe9ecf1), brd: h(0xccd0da),
            brd_2: h(0xbcc0cc), scroll: h(0xbcc0cc), txt: h(0x4c4f69), txt_hi: h(0x4c4f69),
            txt_strong: h(0x2b2d3f), txt_mut: h(0x5c5f77), txt_sub: h(0x6c6f85), txt_dim: h(0x7c7f93),
            txt_faint: h(0x9ca0b0), tip_bg: h(0x303446), tip_fg: h(0xeff1f5), tip_brd: h(0x303446),
            prim: h(0x1e66f5), prim_hover: h(0x1756d6), on_prim: h(0xffffff), accent_2: h(0x1e66f5),
            warn: h(0xdf8e1d), warn_hover: h(0xc77d15), on_warn: h(0xffffff), mark_txt: h(0x8a5a0b),
            danger: h(0xd20f39), danger_hover: h(0xb50d31), danger_txt: h(0xd20f39), on_danger: h(0xffffff),
            ok: h(0x40a02b), ok_txt: h(0x2f7d1f), ok_subtle: h(0xd1e7dd), ok_border_subtle: h(0xa3cfbb),
            ok_emph: h(0x0a3622), danger_subtle: h(0xf8d7da), danger_border_subtle: h(0xf1aeb5), danger_emph: h(0x58151c),
            diff_txt: h(0xb02a37), orange: h(0xfe640b), orange_txt: h(0xc9500a), violet_txt: h(0x8839ef),
            cyan_txt: h(0x137a80), pink_txt: h(0xb8489a), menu_bg_hover: h(0x7a2fd6),
            status_a: h(0xe6e9ef), status_b: h(0xdce0e8), status_brd: h(0xccd0da),
            status_sep: h(0xbcc0cc), status_fg: h(0x6c6f85), stat_filt: h(0xc9500a), stat_total: h(0x2f7d1f),
            idx_hover: h(0xccd0da), row_alt: h(0xe9ecf1), row_hover: h(0xdce0e8), sy_kw: h(0x8839ef),
            sy_ctl: h(0x8839ef), sy_str: h(0x40a02b), sy_num: h(0xfe640b), sy_com: h(0x7c7f93),
            sy_fn: h(0x1e66f5), sy_type: h(0xdf8e1d), sy_var: h(0xdd7878), sy_prop: h(0x7287fd),
            sy_re: h(0xea76cb), sy_tag: h(0x1e66f5), sy_attr: h(0xdf8e1d), sy_esc: h(0xea76cb),
            sy_const: h(0xfe640b), sy_err: h(0xd20f39), sy_wrn: h(0xdf8e1d), sy_inf: h(0x1e66f5),
            sy_dbg: h(0x8c8fa1), accent: h(0x1e66f5), warn_rgb: h(0xdf8e1d), ok_rgb: h(0x40a02b),
            violet: h(0x8839ef), cyan: h(0x179299), pink: h(0xea76cb), gray: h(0x7c7f93),
            gray_hi: h(0x5c5f77), hi: h(0x4c4f69), row_bg: h(0xeff1f5), addon_bg: h(0xdce0e8),
            ..Default::default()
        }
        .derived(),
    ]
}

/// Every size the grid and the chrome need, in logical px (CSS px = egui points),
/// read from app.css. Heights include the 1 px rule that closes a band.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    // ---- Grid ----
    /// A data row, its 1 px bottom rule included: the density's (`DENSITIES`), 32 by default.
    pub row_h: f32,
    /// The titles' line: 32 + 1 (its rule only when no chart follows).
    pub header_h: f32,
    /// The column charts under the titles, one block with them (68 tall together): 34 + 1 (hidden: 0).
    pub spk_h: f32,
    /// The filter row: 28 + 1 (hidden: 0).
    pub filter_h: f32,
    /// The totals row: a 1 px top rule (`brd_2`) and the rest, as tall as a data row.
    pub totals_h: f32,
    /// The row-number gutter: 70 + 2 × 10 padding + its 2 px `prim` rule on the right.
    pub gutter_w: f32,
    pub gutter_rule_w: f32,
    /// Horizontal padding of a cell (also the title's), each side.
    pub cell_pad_x: f32,
    /// A column's automatic width: its widest value of the first 80 rows + 21, clamped to these;
    /// fitted on demand (double-click on the resize handle) up to `col_fit_max`.
    pub col_min_w: f32,
    pub col_auto_max: f32,
    pub col_fit_max: f32,
    /// The resize handle at a title's right edge.
    pub resize_w: f32,
    /// The ▾ panel button at a title's right: 20 px square, 7 px from the edge; the title text stops 26 px before the edge.
    pub col_menu: f32,
    pub col_title_pad_r: f32,
    /// Type icon before a title (Aa, #, calendar): 16 px tall, min 16 wide, 7 px gap after.
    pub type_icon: f32,
    /// Filter box of the filter row: 22 px tall, 5 px from the cell's sides, text 24 px in (magnifier at 7).
    pub filter_box_h: f32,
    pub filter_cell_pad_x: f32,
    /// Column chart box: 27 px tall, padding 1 top / 7 bottom / 10 sides.
    pub spk_box_h: f32,
    /// Gauge / ribbon at the box's bottom: 4 px, radius 2; the curve stops 3 px above it.
    pub spk_gauge_h: f32,
    /// Category dot: 7 px across, 6 px before its text.
    pub dot_d: f32,
    /// The scroll strips over the scrollbars (vertical width, horizontal height).
    pub strip_w: f32,
    // ---- Chrome ----
    /// The top band (menu, tabs, search, Save): 46 px including its bottom rule, padding 0 12, gap 8.
    pub band_h: f32,
    pub band_pad_x: f32,
    pub band_gap: f32,
    pub tab_h: f32,
    pub tab_max_w: f32,
    /// The filter chips' bar, shown while a filter is in force: 34 px including its bottom rule.
    pub chips_h: f32,
    /// The column inspector, docked at the right.
    pub inspector_w: f32,
    /// Status bar: 36 px including its top rule, padding 0 12 0 18, gap 14.
    pub status_h: f32,
    /// Buttons (`.btn`): padding 6 × 14 → 30 px tall; fields (`.bs-input`, 14 px text): padding 6 × 12 → 31 px.
    pub btn_h: f32,
    pub btn_pad_x: f32,
    pub input_h: f32,
    pub input_pad_x: f32,
    pub search_w: f32,
    pub search_expr_w: f32,
    /// Filter chip: 26 px with its border, radius 12; status bar pill: 22 px, radius 11.
    pub chip_h: f32,
    pub sb_pill_h: f32,
    /// Native scrollbars elsewhere: 12 px, the thumb 6 px inside a 3 px transparent border.
    pub scrollbar_w: f32,
    // ---- Radii ----
    pub r_btn: u8,
    pub r_input: u8,
    pub r_menu: u8,
    pub r_dialog: u8,
    pub r_tip: u8,
    pub r_tab: u8,
    pub r_menu_item: u8,
    // ---- Fonts (px) ----
    /// The cells, in the monospace font.
    pub font_grid: f32,
    /// The chrome: buttons, menus, dialogs' body.
    pub font_ui: f32,
    /// Titles of the grid (weight 600), tabs.
    pub font_header: f32,
    /// Status bar, filter boxes, chips, field labels.
    pub font_small: f32,
    /// Tooltips, pills, chart labels, menu shortcuts.
    pub font_tip: f32,
    /// Column numbers, tab meta, menu section heads.
    pub font_tiny: f32,
    /// Scroll strip labels (weight 600).
    pub font_micro: f32,
    /// A dialog's message (weight 600).
    pub font_title: f32,
}

/// The rows' densities: (name, row height), the setting's order.
pub const DENSITIES: [(&str, f32); 3] = [("Compact", 28.0), ("Normal", 32.0), ("Airy", 38.0)];
pub const DEFAULT_DENSITY: usize = 1;

impl Default for Metrics {
    fn default() -> Self {
        Self {
            row_h: DENSITIES[DEFAULT_DENSITY].1,
            header_h: 33.0,
            spk_h: 35.0,
            filter_h: 29.0,
            totals_h: DENSITIES[DEFAULT_DENSITY].1,
            gutter_w: 92.0,
            gutter_rule_w: 2.0,
            cell_pad_x: 10.0,
            col_min_w: 60.0,
            col_auto_max: 480.0,
            col_fit_max: 900.0,
            resize_w: 5.0,
            col_menu: 20.0,
            col_title_pad_r: 26.0,
            type_icon: 16.0,
            filter_box_h: 22.0,
            filter_cell_pad_x: 5.0,
            spk_box_h: 27.0,
            spk_gauge_h: 4.0,
            dot_d: 7.0,
            strip_w: 24.0,
            band_h: 52.0,
            band_pad_x: 14.0,
            band_gap: 10.0,
            tab_h: 32.0,
            tab_max_w: 250.0,
            chips_h: 34.0,
            inspector_w: 320.0,
            status_h: 36.0,
            btn_h: 30.0,
            btn_pad_x: 14.0,
            input_h: 31.0,
            input_pad_x: 12.0,
            search_w: 260.0,
            search_expr_w: 340.0,
            chip_h: 26.0,
            sb_pill_h: 22.0,
            scrollbar_w: 12.0,
            r_btn: 6,
            r_input: 6,
            r_menu: 8,
            r_dialog: 10,
            r_tip: 5,
            r_tab: 7,
            r_menu_item: 5,
            font_grid: 13.0,
            font_ui: 13.0,
            font_header: 12.5,
            font_small: 12.0,
            font_tip: 11.5,
            font_tiny: 11.0,
            font_micro: 9.0,
            font_title: 14.5,
        }
    }
}

impl Metrics {
    /// The header's height with the optional bands shown or not.
    pub fn head_h(&self, charts: bool, filters: bool) -> f32 {
        self.header_h + if charts { self.spk_h } else { 0.0 } + if filters { self.filter_h } else { 0.0 }
    }

    /// The rows at density `d` (an index of `DENSITIES`; past them, the default).
    pub fn set_density(&mut self, d: usize) {
        let h = DENSITIES.get(d).map_or(DENSITIES[DEFAULT_DENSITY].1, |x| x.1);
        (self.row_h, self.totals_h) = (h, h);
    }

    /// The density in force.
    pub fn density(&self) -> usize {
        DENSITIES.iter().position(|d| d.1 == self.row_h).unwrap_or(DEFAULT_DENSITY)
    }
}

fn shadow(y: i8, blur: u8, a: f32) -> Shadow {
    Shadow { offset: [0, y], blur, spread: 0, color: Color32::from_black_alpha((a * 255.0) as u8) }
}

fn widget(bg: Color32, weak: Color32, brd: Color32, fg: Color32, r: u8) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: weak,
        bg_stroke: Stroke::new(1.0, brd),
        corner_radius: CornerRadius::same(r),
        fg_stroke: Stroke::new(1.0, fg),
        // The web app's controls never grow on hover.
        expansion: 0.0,
    }
}

/// Dress egui in `t`: panels, windows, menus, buttons (the outline `.btn-outline`
/// look; a primary button is `Button::fill(p.prim)` with `on_prim` text), fields
/// (`.bs-input`), scrollbars, text styles. Sets both of egui's styles (dark and
/// light) and the matching egui theme, so the OS preference cannot override it.
pub fn apply(ctx: &egui::Context, t: &Theme, m: &Metrics) {
    let p = &t.p;
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(m.font_tiny)),
            (TextStyle::Body, FontId::proportional(m.font_ui)),
            (TextStyle::Button, FontId::proportional(m.font_ui)),
            (TextStyle::Heading, FontId::proportional(m.font_title)),
            (TextStyle::Monospace, FontId::new(m.font_grid, FontFamily::Monospace)),
        ]
        .into();
        s.animation_time = 0.12;
        // Tooltips once the pointer rests, not on every pass over a button: only buttons
        // without a label and values cut short carry one. The grid's and charts' own
        // (grid::tip) wait as long.
        s.interaction.tooltip_delay = crate::grid::TIP_DELAY as f32;
        s.interaction.show_tooltips_only_when_still = true;

        let sp = &mut s.spacing;
        sp.item_spacing = vec2(8.0, 6.0);
        sp.button_padding = vec2(m.btn_pad_x, 6.0);
        sp.interact_size = vec2(24.0, m.btn_h);
        sp.window_margin = Margin::same(20);
        sp.menu_margin = Margin::same(6);
        sp.menu_spacing = 2.0;
        sp.menu_width = 230.0;
        sp.icon_width = 16.0;
        sp.icon_width_inner = 10.0;
        sp.icon_spacing = 8.0;
        sp.text_edit_width = m.search_w;
        sp.tooltip_width = 420.0;
        sp.combo_height = 320.0;
        // Always shown, 6 px thumbs on no track, as the web app's ::-webkit-scrollbar.
        sp.scroll = ScrollStyle {
            floating: true,
            bar_width: 6.0,
            floating_width: 6.0,
            floating_allocated_width: m.scrollbar_w,
            bar_inner_margin: 3.0,
            bar_outer_margin: 3.0,
            handle_min_length: 22.0,
            foreground_color: false,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            dormant_handle_opacity: 1.0,
            active_handle_opacity: 1.0,
            interact_handle_opacity: 1.0,
            ..ScrollStyle::solid()
        };

        let v = &mut s.visuals;
        // A pointing hand over what can be clicked, as the web app's `cursor: pointer`
        // (egui's buttons; other click-only responses: `pointing_hand`).
        v.interact_cursor = Some(egui::CursorIcon::PointingHand);
        // Text as the typeface was drawn, for HiDPI and 4K screens: outlines
        // rasterized unhinted (no snapping of stems or heights to the pixel grid),
        // glyphs placed at their exact advances (quarter-pixel bins rather than
        // whole pixels), greyscale coverage only (never LCD sub-pixels). Coverage
        // becomes opacity as linear-light blending would: egui blends in sRGB, so
        // light-on-dark text takes coverage^(1/2.2) — the exact correction, where
        // egui's default (2c − c²) is an approximation that thickens the strokes.
        // Dark-on-light text keeps the raw coverage, the usual gamma-space look.
        v.text_options.font_hinting = false;
        v.text_options.subpixel_binning = true;
        v.text_options.color_transfer_function = if t.dark {
            egui::epaint::FontColorTransferFunction::Gamma(1.0 / 2.2)
        } else {
            egui::epaint::FontColorTransferFunction::Off
        };
        v.dark_mode = t.dark;
        v.override_text_color = None;
        v.weak_text_color = Some(p.txt_mut);
        v.hyperlink_color = p.accent_2;
        v.faint_bg_color = p.row_alt;
        v.extreme_bg_color = p.bg_deep;
        v.text_edit_bg_color = Some(p.bg_deep);
        v.code_bg_color = p.surf_2;
        v.warn_fg_color = p.warn;
        v.error_fg_color = p.danger_txt;
        v.panel_fill = p.bg;
        v.window_fill = p.surf;
        v.window_stroke = Stroke::new(1.0, p.brd);
        v.window_corner_radius = CornerRadius::same(m.r_dialog);
        v.window_shadow = shadow(18, 50, 0.55);
        v.window_highlight_topmost = false;
        v.menu_corner_radius = CornerRadius::same(m.r_menu);
        v.popup_shadow = shadow(10, 30, 0.5);
        v.selection.bg_fill = alpha(p.accent, 0.25);
        // Also the frame of a focused field.
        v.selection.stroke = Stroke::new(1.0, p.prim);
        v.text_cursor.stroke = Stroke::new(1.5, p.txt_hi);
        v.striped = false;
        v.indent_has_left_vline = false;
        v.collapsing_header_frame = false;
        v.button_frame = true;
        v.slider_trailing_fill = true;
        v.disabled_alpha = 0.45;
        // bg_fill is a scrollbar thumb and a checkbox's box; weak_bg_fill a button's fill.
        let r = m.r_btn;
        v.widgets.noninteractive = widget(p.surf, p.surf, p.brd, p.txt, r);
        v.widgets.inactive = widget(p.surf_3, Color32::TRANSPARENT, p.brd, p.txt, r);
        v.widgets.hovered = widget(p.brd_2, p.surf_2, p.brd_2, p.txt_hi, r);
        v.widgets.active = widget(p.brd_2, p.surf_3, p.brd_2, p.txt_strong, r);
        v.widgets.open = widget(p.surf_2, p.surf_2, p.prim, p.txt_hi, r);
    });
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
}

/// A menu or context menu (`.dd-menu`): `surf`, 1 px `brd`, radius 8, padding 6, shadow 0 10 30 / .5.
pub fn menu_frame(p: &Palette, m: &Metrics) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surf)
        .stroke(Stroke::new(1.0, p.brd))
        .corner_radius(m.r_menu)
        .inner_margin(6)
        .shadow(shadow(10, 30, 0.5))
}

/// A dialog (`#dlg`, `.modal-box`): `surf`, 1 px `brd`, radius 10, padding 22 / 18 at the bottom.
pub fn dialog_frame(p: &Palette, m: &Metrics) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surf)
        .stroke(Stroke::new(1.0, p.brd))
        .corner_radius(m.r_dialog)
        .inner_margin(Margin { left: 22, right: 22, top: 22, bottom: 18 })
        .shadow(shadow(18, 50, 0.55))
}

/// A tooltip (`#tip`): `tip_bg` at .72 (the web blurs what is under it), `tip_brd` at .7, radius 5, padding 4 × 8.
pub fn tooltip_frame(p: &Palette, m: &Metrics) -> egui::Frame {
    egui::Frame::new()
        .fill(p.tip_fill)
        .stroke(Stroke::new(1.0, p.tip_stroke))
        .corner_radius(m.r_tip)
        .inner_margin(Margin::symmetric(8, 4))
        .shadow(shadow(4, 14, 0.35))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_from_css() {
        let d = &by_id("dark").p;
        assert_eq!(d.bg, h(0x0d1117));
        assert_eq!(d.accent, h(0x388bfd));
        assert_eq!(d.ok_rgb, h(0x2ea043));
        assert_eq!(d.row_bg, d.bg); // no --row-bg in GitHub Dark
        // color-mix(in srgb, #161b22 45%, #010409)
        assert_eq!(d.addon_bg, Color32::from_rgb(10, 14, 20));
        let tn = &by_id("tokyonight").p;
        assert_eq!(tn.row_bg, h(0x0f1016));
        assert_eq!(by_id("vscode").p.addon_bg, by_id("vscode").p.bg);
        assert_eq!(by_id("light").p.addon_bg, by_id("light").p.surf_2);
        assert_eq!(by_id("light").p.warn_rgb, h(0xd4a72c));
        assert!(!by_id("latte").dark);
        assert_eq!(by_id("nope").id, DEFAULT_THEME);
        assert_eq!(themes().len(), 6);
    }

    #[test]
    fn mixes() {
        let d = &by_id("dark").p;
        assert_eq!(mix(h(0xffffff), h(0x000000), 0.5), Color32::from_rgb(128, 128, 128));
        assert_eq!(alpha(h(0x102030), 1.0), h(0x102030));
        assert_eq!(d.row_cur_alt, mix(d.accent, d.row_alt, 0.16));
        assert_eq!(d.sel_fill.a(), 46);
    }
}

/// The pointing hand over every enabled click-only response (`Sense::click()`: chips, menu
/// rows, the grid's buttons), which egui's `interact_cursor` gives its buttons only. Not
/// over what also drags (the grid's body, splitters), nor where a widget chose a cursor
/// (the text fields' I-beam). To install once per context; a few hash lookups a frame.
pub fn pointing_hand(ctx: &egui::Context) {
    ctx.on_end_pass("pointing-hand", std::sync::Arc::new(|ui| {
        let ctx = ui.ctx();
        if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Default {
            return;
        }
        let click = ctx.viewport(|v| {
            v.interact_widgets.hovered.iter().any(|id| {
                v.this_pass.widgets.get(*id).is_some_and(|w| w.enabled && w.sense.senses_click() && !w.sense.senses_drag())
            })
        });
        if click {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }));
}

#[cfg(test)]
mod cursor_tests {
    use eframe::egui::{self, pos2, CursorIcon, Event, RawInput, Rect, Sense, Vec2};

    /// The cursor over `at` after two frames (hover is known from the frame before).
    fn cursor_at(at: egui::Pos2) -> CursorIcon {
        let ctx = egui::Context::default();
        super::pointing_hand(&ctx);
        super::apply(&ctx, super::by_id(super::DEFAULT_THEME), &super::Metrics::default());
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 300.0))),
            events: vec![Event::PointerMoved(at)],
            ..Default::default()
        };
        let mut cursor = CursorIcon::Default;
        for _ in 0..3 {
            let mut out = ctx.run_ui(input(), |ui| {
                let _ = ui.put(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(100.0, 30.0)), egui::Button::new("b"));
                let _ = ui.interact(Rect::from_min_size(pos2(0.0, 50.0), Vec2::new(100.0, 30.0)), egui::Id::unique("chip"), Sense::click());
                let _ = ui.interact(Rect::from_min_size(pos2(0.0, 100.0), Vec2::new(100.0, 30.0)), egui::Id::unique("body"), Sense::click_and_drag());
                let mut s = String::from("text");
                let _ = ui.put(Rect::from_min_size(pos2(0.0, 150.0), Vec2::new(100.0, 30.0)), egui::TextEdit::singleline(&mut s));
            });
            out.textures_delta.clear();
            cursor = out.platform_output.cursor_icon;
        }
        cursor
    }

    #[test]
    fn pointing_hand_over_clickables_only() {
        assert_eq!(cursor_at(pos2(10.0, 10.0)), CursorIcon::PointingHand, "button");
        assert_eq!(cursor_at(pos2(10.0, 60.0)), CursorIcon::PointingHand, "click response");
        assert_eq!(cursor_at(pos2(10.0, 110.0)), CursorIcon::Default, "click and drag");
        assert_eq!(cursor_at(pos2(10.0, 160.0)), CursorIcon::Text, "text field");
        assert_eq!(cursor_at(pos2(300.0, 250.0)), CursorIcon::Default, "nothing");
    }
}
