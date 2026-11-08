//! Splits what the terminal sends into keys (SPEC.md "Keys"), in every encoding an agent may switch the terminal
//! into: plain bytes, the kitty keyboard protocol (`CSI code;mods:event u`, with or without
//! alternate keys and event types) and xterm's modifyOtherKeys (`CSI 27;mods;code ~`).

#[derive(Debug, PartialEq, Eq)]
pub struct Key {
    /// The key's unshifted character, when it is one.
    pub code: Option<u32>,
    pub ctrl: bool,
    /// Held with alt (meta): `ESC` and the key in a plain terminal, or a report's modifier.
    pub alt: bool,
    /// A release or repeat report, or a modifier key alone: carries no key press.
    pub noise: bool,
}

/// The first key in `b` and how many bytes it takes; `None` when `b` ends inside a sequence.
pub fn next(b: &[u8]) -> Option<(Key, usize)> {
    let first = *b.first()?;
    if first != 0x1b {
        // Ctrl+letter as a control byte.
        if (1..=26).contains(&first) && first != b'\r' && first != b'\t' && first != b'\n' && first != 0x08 {
            return Some((Key { code: Some(u32::from(first) + 96), ctrl: true, alt: false, noise: false }, 1));
        }
        let len = match first {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        }
        .min(b.len());
        let code = std::str::from_utf8(&b[..len]).ok().and_then(|s| s.chars().next()).map(u32::from);
        return Some((Key { code, ctrl: false, alt: false, noise: false }, len));
    }
    if b.len() < 2 {
        return None;
    }
    if b[1] != b'[' {
        // A plain terminal sends alt+key as `ESC` and the key. `ESC O` begins an arrow in
        // application mode, and is left alone.
        let alt = (0x20..0x7f).contains(&b[1]) && b[1] != b'O';
        let code = alt.then_some(u32::from(b[1]));
        return Some((Key { code, ctrl: false, alt, noise: false }, 2));
    }
    let mut j = 2;
    while j < b.len() && (0x20..=0x3f).contains(&b[j]) {
        j += 1;
    }
    if j >= b.len() {
        return None;
    }
    let fin = b[j];
    let len = j + 1;
    let params = std::str::from_utf8(&b[2..j]).unwrap_or("");
    let fields: Vec<&str> = params.split(';').collect();
    let num = |s: &str| s.split(':').next().and_then(|n| n.parse::<u32>().ok());
    let mods_event = |s: Option<&&str>| {
        let s = s.copied().unwrap_or("1");
        let mut it = s.split(':');
        let mods = it.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1);
        let event = it.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1);
        (mods, event)
    };
    match fin {
        b'u' if !params.starts_with('?') && !params.starts_with('>') && !params.starts_with('<') && !params.starts_with('=') => {
            let code = num(fields[0]);
            let (mods, event) = mods_event(fields.get(1));
            let modifier_only = code.is_some_and(|c| (57441..=57452).contains(&c));
            let m = mods.saturating_sub(1);
            Some((Key { code, ctrl: m & 4 != 0, alt: m & 2 != 0, noise: event != 1 || modifier_only }, len))
        }
        b'~' if fields.first() == Some(&"27") && fields.len() >= 3 => {
            let (mods, _) = mods_event(fields.get(1));
            let m = mods.saturating_sub(1);
            Some((Key { code: num(fields[2]), ctrl: m & 4 != 0, alt: m & 2 != 0, noise: false }, len))
        }
        _ => {
            // Arrows and other functional keys may carry an event type too: `CSI 1;1:3 A`.
            let (_, event) = mods_event(fields.get(1));
            Some((Key { code: None, ctrl: false, alt: false, noise: event != 1 }, len))
        }
    }
}

/// The prefix in any encoding: ctrl+letter as a control byte or a report, ctrl+space as NUL
/// (what a plain terminal sends for it) or a report.
pub fn is_prefix(k: &Key, prefix: desl_proto::config::Prefix) -> bool {
    if k.noise {
        return false;
    }
    (k.ctrl && k.code == Some(prefix.code)) || (prefix.code == 32 && k.code == Some(0))
}

/// A mouse report in SGR encoding (`CSI < b ; x ; y M|m`), which desl asks the terminal for.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Mouse {
    /// The button code: 0-2 buttons, 64/65 wheel up/down, +32 for motion, +4/8/16 modifiers.
    pub button: u16,
    /// Column and row on the whole terminal, from 0.
    pub x: u16,
    pub y: u16,
    /// False for a release (`m`).
    pub press: bool,
}

