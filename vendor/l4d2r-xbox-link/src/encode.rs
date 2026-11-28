//! Entity encoding for hosting: the exact inverse of entities.rs decoding
//! (360 SendProp value formats, prop index scheme, PacketEntities headers).
use crate::bits::BitWriter;
use crate::entities::flags::*;
use crate::entities::{Entity, Prop, PropType, Schema, Value};

fn ubits(w: &mut BitWriter, n: u32, v: u64) {
    if n > 0 {
        let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
        w.u(n as usize, v & mask);
    }
}

fn bit_coord(w: &mut BitWriter, v: f32) {
    let a = v.abs();
    let mut int = a.trunc() as u32;
    let mut fract = ((a - int as f32) * 32.0).round() as u32;
    if fract == 32 {
        int += 1;
        fract = 0;
    }
    w.bit(int != 0);
    w.bit(fract != 0);
    if int == 0 && fract == 0 {
        return;
    }
    w.bit(v < 0.0);
    if int != 0 {
        w.u(14, (int - 1) as u64);
    }
    if fract != 0 {
        w.u(5, fract as u64);
    }
}

fn coord_mp(w: &mut BitWriter, v: f32, integral: bool, low: bool) {
    let a = v.abs();
    let (scale, fbits) = if low { (8.0, 3) } else { (32.0, 5) };
    let mut int = if integral { a.round() as u32 } else { a.trunc() as u32 };
    let mut fract = if integral { 0 } else { ((a - int as f32) * scale).round() as u32 };
    if fract as f32 >= scale {
        int += 1;
        fract = 0;
    }
    let in_bounds = int < (1 << 11);
    w.bit(in_bounds);
    let ibits = if in_bounds { 11 } else { 14 };
    if integral {
        w.bit(int != 0);
        if int != 0 {
            w.bit(v < 0.0);
            w.u(ibits, (int - 1) as u64);
        }
    } else {
        w.bit(int != 0);
        w.bit(v < 0.0);
        if int != 0 {
            w.u(ibits, (int - 1) as u64);
        }
        w.u(fbits, fract as u64);
    }
}

fn cell_coord(w: &mut BitWriter, v: f32, bits: u32, integral: bool, low: bool) {
    let a = v.max(0.0);
    if integral {
        ubits(w, bits, a.round() as u64);
        return;
    }
    let (scale, fbits) = if low { (8.0, 3) } else { (32.0, 5) };
    let mut int = a.trunc() as u64;
    let mut fract = ((a - int as f32) * scale).round() as u64;
    if fract as f32 >= scale {
        int += 1;
        fract = 0;
    }
    ubits(w, bits, int);
    w.u(fbits, fract);
}

fn encode_float(w: &mut BitWriter, p: &Prop, v: f32) {
    let fl = p.flags;
    if fl & COORD != 0 {
        bit_coord(w, v)
    } else if fl & COORD_MP != 0 {
        coord_mp(w, v, false, false)
    } else if fl & COORD_MP_LOWPRECISION != 0 {
        coord_mp(w, v, false, true)
    } else if fl & COORD_MP_INTEGRAL != 0 {
        coord_mp(w, v, true, false)
    } else if fl & NOSCALE != 0 {
        w.u(32, v.to_bits() as u64)
    } else if fl & NORMAL != 0 {
        w.bit(v < 0.0);
        w.u(11, (v.abs().min(1.0) * ((1 << 11) - 1) as f32).round() as u64);
    } else if fl & CELL_COORD != 0 {
        cell_coord(w, v, p.bits, false, false)
    } else if fl & CELL_COORD_LOWPRECISION != 0 {
        cell_coord(w, v, p.bits, false, true)
    } else if fl & CELL_COORD_INTEGRAL != 0 {
        cell_coord(w, v, p.bits, true, false)
    } else {
        let max = ((1u64 << p.bits) - 1) as f32;
        let t = if p.high != p.low { ((v - p.low) / (p.high - p.low)).clamp(0.0, 1.0) } else { 0.0 };
        w.u(p.bits as usize, (t * max).round() as u64);
    }
}

/// Inverse of `entities::decode_value`.
pub fn encode_value(w: &mut BitWriter, p: &Prop, v: &Value) -> Option<()> {
    match (p.ty, v) {
        (PropType::Int, Value::Int(i)) => ubits(w, p.bits, *i as u64),
        (PropType::Float, Value::Float(f)) => encode_float(w, p, *f),
        (PropType::Float, Value::Int(i)) => encode_float(w, p, *i as f32),
        (PropType::Vector, Value::Vector([x, y, z])) => {
            encode_float(w, p, *x);
            encode_float(w, p, *y);
            if p.flags & NORMAL != 0 {
                w.bit(*z < 0.0);
            } else {
                encode_float(w, p, *z);
            }
        }
        (PropType::VectorXY, Value::VectorXY([x, y])) => {
            encode_float(w, p, *x);
            encode_float(w, p, *y);
        }
        (PropType::String, Value::String(s)) => {
            let b = &s.as_bytes()[..s.len().min(511)];
            w.u(9, b.len() as u64);
            w.bytes(b);
        }
        (PropType::Array, Value::Array(items)) => {
            let el = p.element.as_ref()?;
            let count_bits = 32 - p.elements.leading_zeros();
            ubits(w, count_bits, items.len() as u64);
            for item in items {
                encode_value(w, el, item)?;
            }
        }
        (PropType::Int64, Value::Int64(v)) => {
            w.u(32, v & 0xFFFF_FFFF);
            ubits(w, p.bits.saturating_sub(32), v >> 32);
        }
        _ => return None,
    }
    Some(())
}

