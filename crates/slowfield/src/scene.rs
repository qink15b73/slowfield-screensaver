use slowfield_core::SceneId;
use windows_sys::Win32::Graphics::OpenGL::{GL_COLOR_BUFFER_BIT, glClear, glClearColor};

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
    // Prime Field is the first field to move behind the shared host. The
    // diagnostic implementation below occupies that selection until its visual
    // renderer lands, so random selection already has the final policy shape.
    &[SceneId::Prime]
}

pub fn create_scene(scene: SceneId) -> Box<dyn Scene> {
    Box::new(DiagnosticScene::new(scene))
}

struct DiagnosticScene {
    scene: SceneId,
    viewport: Viewport,
}

impl DiagnosticScene {
    fn new(scene: SceneId) -> Self {
        Self {
            scene,
            viewport: Viewport {
                width: 1,
                height: 1,
            },
        }
    }
}

impl Scene for DiagnosticScene {
    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    unsafe fn render(&mut self, frame: Frame) {
        let motion = 0.5 + 0.5 * (frame.elapsed_seconds * 0.45).sin();
        let aspect = self.viewport.width as f32 / self.viewport.height.max(1) as f32;
        let (red, green, blue) = match self.scene {
            SceneId::Prime => (
                0.008 + motion * 0.010,
                0.040 + aspect * 0.002,
                0.090 + motion * 0.025,
            ),
            SceneId::Gravity => (0.012 + motion * 0.015, 0.004, 0.020 + motion * 0.010),
            SceneId::Flock => (0.002, 0.025 + motion * 0.020, 0.030 + motion * 0.020),
        };
        unsafe {
            glClearColor(red, green, blue, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
        }
    }
}