impl Mouse {
    pub fn wheel(&self) -> Option<bool> {
        match self.button & !(4 | 8 | 16) {
            64 => Some(true),
            65 => Some(false),
            _ => None,
        }
    }
    pub fn motion(&self) -> bool {
        self.button & 32 != 0
    }
    /// This report for a program whose screen starts at (`left`, `top`), in the encoding it asked
    /// for: SGR, or the original X10 bytes (which cannot say which button was released).
    pub fn encode(&self, left: u16, top: u16, sgr: bool) -> Vec<u8> {
        let (x, y) = (self.x.saturating_sub(left) + 1, self.y.saturating_sub(top) + 1);
        if sgr {
            return format!("\x1b[<{};{x};{y}{}", self.button, if self.press { 'M' } else { 'm' }).into_bytes();
        }
        let b = if self.press { self.button } else { 3 | (self.button & !3) };
        let byte = |n: u16| u8::try_from((n + 32).min(255)).unwrap_or(255);
        vec![0x1b, b'[', b'M', byte(b), byte(x), byte(y)]
    }
}

/// The mouse report at the start of `b` and its length.
pub fn mouse(b: &[u8]) -> Option<(Mouse, usize)> {
    let rest = b.strip_prefix(b"\x1b[<")?;
    let end = rest.iter().position(|c| *c == b'M' || *c == b'm')?;
    let mut it = std::str::from_utf8(&rest[..end]).ok()?.split(';').map(|n| n.parse::<u16>().ok());
    let (button, x, y) = (it.next()??, it.next()??, it.next()??);
    Some((Mouse { button, x: x.saturating_sub(1), y: y.saturating_sub(1), press: rest[end] == b'M' }, 3 + end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(b: &[u8]) -> Vec<Key> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some((k, n)) = next(&b[at..]) {
            out.push(k);
            at += n;
        }
        out
    }

    #[test]
    fn the_prefix_is_found_in_every_encoding() {
        use desl_proto::config::Prefix;
        let ctrl_b = Prefix::parse("ctrl+b").unwrap();
        for b in [&b"\x02"[..], b"\x1b[98;5u", b"\x1b[98;5:1u", b"\x1b[98:66;5u", b"\x1b[98;5:1;98u", b"\x1b[27;5;98~"] {
            let k = keys(b);
            assert!(is_prefix(&k[0], ctrl_b), "{:?}", String::from_utf8_lossy(b));
        }
        let space = Prefix::parse("ctrl+space").unwrap();
        for b in [&b"\x00"[..], b"\x1b[32;5u", b"\x1b[32;5:1u", b"\x1b[27;5;32~"] {
            let k = keys(b);
            assert!(is_prefix(&k[0], space), "{:?}", String::from_utf8_lossy(b));
            assert!(!is_prefix(&k[0], ctrl_b));
        }
        assert!(!is_prefix(&keys(b" ")[0], space), "a plain space is not the prefix");
        assert!(!is_prefix(&keys(b"\x1b[32;5:3u")[0], space), "nor its release");
    }

    #[test]
    fn releases_and_modifiers_alone_are_noise() {
        let k = keys(b"\x1b[57442;5u\x1b[98;5:3u\x1b[57442;1:3u\x1b[115u");
        assert!(k[0].noise && k[1].noise && k[2].noise);
        assert_eq!(k[3], Key { code: Some(115), ctrl: false, alt: false, noise: false });
    }

    #[test]
    fn alt_and_a_letter_is_found_in_every_encoding() {
        for b in [&b"\x1bj"[..], b"\x1b[106;3u", b"\x1b[106;3:1u", b"\x1b[27;3;106~"] {
            let k = keys(b);
            assert_eq!((k.len(), k[0].code, k[0].alt, k[0].ctrl), (1, Some(106), true, false), "{:?}", String::from_utf8_lossy(b));
        }
        assert!(!keys(b"j")[0].alt);
        assert!(keys(b"\x1b[106;3:3u")[0].noise, "a release is noise");
        assert!(!keys(b"\x1bOA")[0].alt, "an arrow in application mode is not alt+O");
        assert!(keys(b"\x1b[106;7u")[0].ctrl && keys(b"\x1b[106;7u")[0].alt, "ctrl+alt carries both");
    }

    #[test]
    fn plain_text_is_one_key_per_character() {
        let k = keys("sé".as_bytes());
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].code, Some(115));
    }

    #[test]
    fn mouse_reports_are_read_and_moved_into_a_pane() {
        let (m, n) = mouse(b"\x1b[<65;40;12Mrest").unwrap();
        assert_eq!(n, 12);
        assert_eq!(m, Mouse { button: 65, x: 39, y: 11, press: true });
        assert_eq!(m.wheel(), Some(false));
        assert_eq!(m.encode(30, 2, true), b"\x1b[<65;10;10M");
        assert_eq!(m.encode(30, 2, false), vec![0x1b, b'[', b'M', 65 + 32, 10 + 32, 10 + 32]);
        assert!(mouse(b"\x1b[<0;1").is_none());
    }
}
