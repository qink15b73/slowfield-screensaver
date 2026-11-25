mod flock;
mod prime;

use slowfield_core::SceneId;

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
    fn resize(&mut self, viewport: Viewport);

    /// # Safety
    ///
    /// The caller must make the host's OpenGL context current on this thread.
    unsafe fn render(&mut self, frame: Frame);
}

pub fn available_scenes() -> &'static [SceneId] {
    &[SceneId::Prime, SceneId::Flock]
}

pub fn create_scene(scene: SceneId, preview: bool) -> Box<dyn Scene> {
    match scene {
        SceneId::Prime => Box::new(prime::PrimeScene::new(preview)),
        SceneId::Flock => Box::new(flock::FlockScene::new(preview)),
        // Gravity Field is not advertised until its port supplies a scene.
        SceneId::Gravity => unreachable!("unavailable scene selected"),
    }
}
