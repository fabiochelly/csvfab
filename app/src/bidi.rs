//! Right-to-left text in a line. egui shapes a run of one font in its script's direction, but
//! lays runs left to right and knows no paragraph direction: an Arabic sentence comes out with
//! its words in the wrong order, the digits of a Hebrew one backwards. `append` gives it the
//! pieces in display order (Unicode's bidirectional algorithm, unicode-bidi), one section a
//! word, each then shaped on its own. For one line drawn only: nothing here can be edited.

use eframe::egui::epaint::text::{ByteIndex, LayoutSection};
use eframe::egui::text::{LayoutJob, TextFormat};
use unicode_bidi::BidiInfo;

/// Whether `s` holds a right-to-left letter.
#[inline]
pub fn has_rtl(s: &str) -> bool {
    !s.is_ascii() && s.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF))
}

/// The pieces of `s` in display order, left to right. The paragraph's direction is its first
/// strong letter's; only the first line is looked at (a cell shows one).
pub fn pieces(s: &str) -> Vec<&str> {
    let line = s.split(['\n', '\r']).next().unwrap_or(s);
    let info = BidiInfo::new(line, None);
    let Some(para) = info.paragraphs.first() else { return vec![line] };
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut out = Vec::new();
    for r in runs {
        if !levels[r.start].is_rtl() {
            out.push(&line[r]);
            continue;
        }
        // Words backwards, each left for the shaper to run right to left.
        let run = &line[r];
        let mut words: Vec<&str> = Vec::new();
        let mut at = 0;
        for (i, c) in run.char_indices() {
            let space = c.is_whitespace();
            if space {
                if at < i { words.push(&run[at..i]); }
                words.push(&run[i..i + c.len_utf8()]);
                at = i + c.len_utf8();
            }
        }
        if at < run.len() { words.push(&run[at..]); }
        out.extend(words.into_iter().rev());
    }
    out
}

/// `s` into `job`, in display order.
pub fn append(job: &mut LayoutJob, s: &str, fmt: &TextFormat) {
    // Sections of their own (`append` would join equal ones), or the shaper sees one run again.
    for p in pieces(s) {
        let at = job.text.len();
        job.text.push_str(p);
        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(at)..ByteIndex(job.text.len()), format: fmt.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_order() {
        assert!(!has_rtl("Hôtel 東京"));
        assert!(has_rtl("a שלום"));
        assert_eq!(pieces("שלום עולם"), ["עולם", " ", "שלום"]);
        // A number keeps its own order, at its place.
        assert_eq!(pieces("ירושלים 45"), ["45", " ", "ירושלים"]);
        // Latin first: left to right, the Hebrew words reversed in their run.
        assert_eq!(pieces("abc שלום עולם def"), ["abc ", "עולם", " ", "שלום", " def"]);
        assert_eq!(pieces("مرحبا بالعالم"), ["بالعالم", " ", "مرحبا"]);
    }
}
