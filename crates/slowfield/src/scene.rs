mod flock;
mod gravity;
mod prime;

use slowfield_core::{FlockFieldSettings, GravityFieldSettings, SceneId};

/// Pixel dimensions supplied by the host after every window resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: i32,
    pub height: i32,
}

/// Monotonic timing data for one rendered frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub elapsed_seconds: f32,
}

/// A field renders into the OpenGL context that the Slowfield host already
/// owns. Fields never create a window, GL context, message loop, timer, or
/// process. This is intentionally narrow so each legacy renderer can migrate
/// without taking platform ownership back from the host.
pub trait Scene {
    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    unsafe fn activate(&mut self) -> Result<(), String>;

    fn resize(&mut self, viewport: Viewport);

    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    /// Returns whether the host should present the frame. A field may return
    /// false to apply its own bounded submission policy without accumulating a
    /// queue of expensive frames.
    unsafe fn render(&mut self, frame: Frame) -> bool;

    /// Called after drawing and immediately before the host presents. Fields
    /// with GPU fences insert their fence here, preserving the ordering between
    /// drawing and the host-owned buffer swap.
    ///
    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    unsafe fn prepare_present(&mut self, frame: Frame);

    /// Reports the result of a host-owned present. A field must release or
    /// synchronously complete any prepared GPU work if the present failed.
    ///
    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    unsafe fn presented(&mut self, frame: Frame, succeeded: bool);

    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    unsafe fn deactivate(&mut self);
}

pub fn available_scenes() -> &'static [SceneId] {
    &[SceneId::Prime, SceneId::Gravity, SceneId::Flock]
}

/// Creates one field from the host's one-per-launch random state. Fields may
/// derive their internal composition from it, but never own process-wide random
/// state themselves.
pub fn create_scene(
    scene: SceneId,
    preview: bool,
    launch_seed: u32,
    gravity_settings: GravityFieldSettings,
    flock_settings: FlockFieldSettings,
) -> Box<dyn Scene> {
    match scene {
        SceneId::Prime => Box::new(prime::PrimeScene::new(preview)),
        SceneId::Flock => Box::new(flock::FlockScene::new(preview, flock_settings)),
        SceneId::Gravity => Box::new(gravity::GravityScene::new(
            preview,
            launch_seed,
            gravity_settings,
        )),
    }
}
