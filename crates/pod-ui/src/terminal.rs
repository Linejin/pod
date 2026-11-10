//! A default VT palette, never derived from ThemeColor or applied to truecolor output.
use crate::{Mode, Tokens};
#[derive(Clone, Copy, Debug)]
pub struct TerminalPalette {
    pub background: u32,
    pub foreground: u32,
    pub selection: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
}
impl TerminalPalette {
    pub fn for_mode(mode: Mode) -> Self {
        let t = Tokens::for_mode(mode);
        Self {
            background: t.terminal_bg,
            foreground: t.terminal_fg,
            selection: t.terminal_selection,
            cursor: t.terminal_cursor,
            ansi: match mode {
                Mode::Dark => [
                    0x1e2127, 0xe06c75, 0x98c379, 0xd19a66, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                    0x5c6370, 0xff8c95, 0xb8e399, 0xf1ba86, 0x81cfff, 0xe698fd, 0x76d6e2, 0xffffff,
                ],
                Mode::Light => [
                    0x243246, 0xa52239, 0x286b36, 0x805200, 0x205ca5, 0x85409a, 0x176877, 0xc8d0dc,
                    0x596a80, 0xc6344d, 0x367f44, 0x976600, 0x3274bf, 0x9a53ae, 0x25818e, 0xffffff,
                ],
            },
        }
    }
    pub fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let c = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
                (c(i / 36) << 16) | (c(i / 6 % 6) << 8) | c(i % 6)
            }
            _ => {
                let v = 8 + (i as u32 - 232) * 10;
                (v << 16) | (v << 8) | v
            }
        }
    }
}
