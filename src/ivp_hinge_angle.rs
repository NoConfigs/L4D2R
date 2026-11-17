//! Installed 100b7b4a..100b7bad x87 dot/FPATAN sequence. Native controls
//! report control word 0x027f (round-to-nearest, 53-bit significand).
pub(crate) fn coordinate(perpendicular: [f32; 3], second: [f32; 3], attached: [f32; 3]) -> Result<f32, &'static str> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let vectors = [perpendicular, second, attached];
        let mut saved = 0u16;
        let control = 0x027fu16;
        let mut angle = 0f32;
        // Register-only x87 opcodes retain the installed operand directions.
        // In particular D8 C9 and DC C9 are different FMUL destinations;
        // the disassembler prints both as "FMUL ST1".
        // Save/restore caller precision; leave the x87 stack balanced.
        unsafe { std::arch::asm!(
            "fnstcw word ptr [{saved}]",
            "fldcw word ptr [{control}]",
            "fld dword ptr [{v} + 28]",
            "fld dword ptr [{v} + 16]",
            ".byte 0xd8, 0xc9",
            "fld dword ptr [{v} + 12]",
            "fld dword ptr [{v} + 24]",
            ".byte 0xdc, 0xc9",
            ".byte 0xd9, 0xca",
            ".byte 0xde, 0xc1",
            "fld dword ptr [{v} + 32]",
            "fld dword ptr [{v} + 20]",
            ".byte 0xd8, 0xc9",
            ".byte 0xde, 0xc2",
            "fld dword ptr [{v} + 4]",
            ".byte 0xde, 0xcc",
            "fld dword ptr [{v}]",
            ".byte 0xde, 0xcb",
            ".byte 0xd9, 0xcb",
            ".byte 0xde, 0xc2",
            "fld dword ptr [{v} + 8]",
            ".byte 0xde, 0xcb",
            ".byte 0xd9, 0xc9",
            ".byte 0xde, 0xc2",
            ".byte 0xd9, 0xc9",
            "fpatan",
            "fstp dword ptr [{angle}]",
            "fldcw word ptr [{saved}]",
            saved = in(reg) &mut saved, control = in(reg) &control,
            v = in(reg) vectors.as_ptr(), angle = in(reg) &mut angle,
            options(nostack, preserves_flags),
        ); }
        Ok(angle)
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    { let _ = (perpendicular, second, attached); Err("native x87 hinge angle backend unavailable") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn native_x87_hinge_angles_match_and_restore_caller_control_word() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-callback.json")).unwrap();
        let mut total = 0;
        for call in data["calls"].as_array().unwrap() {
            let a = &call["angle_data"]; if a.is_null() { continue; }
            assert_eq!(a["control"], 0x027f);
            let vectors: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| f32::from_bits(a["vectors"][i * 3 + j].as_u64().unwrap() as u32)));
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            unsafe {
                let mut old = 0u16; let mut restored = 0u16;
                std::arch::asm!("fnstcw word ptr [{p}]", p=in(reg) &mut old, options(nostack, preserves_flags));
                // Different precision from the native sequence exercises restoration.
                let changed = (old & !0x0300) | 0x0300;
                std::arch::asm!("fldcw word ptr [{p}]", p=in(reg) &changed, options(nostack, preserves_flags));
                let angle = coordinate(vectors[1], vectors[0], vectors[2]).unwrap();
                std::arch::asm!("fnstcw word ptr [{p}]", p=in(reg) &mut restored, options(nostack, preserves_flags));
                std::arch::asm!("fldcw word ptr [{p}]", p=in(reg) &old, options(nostack, preserves_flags));
                assert_eq!(restored, changed);
                assert_eq!(angle.to_bits() as u64, a["angle"].as_u64().unwrap());
            }
            total += 1;
        }
        assert_eq!(total, 168);
    }
}
