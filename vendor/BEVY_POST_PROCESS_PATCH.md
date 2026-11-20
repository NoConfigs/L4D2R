# bevy_post_process 0.19.1 patch (L4D2 rewrite)

Copied from crates.io 0.19.1 and patched via `[patch.crates-io]` in Cargo.toml.

Change (src/auto_exposure): `AutoExposure` gains `readback: Option<Handle<ShaderBuffer>>`.
After the auto-exposure compute pass, the view's adapted exposure (EV, one f32) is
copied into that buffer, and the state buffer gets `COPY_SRC` usage. The game reads it
back with `Readback` (native_tonemap::install_exposure_readback), so the fog colour can
be divided by the exposure. Source blends fog after the tonemap scale, which was
verified against live retail (KNOWN_GAPS.md, 14 Nov).

Change (19 Nov): `AutoExposure` gains `initial_ev`, the adapted exposure a new
view's state buffer starts at (upstream always starts at EV 0).

Change (19 Nov): `AutoExposure` gains `min_average_luminance` and `max_ev`:
compute_average finds the histogram median and raises the target exposure so
the median reaches the floor (Source's mat_tonemap_min_avglum), capped at
`max_ev`.
