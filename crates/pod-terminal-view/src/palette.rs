//! Default colors (One Dark-ish). Engine-neutral: values are 0xRRGGBB.

pub const ANSI16: [u32; 16] = [
    0x1e2127, 0xe06c75, 0x98c379, 0xd19a66, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
    0xe06c75, 0x98c379, 0xd19a66, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
];
pub const DEFAULT_FG: u32 = 0xd6d8db;
pub const DEFAULT_BG: u32 = 0x1c1d21;
pub const CURSOR: u32 = 0xd6d8db;
pub const SELECTION_BG: u32 = 0x3e5c8a;
pub const SCROLLBAR: u32 = 0x8a8f98;

pub fn dim(c: u32) -> u32 {
    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
    ((r * 2 / 3) << 16) | ((g * 2 / 3) << 8) | (b * 2 / 3)
}

/// xterm 256-color palette entry.
pub fn xterm_256(i: u8) -> u32 {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (i as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

/// Host defaults, in 0xRRGGBB. Program OSC/SGR colors take precedence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalPalette {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub selection_bg: u32,
    pub ansi: [u32; 16],
}
impl Default for TerminalPalette {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FG,
            background: DEFAULT_BG,
            cursor: CURSOR,
            selection_bg: SELECTION_BG,
            ansi: ANSI16,
        }
    }
}
impl TerminalPalette {
    pub fn indexed(&self, index: u8) -> u32 {
        if index < 16 {
            self.ansi[index as usize]
        } else {
            xterm_256(index)
        }
    }
}
