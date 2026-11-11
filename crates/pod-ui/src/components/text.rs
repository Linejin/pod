//! Word-boundary (keep-all) text wrapping.
use gpui_kit::*;
/// Korean-friendly wrapping (CSS `word-break: keep-all`): lines break only between
/// words (spaces), never inside a word as GPUI's per-character CJK wrapping does.
/// Centered rows of unbreakable words; the text itself is unchanged.
pub fn keep_all(text: &str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap_x(px(4.))
        .children(keep_all_words(text).into_iter().map(|w| {
            div()
                .whitespace_nowrap()
                .child(SharedString::from(w.to_string()))
        }))
}
/// [`keep_all`] left-aligned (banners, descriptions). A piece longer than [`KEEP_ALL_MAX`]
/// characters (a path, a URL) may still break so it never overflows its box.
pub fn keep_all_start(text: &str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .gap_x(px(4.))
        .children(keep_all_words(text).into_iter().map(|w| {
            let piece = div()
                .max_w_full()
                .min_w_0()
                .child(SharedString::from(w.to_string()));
            if keep_all_unbreakable(w) {
                piece.whitespace_nowrap()
            } else {
                piece
            }
        }))
}
/// Longest piece kept on one line by [`keep_all_start`].
pub const KEEP_ALL_MAX: usize = 24;
/// Whether [`keep_all_start`] keeps this piece unbroken.
pub fn keep_all_unbreakable(word: &str) -> bool {
    word.chars().count() <= KEEP_ALL_MAX
}
/// The unbreakable pieces of [`keep_all`].
pub fn keep_all_words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}
