//! Recovered100ba0d0 SSE estimate angle, shared by numeric and local solvers.
pub(crate) fn native_atan2(y: f32, x: f32) -> f32 {
    let ay = y.abs(); let ax = x.abs();
    let swap = ax < ay;
    let (high, low) = if swap { (ay, ax) } else { (ax, ay) };
    let valid = high > f32::EPSILON;
    let reciprocal = crate::ivp_joint_angular::reciprocal;
    let ratio = f32::from_bits((reciprocal(high) * low).to_bits()
        & if valid { u32::MAX } else { 0 });
    let centered = reciprocal(ay + ax) * (ay - ax);
    let use_center = centered.abs() < ratio && valid;
    let t = if use_center { centered } else { ratio };
    let square = t * t;
    let polynomial = ((f32::from_bits(0x3e29e496) * square + f32::from_bits(0xbea95fa8))
        * square + f32::from_bits(0x3f7ffced)) * t;
    let angle = if use_center { polynomial + std::f32::consts::FRAC_PI_4 }
        else if swap { std::f32::consts::FRAC_PI_2 - polynomial } else { polynomial };
    let angle = f32::from_bits(angle.to_bits() ^ ((y.to_bits() ^ x.to_bits()) & 0x80000000));
    // ADDPS executes even when x >= 0: adding +0 canonicalizes signed zero.
    angle + if x < 0.0 { f32::from_bits((y.to_bits() & 0x80000000) | 0x40490fdb) } else { 0.0 }
}
