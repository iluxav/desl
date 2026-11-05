//! Base64 (standard alphabet, padded) for the raw terminal bytes the `pty` frames carry inside
//! JSON. Small enough not to need a crate.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// None when `text` is not base64.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let t = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(t.len() * 3 / 4);
    for chunk in t.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * k);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_length() {
        let data: Vec<u8> = (0..=255u8).collect();
        for n in 0..40 {
            assert_eq!(decode(&encode(&data[..n])).unwrap(), data[..n]);
        }
        assert_eq!(encode(b"hi\x1b["), "aGkbWw==");
        assert!(decode("a").is_none());
    }
}
