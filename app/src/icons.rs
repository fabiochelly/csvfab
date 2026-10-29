//! The web app's SVG icons (`assets/icons`, see its README), drawn crisp and tinted.
//!
//! Each icon is rasterized by resvg at the exact pixel size it is shown at (its box × pixels per
//! point) and drawn 1:1 on the pixel grid: no texture scaling, so no blur, whatever the size or
//! the screen's scale. The files are white; the texture is multiplied by the colour, as the web
//! app's CSS masks and `currentColor` did. Textures are cached per (icon, pixel size).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, pos2, Color32, ColorImage, Context, Id, Painter, Rect, SizeHint, TextureHandle, TextureOptions, Vec2};

macro_rules! icons {
    ($($v:ident = $f:literal),* $(,)?) => {
        /// One per file of `assets/icons`: the variant is the file name in PascalCase.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[allow(dead_code)] // the whole set ships; the app draws a subset of it
        pub enum Icon { $($v),* }

        impl Icon {
            #[allow(dead_code)]
            pub const ALL: &[Icon] = &[$(Icon::$v),*];

            pub fn name(self) -> &'static str {
                match self { $(Icon::$v => $f),* }
            }

            fn svg(self) -> &'static [u8] {
                match self { $(Icon::$v => include_bytes!(concat!("../assets/icons/", $f, ".svg"))),* }
            }
        }
    };
}

icons! {
    AccentE = "accent-e", AccentMark = "accent-mark", Anonymise = "anonymise", ArrowUp = "arrow-up", Bars = "bars",
    Bolt = "bolt", Calendar = "calendar", Card = "card", Chart = "chart", Check = "check", Checkbox = "checkbox",
    ChevronDown = "chevron-down", ChevronDownBold = "chevron-down-bold", ChevronDownWide = "chevron-down-wide",
    ChevronLeft = "chevron-left", ChevronRight = "chevron-right", ChevronUpBold = "chevron-up-bold", Clean = "clean",
    Code = "code", ColorScale = "color-scale", Columns = "columns", CombineFiles = "combine-files", Command = "command",
    Compare = "compare", Convert = "convert", ConvertColumn = "convert-column", Copy = "copy", Cross = "cross",
    Cut = "cut", DashedSquare = "dashed-square", DataBars = "data-bars", Discard = "discard", DupDelete = "dup-delete",
    DupMerge = "dup-merge", Duplicates = "duplicates", Edit = "edit", Extract = "extract", Eye = "eye",
    EyeOff = "eye-off", File = "file", FileDiff = "file-diff", FilePlus = "file-plus", FileText = "file-text",
    FillBlanks = "fill-blanks", FillDown = "fill-down", Find = "find", FontCheck = "font-check", Formula = "formula",
    Freeze = "freeze", FreezeColumn = "freeze-column", Funnel = "funnel", FunnelClear = "funnel-clear",
    FunnelFilled = "funnel-filled", FunnelX = "funnel-x", Fx = "fx", Goto = "goto", GroupBy = "group-by",
    Hamburger = "hamburger", Heat = "heat", Help = "help", Hide = "hide", History = "history", InDep = "in-dep",
    InDup = "in-dup", InFar = "in-far", InKey = "in-key", InNone = "in-none", InRel = "in-rel", InShape = "in-shape",
    InSpace = "in-space", Info = "info", InsertAfter = "insert-after", InsertBefore = "insert-before", Kebab = "kebab",
    Keep = "keep", Keyboard = "keyboard", Link = "link", Logo = "logo", Magnifier = "magnifier",
    MagnifierFilter = "magnifier-filter", Map = "map", MenuCheck = "menu-check", Merge = "merge",
    MergeColumns = "merge-columns", MergeDrop = "merge-drop", MergeKeep = "merge-keep", Noise = "noise",
    NotEqual = "not-equal", Open = "open", Paste = "paste", Pivot = "pivot", Plus = "plus", Profile = "profile",
    Quit = "quit", Radio = "radio", Redo = "redo", Regex = "regex", Revert = "revert", Row = "row",
    RowAbove = "row-above", RowBelow = "row-below", Rows = "rows", Save = "save", Schema = "schema",
    SelectChevron = "select-chevron", Sigma = "sigma", Sliders = "sliders", Sparkle = "sparkle", Sparkles = "sparkles",
    Split = "split", SplitColumn = "split-column", SplitFiles = "split-files", ToastDanger = "toast-danger",
    ToastInfo = "toast-info", ToastOk = "toast-ok", ToastWarn = "toast-warn", Totals = "totals", Trash = "trash",
    Undo = "undo", Upload = "upload", Warning = "warning",
}

