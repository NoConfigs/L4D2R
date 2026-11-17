use std::io;
pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
pub fn range(bytes: &[u8], offset: usize, size: usize) -> io::Result<&[u8]> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(size)
                    .ok_or_else(|| invalid("Binary range overflow"))?,
        )
        .ok_or_else(|| {
            invalid(format!(
                "Binary range {offset}+{size} outside {} bytes",
                bytes.len()
            ))
        })
}
pub fn u32_at(b: &[u8], o: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(range(b, o, 4)?.try_into().unwrap()))
}
pub fn u16_at(b: &[u8], o: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(range(b, o, 2)?.try_into().unwrap()))
}
pub fn f32_at(b: &[u8], o: usize) -> io::Result<f32> {
    let v = f32::from_bits(u32_at(b, o)?);
    if v.is_finite() {
        Ok(v)
    } else {
        Err(invalid("Non-finite binary float"))
    }
}
pub fn vec3(b: &[u8], o: usize) -> io::Result<[f32; 3]> {
    Ok([f32_at(b, o)?, f32_at(b, o + 4)?, f32_at(b, o + 8)?])
}
pub fn count(b: &[u8], o: usize, max: usize) -> io::Result<usize> {
    let n = u32_at(b, o)? as usize;
    if n > max {
        Err(invalid(format!("Invalid count {n}, maximum {max}")))
    } else {
        Ok(n)
    }
}
pub fn relative(b: &[u8], base: usize, field: usize) -> io::Result<usize> {
    let delta = u32_at(b, base + field)? as i32;
    base.checked_add_signed(delta as isize)
        .ok_or_else(|| invalid("Relative offset overflow"))
}
pub fn cstring(b: &[u8], o: usize) -> io::Result<String> {
    let tail = b
        .get(o..)
        .ok_or_else(|| invalid("String offset outside binary"))?;
    let n = tail
        .iter()
        .position(|c| *c == 0)
        .ok_or_else(|| invalid("Unterminated binary string"))?;
    Ok(String::from_utf8_lossy(&tail[..n]).into_owned())
}
pub fn source_position(p: [f32; 3]) -> [f32; 3] {
    [p[0] * 0.0254, p[2] * 0.0254, -p[1] * 0.0254]
}
pub fn source_normal(n: [f32; 3]) -> [f32; 3] {
    [n[0], n[2], -n[1]]
}
