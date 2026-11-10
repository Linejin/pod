//! xterm mouse reporting (modes 1000/1002/1003, encodings legacy/1005/1006).

use crate::vt::{Modes, MouseTracking};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held (mode 1003 only).
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseModes {
    pub tracking: MouseTracking,
    pub sgr: bool,
    pub utf8: bool,
}

impl From<&Modes> for MouseModes {
    fn from(m: &Modes) -> Self {
        Self {
            tracking: m.mouse,
            sgr: m.mouse_sgr,
            utf8: m.mouse_utf8,
        }
    }
}

/// Encode a mouse event at 0-based cell (`col`, `row`), or `None` if the
/// current mode does not report it (or legacy encoding cannot express it).
pub fn encode_mouse(
    action: Action,
    button: Button,
    col: u16,
    row: u16,
    mods: MouseMods,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    match modes.tracking {
        MouseTracking::Off => return None,
        MouseTracking::Click if action == Action::Motion => return None,
        MouseTracking::Drag if action == Action::Motion && button == Button::None => return None,
        _ => {}
    }
    let wheel = matches!(button, Button::WheelUp | Button::WheelDown);
    if wheel && action != Action::Press {
        return None;
    }
    let base: u32 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    let mut code = base;
    if action == Action::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    let (x, y) = (col as u32 + 1, row as u32 + 1);

    if modes.sgr {
        let fin = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{fin}").into_bytes());
    }

    // Legacy encodings report every release as button 3.
    if action == Action::Release {
        code = 3 + (code & !3 & !64);
    }
    let mut out = b"\x1b[M".to_vec();
    out.push((32 + code) as u8);
    if modes.utf8 {
        for v in [x, y] {
            let v = 32 + v;
            if v > 2047 {
                return None;
            }
            let mut buf = [0u8; 4];
            out.extend_from_slice(char::from_u32(v)?.encode_utf8(&mut buf).as_bytes());
        }
    } else {
        for v in [x, y] {
            let v = 32 + v;
            if v > 255 {
                return None;
            }
            out.push(v as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: MouseMods = MouseMods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    fn modes(tracking: MouseTracking, sgr: bool) -> MouseModes {
        MouseModes {
            tracking,
            sgr,
            utf8: false,
        }
    }

    #[test]
    fn off_reports_nothing() {
        assert_eq!(
            encode_mouse(
                Action::Press,
                Button::Left,
                0,
                0,
                NONE,
                MouseModes::default()
            ),
            None
        );
    }

    #[test]
    fn sgr_press_release() {
        let m = modes(MouseTracking::Click, true);
        assert_eq!(
            encode_mouse(Action::Press, Button::Left, 4, 9, NONE, m).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            encode_mouse(Action::Release, Button::Left, 4, 9, NONE, m).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            encode_mouse(Action::Press, Button::Right, 0, 0, NONE, m).unwrap(),
            b"\x1b[<2;1;1M"
        );
        assert_eq!(
            encode_mouse(Action::Press, Button::Middle, 0, 0, NONE, m).unwrap(),
            b"\x1b[<1;1;1M"
        );
        // Large coordinates are fine in SGR.
        assert_eq!(
            encode_mouse(Action::Press, Button::Left, 299, 0, NONE, m).unwrap(),
            b"\x1b[<0;300;1M"
        );
    }

    #[test]
    fn sgr_modifiers_and_wheel() {
        let m = modes(MouseTracking::Click, true);
        let mods = MouseMods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(
            encode_mouse(Action::Press, Button::Left, 0, 0, mods, m).unwrap(),
            b"\x1b[<28;1;1M"
        );
        assert_eq!(
            encode_mouse(Action::Press, Button::WheelUp, 2, 3, NONE, m).unwrap(),
            b"\x1b[<64;3;4M"
        );
        assert_eq!(
            encode_mouse(Action::Press, Button::WheelDown, 2, 3, NONE, m).unwrap(),
            b"\x1b[<65;3;4M"
        );
        assert_eq!(
            encode_mouse(Action::Release, Button::WheelDown, 2, 3, NONE, m),
            None
        );
    }

    #[test]
    fn motion_by_mode() {
        let click = modes(MouseTracking::Click, true);
        let drag = modes(MouseTracking::Drag, true);
        let any = modes(MouseTracking::Motion, true);
        assert_eq!(
            encode_mouse(Action::Motion, Button::Left, 1, 1, NONE, click),
            None
        );
        assert_eq!(
            encode_mouse(Action::Motion, Button::Left, 1, 1, NONE, drag).unwrap(),
            b"\x1b[<32;2;2M"
        );
        assert_eq!(
            encode_mouse(Action::Motion, Button::None, 1, 1, NONE, drag),
            None
        );
        assert_eq!(
            encode_mouse(Action::Motion, Button::None, 1, 1, NONE, any).unwrap(),
            b"\x1b[<35;2;2M"
        );
    }

    #[test]
    fn legacy_encoding() {
        let m = modes(MouseTracking::Click, false);
        assert_eq!(
            encode_mouse(Action::Press, Button::Left, 0, 0, NONE, m).unwrap(),
            b"\x1b[M\x20\x21\x21"
        );
        assert_eq!(
            encode_mouse(Action::Release, Button::Left, 0, 0, NONE, m).unwrap(),
            b"\x1b[M\x23\x21\x21"
        );
        assert_eq!(
            encode_mouse(Action::Press, Button::WheelUp, 0, 0, NONE, m).unwrap(),
            b"\x1b[M\x60\x21\x21"
        );
        // Column 223 is the last one legacy encoding can express.
        assert!(encode_mouse(Action::Press, Button::Left, 222, 0, NONE, m).is_some());
        assert_eq!(
            encode_mouse(Action::Press, Button::Left, 223, 0, NONE, m),
            None
        );
        let ctrl = MouseMods { ctrl: true, ..NONE };
        assert_eq!(
            encode_mouse(Action::Release, Button::Right, 0, 0, ctrl, m).unwrap(),
            b"\x1b[M\x33\x21\x21"
        );
    }

    #[test]
    fn utf8_encoding() {
        let m = MouseModes {
            tracking: MouseTracking::Click,
            sgr: false,
            utf8: true,
        };
        let out = encode_mouse(Action::Press, Button::Left, 299, 0, NONE, m).unwrap();
        let mut want = b"\x1b[M ".to_vec();
        want.extend_from_slice("\u{14c}".as_bytes()); // 32 + 300
        want.push(b'!');
        assert_eq!(out, want);
    }
}