/// The chrome's icons at the sizes they are drawn (chrome.rs, grid.rs), rasterized before the
/// first frame paints. Any other (icon, size) is rasterized on first use: tens of µs.
const PRELOAD: &[(Icon, f32, f32)] = &[
    (Icon::Hamburger, 18.0, 18.0), (Icon::Fx, 22.0, 19.0), (Icon::AccentE, 18.0, 18.0), (Icon::AccentMark, 18.0, 18.0),
    (Icon::Regex, 18.0, 18.0), (Icon::NotEqual, 18.0, 18.0), (Icon::Cross, 10.0, 10.0), (Icon::Cross, 12.0, 12.0),
    (Icon::Plus, 14.0, 14.0), (Icon::FunnelX, 16.0, 16.0), (Icon::Funnel, 16.0, 16.0), (Icon::ChevronDown, 16.0, 16.0),
    (Icon::ChevronDownWide, 16.0, 12.0), (Icon::Save, 16.0, 16.0), (Icon::Extract, 16.0, 16.0), (Icon::Discard, 16.0, 16.0),
    (Icon::Bars, 18.0, 18.0), (Icon::Bars, 20.0, 20.0), (Icon::FunnelFilled, 15.0, 15.0), (Icon::FunnelFilled, 17.0, 17.0),
    (Icon::Sigma, 15.0, 15.0), (Icon::Sigma, 17.0, 17.0), (Icon::Magnifier, 18.0, 18.0), (Icon::Upload, 23.0, 23.0),
    (Icon::ArrowUp, 24.0, 24.0), (Icon::ToastOk, 16.0, 16.0),
    // The grid's header, filter and charts rows.
    (Icon::Calendar, 11.0, 11.0), (Icon::ArrowUp, 12.0, 12.0), (Icon::FunnelFilled, 13.0, 13.0),
    (Icon::ChevronDown, 13.0, 13.0), (Icon::MagnifierFilter, 11.0, 11.0), (Icon::Sliders, 16.0, 16.0),
    (Icon::Bars, 15.0, 15.0),
];

/// To call once at startup: rasterizes [`PRELOAD`] at the start of the first pass (the scale is
/// only known then) and again whenever the scale changes (a screen of another scale, a zoom).
pub fn install(ctx: &Context) {
    ctx.on_begin_pass("icons", Arc::new(|ui| {
        let ctx = ui.ctx();
        // A first frame left empty: its scale is about to change (app::provisional).
        if !crate::app::provisional(ctx) {
            preload(ctx, ctx.pixels_per_point());
        }
    }));
}

/// Rasterizes [`PRELOAD`] for `ppp`, once (the textures are made then; the images may
/// already be there, from [`rasterize_ahead`]).
fn preload(ctx: &Context, ppp: f32) {
    let id = Id::unique("icons-preloaded-at");
    if ctx.data(|d| d.get_temp::<f32>(id)) != Some(ppp) {
        ctx.data_mut(|d| d.insert_temp(id, ppp));
        let mut ahead = AHEAD.lock().ok().and_then(|mut a| a.take()).filter(|(s, _)| *s == ppp).map(|(_, v)| v).unwrap_or_default();
        for &(icon, w, h) in PRELOAD {
            let px = pixels(Vec2::new(w, h), ppp);
            let image = ahead.iter().position(|(k, _)| *k == (icon, px)).map(|i| ahead.swap_remove(i).1);
            let _ = texture_from(ctx, icon, px, image);
        }
    }
}

