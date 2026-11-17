//! Partial `ShaderBuffer` uploads. Replacing a `ShaderBuffer`'s data makes
//! Bevy re-upload (and, for the asset path, clone) the whole buffer; per-frame
//! tables only need their live prefix written. Main-world systems queue
//! writes; the render world moves them out during extract and applies them
//! after the GPU buffers are prepared.
use bevy::{
    prelude::*,
    render::{
        MainWorld, Render, RenderApp, RenderSystems,
        render_asset::RenderAssets,
        renderer::RenderQueue,
        storage::{GpuShaderBuffer, ShaderBuffer},
    },
};

/// Queued (buffer, byte offset, bytes, frames waited) writes.
#[derive(Resource, Default)]
pub struct GpuWrites(Vec<(AssetId<ShaderBuffer>, u64, Vec<u8>, u8)>);
impl GpuWrites {
    pub fn write(&mut self, buffer: AssetId<ShaderBuffer>, offset: u64, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.0.push((buffer, offset, bytes, 0));
        }
    }
}

/// Safe to add from several installers; only the first registers systems.
pub struct GpuWritesPlugin;
impl Plugin for GpuWritesPlugin {
    fn build(&self, app: &mut App) {
        install(app);
    }
    fn is_unique(&self) -> bool {
        false
    }
}
fn install(app: &mut App) {
    if app.world().contains_resource::<GpuWrites>() {
        return;
    }
    app.init_resource::<GpuWrites>();
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .init_resource::<GpuWrites>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(Render, apply.in_set(RenderSystems::PrepareResources));
    }
}
fn extract(mut main: ResMut<MainWorld>, mut pending: ResMut<GpuWrites>) {
    if let Some(mut queued) = main.get_resource_mut::<GpuWrites>()
        && !queued.0.is_empty()
    {
        pending.0.append(&mut queued.0);
    }
}
fn apply(
    mut pending: ResMut<GpuWrites>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
) {
    let _timing = crate::performance::scope("gpu_writes::apply");
    // A buffer created this frame is prepared before this set; anything still
    // missing waits a few frames, then is dropped (its buffer is gone).
    pending.0.retain_mut(|(id, offset, bytes, waited)| {
        let Some(gpu) = buffers.get(*id) else {
            *waited += 1;
            return *waited < 8;
        };
        let end = *offset + bytes.len() as u64;
        if end <= gpu.buffer.size() {
            queue.write_buffer(&gpu.buffer, *offset, bytes);
        }
        false
    });
}
