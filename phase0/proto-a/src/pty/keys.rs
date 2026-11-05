//! crossterm key events to the bytes a terminal would send. Only what the agents' TUIs need.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn modifier_param(m: KeyModifiers) -> u8 {
    let mut n = 1;
    if m.contains(KeyModifiers::SHIFT) {
        n += 1;
    }
    if m.contains(KeyModifiers::ALT) {
        n += 2;
    }
    if m.contains(KeyModifiers::CONTROL) {
        n += 4;
    }
    n
}

fn csi(final_byte: char, m: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    let p = modifier_param(m);
    if p == 1 {
        if app_cursor && matches!(final_byte, 'A' | 'B' | 'C' | 'D' | 'H' | 'F') {
            return format!("\x1bO{final_byte}").into_bytes();
        }
        return format!("\x1b[{final_byte}").into_bytes();
    }
    format!("\x1b[1;{p}{final_byte}").into_bytes()
}

fn tilde(n: u8, m: KeyModifiers) -> Vec<u8> {
    let p = modifier_param(m);
    if p == 1 { format!("\x1b[{n}~").into_bytes() } else { format!("\x1b[{n};{p}~").into_bytes() }
}

pub fn encode(k: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = k.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let mut out: Vec<u8> = Vec::new();
    match k.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    ' ' | '@' | '2' => 0x00,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '/' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    c if c.is_ascii_lowercase() => (c as u8) & 0x1f,
                    _ => return None,
                };
                out.push(b);
            } else {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(0x7f);
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out = csi('A', m, app_cursor),
        KeyCode::Down => out = csi('B', m, app_cursor),
        KeyCode::Right => out = csi('C', m, app_cursor),
        KeyCode::Left => out = csi('D', m, app_cursor),
        KeyCode::Home => out = csi('H', m, app_cursor),
        KeyCode::End => out = csi('F', m, app_cursor),
        KeyCode::Insert => out = tilde(2, m),
        KeyCode::Delete => out = tilde(3, m),
        KeyCode::PageUp => out = tilde(5, m),
        KeyCode::PageDown => out = tilde(6, m),
        KeyCode::F(n) => {
            out = match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => tilde(15, m),
                6 => tilde(17, m),
                7 => tilde(18, m),
                8 => tilde(19, m),
                9 => tilde(20, m),
                10 => tilde(21, m),
                11 => tilde(23, m),
                12 => tilde(24, m),
                _ => return None,
            }
        }
        _ => return None,
    }
    Some(out)
}

pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace('\n', "\r");
    if bracketed { format!("\x1b[200~{body}\x1b[201~").into_bytes() } else { body.into_bytes() }
}