/// [`PRELOAD`]'s images rasterized ahead, for a scale.
#[allow(clippy::type_complexity)]
static AHEAD: Mutex<Option<(f32, Vec<((Icon, [u32; 2]), ColorImage)>)>> = Mutex::new(None);

/// Rasterizes [`PRELOAD`] for the scale the window is expected at, on the calling thread
/// (`crate::early`, before the window exists): the first frame drawn at that scale only
/// makes the textures. Only the images: a texture made now would be uploaded by the
/// frame drawn before the scale is known.
pub fn rasterize_ahead(ppp: f32) {
    let v = PRELOAD.iter().map(|&(icon, w, h)| { let px = pixels(Vec2::new(w, h), ppp); ((icon, px), rasterize(icon, px)) }).collect();
    if let Ok(mut a) = AHEAD.lock() { *a = Some((ppp, v)); }
}

/// Draws `icon` in `color`, fitted to `rect` (centred, its aspect kept, like CSS `contain`).
pub fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    draw(painter, icon, rect, color, false);
}

/// `turned`: rotated 180°, as the web app's CSS turns the sort arrow for descending.
fn draw(painter: &Painter, icon: Icon, rect: Rect, color: Color32, turned: bool) {
    let ppp = painter.pixels_per_point();
    let px = pixels(rect.size(), ppp);
    if px[0] == 0 || px[1] == 0 {
        return;
    }
    let slot = texture(painter.ctx(), icon, px);
    let size = Vec2::new(slot.size[0] as f32, slot.size[1] as f32);
    // On whole pixels, so each texel lands on one pixel: drawn as rasterized.
    let min = ((rect.center().to_vec2() * ppp - size / 2.0).round() / ppp).to_pos2();
    let uv = if turned { Rect::from_min_max(slot.uv.max, slot.uv.min) } else { slot.uv };
    painter.image(slot.tex, Rect::from_min_size(min, size / ppp), uv, color);
}

/// The box's size in physical pixels.
fn pixels(size: Vec2, ppp: f32) -> [u32; 2] {
    let p = (size * ppp).round();
    [p.x.max(0.0) as u32, p.y.max(0.0) as u32]
}

/// Where an icon at a size sits: its page of the atlas, its texels, its size.
#[derive(Clone, Copy)]
struct Slot {
    tex: egui::TextureId,
    uv: Rect,
    size: [u32; 2],
}

/// Every icon drawn, at every size, packed into a few atlas pages rather than one texture
/// each: a frame then binds one icon texture instead of twenty, and egui batches the
/// icons' quads into a handful of meshes.
const PAGE: usize = 512;

struct Page {
    tex: TextureHandle,
    x: usize,
    y: usize,
    row_h: usize,
}

#[derive(Default)]
struct Atlas {
    pages: Vec<Page>,
    slots: HashMap<(Icon, [u32; 2]), Slot>,
}

#[derive(Clone, Default)]
struct Cache(Arc<Mutex<Atlas>>);

/// The slot of `icon` fitted in a `px` box, rasterized on first use.
fn texture(ctx: &Context, icon: Icon, px: [u32; 2]) -> Slot {
    texture_from(ctx, icon, px, None)
}

