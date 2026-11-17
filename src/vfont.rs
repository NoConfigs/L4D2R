//! Bounded VFONT1 envelope decoding. Font payloads stay in memory.
use crate::binary::invalid;
pub fn decode(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    if bytes.len() < 7 || !bytes.ends_with(b"VFONT1") {
        return Err(invalid("Expected VFONT1"));
    }
    let trailer = bytes.len() - 6;
    let salt = bytes[trailer - 1] as usize;
    if salt == 0 || salt > trailer {
        return Err(invalid("Invalid VFONT salt length"));
    }
    let payload = trailer - salt;
    let mut key = 167u8;
    for byte in &bytes[payload..trailer - 1] {
        key ^= byte.wrapping_add(167);
    }
    let output = bytes[..payload]
        .iter()
        .map(|byte| {
            let decoded = *byte ^ key;
            key = byte.wrapping_add(167);
            decoded
        })
        .collect::<Vec<_>>();
    if !output.starts_with(&[0, 1, 0, 0])
        && !output.starts_with(b"OTTO")
        && !output.starts_with(b"ttcf")
    {
        return Err(invalid("VFONT payload is not SFNT"));
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_known_sfnt_prefix() {
        assert_eq!(
            decode(&[167, 79, 246, 157, 1, b'V', b'F', b'O', b'N', b'T', b'1']).unwrap(),
            [0, 1, 0, 0]
        );
    }
    #[test]
    fn malformed_envelopes_are_rejected() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"\0VFONT1").is_err());
        assert!(decode(b"\xffVFONT1").is_err());
    }
}
