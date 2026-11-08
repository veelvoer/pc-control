use eframe::egui::Key;
use proto::char_to_key;

pub const CTRL: u16 = 29;
pub const SHIFT: u16 = 42;
pub const ALT: u16 = 56;
pub const SUPER: u16 = 125;

/// egui key -> (evdev code, produces text on its own).
pub fn map(k: Key) -> Option<(u16, bool)> {
    use Key::*;
    let special = |c| Some((c, false));
    let text = |c| Some((c, true));
    match k {
        Escape => special(1), Tab => special(15), Backspace => special(14), Enter => special(28),
        Insert => special(110), Delete => special(111), Home => special(102), End => special(107),
        PageUp => special(104), PageDown => special(109),
        ArrowLeft => special(105), ArrowRight => special(106), ArrowUp => special(103), ArrowDown => special(108),
        F1 => special(59), F2 => special(60), F3 => special(61), F4 => special(62), F5 => special(63),
        F6 => special(64), F7 => special(65), F8 => special(66), F9 => special(67), F10 => special(68),
        F11 => special(87), F12 => special(88),
        Space => text(57), Minus => text(12), Equals => text(13), OpenBracket => text(26),
        CloseBracket => text(27), Semicolon => text(39), Quote => text(40), Backtick => text(41),
        Backslash => text(43), Comma => text(51), Period => text(52), Slash => text(53),
        _ => {
            let n = k.name();
            let mut it = n.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => char_to_key(c.to_ascii_lowercase()).map(|(code, _)| (code, true)),
                _ => None,
            }
        }
    }
}