/// [`texture`], from `image` if it is already rasterized.
fn texture_from(ctx: &Context, icon: Icon, px: [u32; 2], image: Option<ColorImage>) -> Slot {
    let id = Id::unique("icons");
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(id).clone());
    let mut atlas = cache.0.lock().unwrap();
    if let Some(s) = atlas.slots.get(&(icon, px)) {
        return *s;
    }
    let image = image.unwrap_or_else(|| rasterize(icon, px));
    let [w, h] = image.size;
    // Shelf packing, a transparent texel between neighbours (LINEAR sampling at the edges).
    let fits = |p: &Page| (p.x + w + 1 <= PAGE && p.y + h <= PAGE) || (p.y + p.row_h + 1 + h <= PAGE && w <= PAGE);
    if w > PAGE || h > PAGE {
        // Too big for a page (a huge zoom): a texture of its own.
        let tex = ctx.load_texture(format!("icon-{}-{}x{}", icon.name(), w, h), image, TextureOptions::LINEAR);
        let slot = Slot { tex: tex.id(), uv: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), size: [w as u32, h as u32] };
        atlas.pages.push(Page { tex, x: PAGE, y: PAGE, row_h: 0 });
        atlas.slots.insert((icon, px), slot);
        return slot;
    }
    if atlas.pages.last().is_none_or(|p| !fits(p)) {
        // Outside the data lock already: making a texture takes the context's lock.
        let n = atlas.pages.len();
        let tex = ctx.load_texture(format!("icons-{n}"), ColorImage::filled([PAGE, PAGE], Color32::TRANSPARENT), TextureOptions::LINEAR);
        atlas.pages.push(Page { tex, x: 0, y: 0, row_h: 0 });
    }
    let page = atlas.pages.last_mut().unwrap();
    if page.x + w + 1 > PAGE {
        page.x = 0;
        page.y += page.row_h + 1;
        page.row_h = 0;
    }
    let (x, y) = (page.x, page.y);
    page.tex.set_partial([x, y], image, TextureOptions::LINEAR);
    page.x += w + 1;
    page.row_h = page.row_h.max(h);
    let k = PAGE as f32;
    let slot = Slot { tex: page.tex.id(), uv: Rect::from_min_max(pos2(x as f32 / k, y as f32 / k), pos2((x + w) as f32 / k, (y + h) as f32 / k)), size: [w as u32, h as u32] };
    atlas.slots.insert((icon, px), slot);
    slot
}

/// resvg's rendering of `icon`, as large as fits in `px` with its aspect kept (the files'
/// width/height are their viewBox, so the whole viewBox is scaled to the image).
fn rasterize(icon: Icon, px: [u32; 2]) -> ColorImage {
    let hint = SizeHint::Size { width: px[0], height: px[1], maintain_aspect_ratio: true };
    egui_extras::image::load_svg_bytes_with_size(icon.svg(), hint, &Default::default())
        .unwrap_or_else(|e| panic!("assets/icons/{}.svg: {e}", icon.name()))
}

// The drawing functions the chrome and the grid call. The stroke widths they pass are the
// web app's, now in the SVGs, so ignored — except the magnifier's, which tells which of its two
// web versions is meant.

pub fn chevron_down(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::ChevronDown, r, c);
}

pub fn arrow(p: &Painter, r: Rect, up: bool, _w: f32, c: Color32) {
    draw(p, Icon::ArrowUp, r, c, !up);
}

/// The palette's (stroke 2), or the filter row's (2.5, `w` ≥ 2.25).
pub fn magnifier(p: &Painter, r: Rect, w: f32, c: Color32) {
    paint(p, if w >= 2.25 { Icon::MagnifierFilter } else { Icon::Magnifier }, r, c);
}

pub fn funnel(p: &Painter, r: Rect, filled: bool, c: Color32) {
    paint(p, if filled { Icon::FunnelFilled } else { Icon::Funnel }, r, c);
}

pub fn sliders(p: &Painter, r: Rect, c: Color32) {
    paint(p, Icon::Sliders, r, c);
}

pub fn bars(p: &Painter, r: Rect, c: Color32) {
    paint(p, Icon::Bars, r, c);
}

pub fn calendar(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::Calendar, r, c);
}

pub fn sigma(p: &Painter, r: Rect, c: Color32) {
    paint(p, Icon::Sigma, r, c);
}

pub fn cross(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::Cross, r, c);
}

pub fn hamburger(p: &Painter, r: Rect, c: Color32) {
    paint(p, Icon::Hamburger, r, c);
}

pub fn fx(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::Fx, r, c);
}

pub fn not_equal(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::NotEqual, r, c);
}

pub fn regex(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::Regex, r, c);
}

/// The accent toggle's é: the letter in `c`, its accent in `accent` (the web app's two colours).
#[allow(dead_code)]
pub fn accent_e(p: &Painter, r: Rect, c: Color32, accent: Color32) {
    paint(p, Icon::AccentE, r, c);
    paint(p, Icon::AccentMark, r, accent);
}