/// Inverse of entities::next_index (new scheme). `ret` = index - last - 1.
fn write_index_gap(w: &mut BitWriter, ret: u64) {
    if ret == 0 {
        w.bit(true);
        return;
    }
    w.bit(false);
    if ret < 8 {
        w.bit(true);
        w.u(3, ret);
        return;
    }
    w.bit(false);
    let low = ret & 31;
    let hi = ret >> 5;
    if hi == 0 {
        w.u(7, ret);
    } else if hi < 4 {
        w.u(7, low | 32);
        w.u(2, hi);
    } else if hi < 16 {
        w.u(7, low | 64);
        w.u(4, hi);
    } else {
        w.u(7, low | 96);
        w.u(7, hi);
    }
}

/// Interleaved index/value prop list (inverse of entities::read_props).
pub fn write_props(w: &mut BitWriter, flat: &[Prop], props: &[(usize, &Value)]) -> Option<()> {
    w.bit(true); // new index scheme
    let mut last: i64 = -1;
    for (i, v) in props {
        write_index_gap(w, (*i as i64 - last - 1) as u64);
        encode_value(w, flat.get(*i)?, v)?;
        last = *i as i64;
    }
    // Terminator: index 0xFFF.
    w.bit(false);
    w.bit(false);
    w.u(7, 31 | 96);
    w.u(7, 0xFFF >> 5);
    Some(())
}

/// Inverse of entities::read_ubitvar.
pub fn write_ubitvar(w: &mut BitWriter, v: u32) {
    let v = v as u64;
    if v < 16 {
        w.u(6, v);
    } else if v < 256 {
        w.u(6, (v & 15) | 16);
        w.u(4, v >> 4);
    } else if v < 4096 {
        w.u(6, (v & 15) | 32);
        w.u(8, v >> 4);
    } else {
        w.u(6, (v & 15) | 48);
        w.u(28, v >> 4);
    }
}

/// A full (non-delta) PacketEntities body: every entity enters with all its
/// props. Returns (data, bits, updated count).
pub fn full_update(schema: &Schema, entities: &[(u32, &Entity)]) -> Option<(Vec<u8>, usize, u32)> {
    let mut sorted: Vec<&(u32, &Entity)> = entities.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    let mut w = BitWriter::new();
    let mut last: i64 = -1;
    let class_bits = schema.class_bits();
    for (id, e) in &sorted {
        write_ubitvar(&mut w, (*id as i64 - last - 1) as u32);
        last = *id as i64;
        w.bit(false); // not leaving
        w.bit(true); // enters
        w.u(class_bits, e.class as u64);
        w.u(10, e.serial as u64);
        let flat = schema.flat(e.class)?;
        let mut props: Vec<(usize, &Value)> = e.props.iter().map(|(i, v)| (*i, v)).collect();
        props.sort_by_key(|(i, _)| *i);
        write_props(&mut w, flat, &props)?;
    }
    Some((w.data.clone(), w.bits, sorted.len() as u32))
}

/// A delta PacketEntities body from frame `from` to frame `to` (inverse of
/// World::apply with delta_from): entering entities carry all props, changed
/// ones only their changed props, gone ones are deleted. Returns (data, bits,
/// updated count).
pub fn delta_update(
    schema: &Schema,
    from: &std::collections::HashMap<u32, Entity>,
    to: &std::collections::HashMap<u32, Entity>,
) -> Option<(Vec<u8>, usize, u32)> {
    let mut ids: Vec<u32> = from.keys().chain(to.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut w = BitWriter::new();
    let mut last: i64 = -1;
    let mut updated = 0u32;
    let class_bits = schema.class_bits();
    for id in ids {
        let (old, new) = (from.get(&id), to.get(&id));
        let header = |w: &mut BitWriter, last: &mut i64| {
            write_ubitvar(w, (id as i64 - *last - 1) as u32);
            *last = id as i64;
        };
        match (old, new) {
            // Same entity, and no prop dropped (a delta can only add or change props;
            // a restarted prop set means it re-entered).
            (Some(o), Some(n)) if o.class == n.class && o.serial == n.serial && o.props.keys().all(|k| n.props.contains_key(k)) => {
                let mut changed: Vec<(usize, &Value)> =
                    n.props.iter().filter(|(k, v)| o.props.get(k) != Some(v)).map(|(k, v)| (*k, v)).collect();
                if changed.is_empty() {
                    continue;
                }
                changed.sort_by_key(|(k, _)| *k);
                header(&mut w, &mut last);
                w.bit(false);
                w.bit(false);
                write_props(&mut w, schema.flat(n.class)?, &changed)?;
            }
            (_, Some(n)) => {
                header(&mut w, &mut last);
                w.bit(false);
                w.bit(true);
                w.u(class_bits, n.class as u64);
                w.u(10, n.serial as u64);
                let mut props: Vec<(usize, &Value)> = n.props.iter().map(|(k, v)| (*k, v)).collect();
                props.sort_by_key(|(k, _)| *k);
                write_props(&mut w, schema.flat(n.class)?, &props)?;
            }
            (Some(_), None) => {
                header(&mut w, &mut last);
                w.bit(true); // leaves PVS
                w.bit(true); // and is deleted
            }
            (None, None) => continue,
        }
        updated += 1;
    }
    w.bit(false); // no explicit deletions follow
    Some((w.data.clone(), w.bits, updated))
}