/// The toast's tick.
pub fn check(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::ToastOk, r, c);
}

#[allow(dead_code)]
pub fn save(p: &Painter, r: Rect, c: Color32) {
    paint(p, Icon::Save, r, c);
}

pub fn plus(p: &Painter, r: Rect, _w: f32, c: Color32) {
    paint(p, Icon::Plus, r, c);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{epaint::Primitive, RawInput, ViewportId, ViewportInfo};
    use std::time::Instant;

    #[test]
    fn every_icon_rasterizes_and_is_white() {
        for &icon in Icon::ALL {
            let img = rasterize(icon, [32, 32]);
            assert!(img.size[0] <= 32 && img.size[1] <= 32 && img.size.contains(&32), "{}: {:?}", icon.name(), img.size);
            // Opaque somewhere, and untinted (white) wherever opaque, but the logo.
            let a = img.pixels.iter().map(|p| p.a()).max().unwrap();
            assert!(a > 200, "{} is empty", icon.name());
            if icon != Icon::Logo {
                assert!(img.pixels.iter().all(|p| p.r() == p.a() && p.g() == p.a() && p.b() == p.a()), "{} is not white", icon.name());
            }
        }
    }

    #[test]
    fn rasterizes_at_the_shown_pixel_size() {
        let ctx = Context::default();
        install(&ctx);
        let mut input = RawInput::default();
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(2.0), ..Default::default() });
        let mut out = None;
        for pass in 0..2 {
            let t = Instant::now();
            let mut o = ctx.run_ui(input.clone(), |ui| {
                let r = Rect::from_min_size(pos2(10.3, 20.6), Vec2::splat(13.0));
                funnel(ui.painter(), r, true, Color32::RED);
                fx(ui.painter(), Rect::from_min_size(pos2(40.0, 20.0), Vec2::new(22.0, 19.0)), 1.8, Color32::RED);
            });
            eprintln!("pass {pass} at ×{}: {:?}", o.pixels_per_point, t.elapsed());
            o.textures_delta.clear(); // no renderer to hand them to
            out = Some(o);
        }
        let out = out.unwrap();
        assert_eq!(out.pixels_per_point, 2.0);
        let meshes: Vec<_> = ctx
            .tessellate(out.shapes, 2.0)
            .into_iter()
            .filter_map(|p| match p.primitive { Primitive::Mesh(m) if m.texture_id != Default::default() => Some(m), _ => None })
            .collect();
        // Both icons from one atlas page: one texture bound, one mesh.
        assert_eq!(meshes.len(), 1);
        assert_eq!(ctx.tex_manager().read().meta(meshes[0].texture_id).unwrap().size, [PAGE, PAGE]);
        // Each quad as large as rasterized: 13 pt at ×2 is 26 px (not a scaled 13 px); fx keeps its 34:30.
        let quads: Vec<[f32; 2]> = meshes[0].vertices.chunks(4).map(|q| {
            let (x0, x1) = q.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.pos.x), b.max(v.pos.x)));
            let (y0, y1) = q.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.pos.y), b.max(v.pos.y)));
            [(x1 - x0) * 2.0, (y1 - y0) * 2.0]
        }).collect();
        assert_eq!(quads, [[26.0, 26.0], [43.0, 38.0]]);
        for m in &meshes {
            let b = m.calc_bounds();
            for v in [b.min.x, b.min.y, b.max.x, b.max.y] {
                assert_eq!((v * 2.0).fract(), 0.0, "not on the pixel grid: {b:?}");
            }
        }
    }

    #[test]
    fn timings() {
        let t = Instant::now();
        for &icon in Icon::ALL {
            rasterize(icon, [32, 32]);
        }
        let all = t.elapsed();
        let t = Instant::now();
        for &(icon, w, h) in PRELOAD {
            rasterize(icon, pixels(Vec2::new(w, h), 2.0));
        }
        eprintln!("{} icons at 32 px: {all:?}; the {} preloaded at ×2: {:?}", Icon::ALL.len(), PRELOAD.len(), t.elapsed());
    }
}
