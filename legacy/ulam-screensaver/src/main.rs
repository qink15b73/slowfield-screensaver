#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ulam;

use std::cell::RefCell;
use std::f32::consts::TAU;
use std::ffi::CStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetDC, HDC, ReleaseDC, UpdateWindow};
use windows_sys::Win32::Graphics::OpenGL::*;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ulam::{UlamPoint, generate_points};

const WINDOW_CLASS: &std::ffi::CStr = c"UlamPrimeFieldScreensaver";
const WINDOW_TITLE: &std::ffi::CStr = c"Ulam Prime Field";
const FRAME_TIMER_ID: usize = 1;
const FRAME_INTERVAL_MS: u32 = 4;
/// One scene grows from the central 1 out to a bounded square, pauses briefly,
/// then starts a new composition. Keeping this finite is deliberate: a saver
/// must not eventually consume all CPU/GPU time or create microscopic points.
/// A slow reveal makes individual primes perceptible rather than looking like
/// a filled-in time lapse.
const GROWTH_DURATION: Duration = Duration::from_secs(240);
const HOLD_DURATION: Duration = Duration::from_secs(7);
const NEW_POINT_GLOW_SECONDS: f32 = 0.85;
/// Initial framing floor only. The sqrt(n)/2 camera would start below one
/// cell, magnifying the first dots to hundreds of pixels. This constant keeps
/// the opening view stable; once sqrt(n)/2 exceeds it, the pure law takes over
/// with no further clamping or per-ring correction.
const MIN_ZOOM_RADIUS: f32 = 9.0;
/// The field occupies this half-extent of the smaller viewport dimension.
/// The remainder is a deliberate narrow safety band: point glow and the
/// outermost ring stay clear of the monitor's top and bottom edges.
const CANVAS_HALF_EXTENT: f32 = 0.42;
const ADJUSTMENT_WIDTH: i32 = 1120;
const ADJUSTMENT_HEIGHT: i32 = 760;

#[derive(Clone, Copy)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
}

impl Color {
    const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    fn scaled(self, amount: f32) -> Self {
        Self::new(self.r * amount, self.g * amount, self.b * amount)
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        Self::new(
            self.r + (other.r - self.r) * amount,
            self.g + (other.g - self.g) * amount,
            self.b + (other.b - self.b) * amount,
        )
    }
}

#[derive(Clone, Copy)]
struct Palette {
    shadow: Color,
    deep: Color,
    body: Color,
    accent: Color,
}

const PALETTES: [Palette; 4] = [
    Palette {
        shadow: Color::new(0.003, 0.006, 0.018),
        deep: Color::new(0.012, 0.036, 0.086),
        body: Color::new(0.08, 0.66, 0.94),
        accent: Color::new(0.96, 0.34, 0.68),
    },
    Palette {
        shadow: Color::new(0.012, 0.004, 0.020),
        deep: Color::new(0.074, 0.016, 0.106),
        body: Color::new(0.68, 0.24, 0.96),
        accent: Color::new(1.00, 0.68, 0.22),
    },
    Palette {
        shadow: Color::new(0.002, 0.014, 0.016),
        deep: Color::new(0.008, 0.080, 0.074),
        body: Color::new(0.12, 0.91, 0.66),
        accent: Color::new(0.84, 0.98, 0.49),
    },
    Palette {
        shadow: Color::new(0.018, 0.007, 0.004),
        deep: Color::new(0.100, 0.026, 0.010),
        body: Color::new(1.00, 0.37, 0.13),
        accent: Color::new(1.00, 0.88, 0.58),
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Fullscreen,
    Preview,
    Adjustment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldShape {
    Square,
    Circle,
}

#[derive(Clone, Copy)]
struct PrimeFrame {
    palette: Palette,
    center: (f32, f32),
    scale: f32,
    angle: f32,
    time: f32,
    lit_number: u32,
    field_radius: f32,
}

impl FieldShape {
    fn from_seed(seed: u32) -> Self {
        if seed & 1 == 0 {
            Self::Square
        } else {
            Self::Circle
        }
    }
}

struct App {
    window: HWND,
    device_context: HDC,
    render_context: HGLRC,
    width: i32,
    height: i32,
    mode: RunMode,
    exit_on_input: bool,
    initial_mouse: POINT,
    launch: Instant,
    scene_started: Instant,
    scene_index: usize,
    scene_seed: u32,
    shape: FieldShape,
    maximum_side: u32,
    points: Vec<UlamPoint>,
}

impl App {
    unsafe fn create(window: HWND, mode: RunMode) -> Result<Self, &'static CStr> {
        let device_context = unsafe { GetDC(window) };
        if device_context.is_null() {
            return Err(c"Could not acquire a drawing surface.");
        }

        let descriptor = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cDepthBits: 24,
            iLayerType: PFD_MAIN_PLANE as u8,
            ..Default::default()
        };
        let format = unsafe { ChoosePixelFormat(device_context, &descriptor) };
        if format == 0 || unsafe { SetPixelFormat(device_context, format, &descriptor) } == 0 {
            unsafe { ReleaseDC(window, device_context) };
            return Err(c"Could not configure an OpenGL pixel format.");
        }

        let render_context = unsafe { wglCreateContext(device_context) };
        if render_context.is_null()
            || unsafe { wglMakeCurrent(device_context, render_context) } == 0
        {
            if !render_context.is_null() {
                unsafe { wglDeleteContext(render_context) };
            }
            unsafe { ReleaseDC(window, device_context) };
            return Err(c"Could not create an OpenGL rendering context.");
        }

        unsafe {
            glDisable(GL_DEPTH_TEST);
            glDisable(GL_CULL_FACE);
            glDisable(GL_POINT_SMOOTH);
        }

        let mut rect = RECT::default();
        unsafe { GetClientRect(window, &mut rect) };
        let mut initial_mouse = POINT::default();
        unsafe { GetCursorPos(&mut initial_mouse) };
        let now = Instant::now();
        let scene_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u32)
            ^ (window as usize as u32).rotate_left(13);
        let mut app = Self {
            window,
            device_context,
            render_context,
            width: (rect.right - rect.left).max(1),
            height: (rect.bottom - rect.top).max(1),
            mode,
            exit_on_input: mode == RunMode::Fullscreen,
            initial_mouse,
            launch: now,
            scene_started: now,
            scene_index: 0,
            scene_seed,
            shape: FieldShape::Square,
            maximum_side: 451,
            points: Vec::new(),
        };
        app.rebuild_scene();
        Ok(app)
    }

    unsafe fn destroy(&mut self) {
        unsafe { wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) };
        if !self.render_context.is_null() {
            unsafe { wglDeleteContext(self.render_context) };
            self.render_context = std::ptr::null_mut();
        }
        if !self.device_context.is_null() {
            unsafe { ReleaseDC(self.window, self.device_context) };
            self.device_context = std::ptr::null_mut();
        }
    }

    fn rebuild_scene(&mut self) {
        // The largest grid is deliberately capped. At 501×501 the scene has
        // only ~22k visible primes, a bounded and comfortable amount for the
        // legacy OpenGL point renderer even on an integrated GPU.
        const FULL_FIELD_SIDES: [u32; 4] = [251, 301, 351, 401];
        self.maximum_side = if self.mode == RunMode::Preview {
            151
        } else {
            FULL_FIELD_SIDES[self.scene_index % FULL_FIELD_SIDES.len()]
        };
        self.shape =
            FieldShape::from_seed(self.scene_seed ^ (self.scene_index as u32).rotate_left(7));
        self.points = generate_points(self.maximum_side, self.scene_seed);
    }

    unsafe fn resize(&mut self) {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.window, &mut rect) };
        self.width = (rect.right - rect.left).max(1);
        self.height = (rect.bottom - rect.top).max(1);
    }

    unsafe fn render(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.scene_started) >= GROWTH_DURATION + HOLD_DURATION
            && self.mode != RunMode::Preview
        {
            self.scene_started = now;
            self.scene_index = (self.scene_index + 1) % PALETTES.len();
            self.scene_seed = self
                .scene_seed
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            self.rebuild_scene();
        }

        let time = now.duration_since(self.launch).as_secs_f32();
        let scene_time = now.duration_since(self.scene_started).as_secs_f32();
        let palette = PALETTES[self.scene_index % PALETTES.len()];
        let width = self.width as f32;
        let height = self.height as f32;
        let center = (width * 0.5, height * 0.5);
        // The reveal cursor is the natural number itself, advancing linearly
        // with time. Prime visibility uses its integer part; the geometry below
        // uses the continuous value so no step can enter the zoom.
        let revealed_number = self.revealed_number(scene_time);
        let lit_number = (revealed_number.floor() as u32).max(2);
        // Ulam's own law: an odd square of side s contains exactly s² natural
        // numbers, so after n numbers the square's side is sqrt(n) and its
        // half-extent is sqrt(n)/2. Anchoring that half-extent to the fixed
        // canvas radius gives scale = canvas / (sqrt(n)/2). This single
        // monotone curve is the whole zoom rule: no prime-count term, no
        // per-ring target, no smoothing chase and no boundary clamp.
        let canvas_radius = width.min(height) * CANVAS_HALF_EXTENT;
        // Circle mode uses the incircle of that same square: r = s/2 =
        // sqrt(n)/2, so its area is pi*n/4 — a constant 78.5% of the revealed
        // square. This is the circle that shares the square's reveal front.
        // Making the disk area equal n (r = sqrt(n/pi)) would put its radius
        // 12.8% beyond the square's half-side, so its caps would run past the
        // generated lattice and the boundary would no longer be a clean disk.
        let field_radius = (revealed_number.sqrt() * 0.5).max(1.0);
        let camera_radius = field_radius.max(MIN_ZOOM_RADIUS);
        let scale = canvas_radius / camera_radius;
        // Keep the mathematical Ulam lattice fixed on screen. The colour,
        // bloom, and scale can evolve, but rotating the grid obscures the
        // persistent prime diagonals the saver is meant to reveal.
        let angle = 0.0;

        unsafe {
            glViewport(0, 0, self.width, self.height);
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();
            glOrtho(0.0, width as f64, height as f64, 0.0, -1.0, 1.0);
            glMatrixMode(GL_MODELVIEW);
            glLoadIdentity();
            glDisable(GL_BLEND);
            self.draw_background(palette, width, height, time);
            glEnable(GL_BLEND);
            self.draw_bloom(palette, center, width, height, time);
            self.draw_grid(palette, center, canvas_radius, angle, scene_time);
            self.draw_orbits(palette, center, canvas_radius, time);
            self.draw_origin(palette, center, scale, time);
            self.draw_primes(PrimeFrame {
                palette,
                center,
                scale,
                angle,
                time,
                lit_number,
                field_radius,
            });
            glDisable(GL_BLEND);
            SwapBuffers(self.device_context);
        }
    }

    /// Marks the mathematical origin (number one). It remains dim so every
    /// later bright point continues to represent an actual prime number.
    unsafe fn draw_origin(&self, palette: Palette, center: (f32, f32), scale: f32, time: f32) {
        let glow = palette.accent.scaled(0.55 + 0.25 * (time * 1.7).sin());
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glPointSize((scale * 2.8).clamp(4.0, 15.0));
            glBegin(GL_POINTS);
            glColor4f(glow.r, glow.g, glow.b, 0.11);
            glVertex2f(center.0, center.1);
            glEnd();
            glPointSize((scale * 0.75).clamp(1.5, 5.0));
            glBegin(GL_POINTS);
            glColor4f(glow.r, glow.g, glow.b, 0.85);
            glVertex2f(center.0, center.1);
            glEnd();
        }
    }

    /// The continuous natural-number cursor. It advances at a constant
    /// numbers-per-second rate, so every spiral edge lights at the same speed.
    /// This is analytical rather than a catch-up loop: even a stalled or busy
    /// frame cannot schedule an unbounded backlog.
    fn revealed_number(&self, scene_time: f32) -> f32 {
        let maximum = (self.maximum_side * self.maximum_side) as f32;
        (scene_time / GROWTH_DURATION.as_secs_f32()).clamp(0.0, 1.0) * maximum
    }

    unsafe fn draw_background(&self, palette: Palette, width: f32, height: f32, time: f32) {
        let drift = 0.11 + 0.06 * (time * 0.043).sin();
        unsafe {
            glClearColor(palette.shadow.r, palette.shadow.g, palette.shadow.b, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            glBegin(GL_QUADS);
            glColor3f(palette.shadow.r, palette.shadow.g, palette.shadow.b);
            glVertex2f(0.0, 0.0);
            let upper = palette.deep.scaled(0.48 + drift);
            glColor3f(upper.r, upper.g, upper.b);
            glVertex2f(width, 0.0);
            let lower = palette.deep.scaled(0.23 + drift * 0.45);
            glColor3f(lower.r, lower.g, lower.b);
            glVertex2f(width, height);
            glColor3f(palette.shadow.r, palette.shadow.g, palette.shadow.b);
            glVertex2f(0.0, height);
            glEnd();
        }
    }

    unsafe fn draw_bloom(
        &self,
        palette: Palette,
        center: (f32, f32),
        width: f32,
        height: f32,
        time: f32,
    ) {
        let radius = width.max(height) * (0.55 + 0.05 * (time * 0.071).sin());
        let offset = (time * 0.11).sin() * width * 0.06;
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glBegin(GL_TRIANGLE_FAN);
            let core = palette.body.scaled(0.045);
            glColor4f(core.r, core.g, core.b, 0.38);
            glVertex2f(center.0 + offset, center.1 - offset * 0.25);
            for index in 0..=48 {
                let angle = TAU * index as f32 / 48.0;
                let edge = palette.deep.scaled(0.12);
                glColor4f(edge.r, edge.g, edge.b, 0.0);
                glVertex2f(
                    center.0 + offset + angle.cos() * radius,
                    center.1 - offset * 0.25 + angle.sin() * radius,
                );
            }
            glEnd();
        }
    }

    unsafe fn draw_grid(
        &self,
        palette: Palette,
        center: (f32, f32),
        scale: f32,
        angle: f32,
        scene_time: f32,
    ) {
        let shade = palette.body.scaled(0.17 + 0.04 * (scene_time * 0.16).sin());
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glLineWidth(1.0);
            glColor4f(shade.r, shade.g, shade.b, 0.17);
            glBegin(GL_LINES);
            for index in -8..=8 {
                let position = index as f32 / 8.0;
                let from = project((position, -1.0), center, scale, angle);
                let to = project((position, 1.0), center, scale, angle);
                glVertex2f(from.0, from.1);
                glVertex2f(to.0, to.1);
                let from = project((-1.0, position), center, scale, angle);
                let to = project((1.0, position), center, scale, angle);
                glVertex2f(from.0, from.1);
                glVertex2f(to.0, to.1);
            }
            glEnd();
        }
    }

    unsafe fn draw_orbits(&self, palette: Palette, center: (f32, f32), scale: f32, time: f32) {
        let color = palette.accent.scaled(0.22);
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glLineWidth(1.0);
            for ring in 1..=3 {
                let radius =
                    scale * (0.17 + ring as f32 * 0.18 + 0.012 * (time * 0.4 + ring as f32).sin());
                glColor4f(color.r, color.g, color.b, 0.095 / ring as f32);
                glBegin(GL_LINE_STRIP);
                for index in 0..=96 {
                    let theta = TAU * index as f32 / 96.0 - time * 0.045 * ring as f32;
                    glVertex2f(
                        center.0 + theta.cos() * radius,
                        center.1 + theta.sin() * radius,
                    );
                }
                glEnd();
            }
        }
    }

    unsafe fn draw_primes(&self, frame: PrimeFrame) {
        let PrimeFrame {
            palette,
            center,
            scale,
            angle,
            time,
            lit_number,
            field_radius,
        } = frame;
        let visible_count = self
            .points
            .partition_point(|point| point.number <= lit_number);
        let core_size = (scale * 0.55).clamp(1.0, 5.6);
        let bloom_size = (core_size * 3.3).clamp(3.3, 11.0);
        let numbers_per_second = self.maximum_side * self.maximum_side;
        let numbers_per_second = numbers_per_second as f32 / GROWTH_DURATION.as_secs_f32();
        let cosine = angle.cos();
        let sine = angle.sin();
        unsafe {
            if bloom_size - core_size > 0.35 {
                glBlendFunc(GL_SRC_ALPHA, GL_ONE);
                glPointSize(bloom_size);
                glBegin(GL_POINTS);
                for point in &self.points[..visible_count] {
                    if self.shape == FieldShape::Circle
                        && point.x * point.x + point.y * point.y > field_radius * field_radius
                    {
                        continue;
                    }
                    let freshness = point_freshness(*point, lit_number, numbers_per_second);
                    let (color, pulse) = point_color(*point, palette, time, freshness);
                    let (x, y) = project_cached((point.x, point.y), center, scale, cosine, sine);
                    glColor4f(
                        color.r,
                        color.g,
                        color.b,
                        0.035 + pulse * 0.075 + freshness * 0.30,
                    );
                    glVertex2f(x, y);
                }
                glEnd();
            }

            glPointSize(core_size);
            glBegin(GL_POINTS);
            for point in &self.points[..visible_count] {
                if self.shape == FieldShape::Circle
                    && point.x * point.x + point.y * point.y > field_radius * field_radius
                {
                    continue;
                }
                let freshness = point_freshness(*point, lit_number, numbers_per_second);
                let (color, pulse) = point_color(*point, palette, time, freshness);
                let (x, y) = project_cached((point.x, point.y), center, scale, cosine, sine);
                glColor4f(
                    color.r,
                    color.g,
                    color.b,
                    0.30 + pulse * 0.55 + freshness * 0.15,
                );
                glVertex2f(x, y);
            }
            glEnd();
        }
    }
}

fn project(point: (f32, f32), center: (f32, f32), scale: f32, angle: f32) -> (f32, f32) {
    project_cached(point, center, scale, angle.cos(), angle.sin())
}

fn project_cached(
    point: (f32, f32),
    center: (f32, f32),
    scale: f32,
    cosine: f32,
    sine: f32,
) -> (f32, f32) {
    (
        center.0 + (point.0 * cosine - point.1 * sine) * scale,
        center.1 + (point.0 * sine + point.1 * cosine) * scale,
    )
}

fn point_freshness(point: UlamPoint, lit_number: u32, numbers_per_second: f32) -> f32 {
    let age_in_numbers = lit_number.saturating_sub(point.number) as f32;
    (1.0 - age_in_numbers / (numbers_per_second * NEW_POINT_GLOW_SECONDS)).clamp(0.0, 1.0)
}

fn point_color(point: UlamPoint, palette: Palette, time: f32, freshness: f32) -> (Color, f32) {
    let wave = 0.5 + 0.5 * (point.phase * TAU * 2.0 - time * 0.72).sin();
    let ripple = 0.5 + 0.5 * (point.phase * TAU * 9.0 + time * 1.17).sin();
    let color = palette
        .body
        .mix(palette.accent, wave * 0.76)
        .scaled(point.energy * (0.62 + ripple * 0.48) * (1.0 + freshness * 1.15));
    (color, ripple)
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            return 0;
        }
        WM_DESTROY => {
            unsafe { KillTimer(window, FRAME_TIMER_ID) };
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        WM_ERASEBKGND => return 1,
        _ => {}
    }

    APP.with(|slot| {
        let mut app = slot.borrow_mut();
        let Some(app) = app.as_mut() else {
            return unsafe { DefWindowProcA(window, message, w_param, l_param) };
        };
        match message {
            WM_SIZE => {
                unsafe { app.resize() };
                0
            }
            WM_TIMER if w_param == FRAME_TIMER_ID => {
                unsafe { app.render() };
                0
            }
            WM_SETCURSOR if app.mode == RunMode::Fullscreen => {
                unsafe { SetCursor(std::ptr::null_mut()) };
                1
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
                if app.exit_on_input =>
            {
                unsafe { PostMessageA(window, WM_CLOSE, 0, 0) };
                0
            }
            WM_MOUSEMOVE if app.exit_on_input => {
                let mut mouse = POINT {
                    x: get_x_lparam(l_param),
                    y: get_y_lparam(l_param),
                };
                unsafe { ClientToScreen(window, &mut mouse) };
                if (mouse.x - app.initial_mouse.x).abs() > 4
                    || (mouse.y - app.initial_mouse.y).abs() > 4
                {
                    unsafe { PostMessageA(window, WM_CLOSE, 0, 0) };
                }
                0
            }
            _ => unsafe { DefWindowProcA(window, message, w_param, l_param) },
        }
    })
}

fn get_x_lparam(value: LPARAM) -> i32 {
    (value as u32 & 0xffff) as i16 as i32
}

fn get_y_lparam(value: LPARAM) -> i32 {
    ((value as u32 >> 16) & 0xffff) as i16 as i32
}

unsafe fn register_window_class(instance: HINSTANCE, fullscreen: bool) -> bool {
    let cursor = if fullscreen {
        std::ptr::null_mut()
    } else {
        unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) }
    };
    let class = WNDCLASSEXA {
        cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
        style: CS_OWNDC | CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_procedure),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: WINDOW_CLASS.as_ptr().cast(),
        ..Default::default()
    };
    unsafe { RegisterClassExA(&class) != 0 }
}

unsafe fn create_window(instance: HINSTANCE, mode: RunMode, preview_parent: HWND) -> HWND {
    let (style, ex_style, x, y, width, height, parent) = if mode == RunMode::Preview {
        let mut rect = RECT::default();
        unsafe { GetClientRect(preview_parent, &mut rect) };
        (
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            0,
            (rect.right - rect.left).max(1),
            (rect.bottom - rect.top).max(1),
            preview_parent,
        )
    } else if mode == RunMode::Adjustment {
        (
            WS_OVERLAPPEDWINDOW,
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ADJUSTMENT_WIDTH,
            ADJUSTMENT_HEIGHT,
            std::ptr::null_mut(),
        )
    } else {
        (
            WS_POPUP,
            WS_EX_TOPMOST,
            unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
            std::ptr::null_mut(),
        )
    };

    unsafe {
        CreateWindowExA(
            ex_style,
            WINDOW_CLASS.as_ptr().cast(),
            if mode == RunMode::Adjustment {
                c"Ulam Prime Field - Live Preview".as_ptr().cast()
            } else {
                WINDOW_TITLE.as_ptr().cast()
            },
            style,
            x,
            y,
            width,
            height,
            parent,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    }
}

fn parse_window_handle(text: &str) -> Option<HWND> {
    let text = text.trim().trim_start_matches(':').trim();
    let numeric = if let Some(hex) = text.strip_prefix("0x") {
        usize::from_str_radix(hex, 16).ok()?
    } else {
        text.parse::<usize>().ok()?
    };
    (numeric != 0).then_some(numeric as HWND)
}

unsafe fn show_message(text: &CStr, flags: u32) {
    unsafe {
        MessageBoxA(
            std::ptr::null_mut(),
            text.as_ptr().cast(),
            WINDOW_TITLE.as_ptr().cast(),
            flags,
        )
    };
}

fn configuration_text() -> &'static CStr {
    c"Ulam Prime Field\n\nA Rust-first OpenGL screensaver. Prime numbers are plotted in a living Ulam spiral: the diagonal bands are real patterns in the prime distribution.\n\nRun modes:\n  /s  full-screen saver\n  /p <HWND>  control-panel preview\n  /w  resizable live preview\n  /d  full-screen debug view"
}

unsafe fn run(mode: RunMode, preview_parent: HWND) -> i32 {
    let instance = unsafe { GetModuleHandleA(std::ptr::null()) };
    if instance.is_null()
        || !unsafe { register_window_class(instance, mode == RunMode::Fullscreen) }
    {
        unsafe {
            show_message(
                c"Could not register the screen-saver window.",
                MB_OK | MB_ICONERROR,
            )
        };
        return 1;
    }
    let window = unsafe { create_window(instance, mode, preview_parent) };
    if window.is_null() {
        unsafe {
            show_message(
                c"Could not create the screen-saver window.",
                MB_OK | MB_ICONERROR,
            )
        };
        return 1;
    }
    let app = match unsafe { App::create(window, mode) } {
        Ok(app) => app,
        Err(message) => {
            unsafe { DestroyWindow(window) };
            unsafe { show_message(message, MB_OK | MB_ICONERROR) };
            return 1;
        }
    };
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    // Windows' default timer granularity is ~15.6 ms. At that resolution a
    // nominal 16 ms animation timer produces uneven intervals, which looks like
    // stutter during a slow zoom. Request 1 ms precision for the saver's
    // lifetime and restore the system default afterwards.
    unsafe { timeBeginPeriod(1) };
    unsafe { SetTimer(window, FRAME_TIMER_ID, FRAME_INTERVAL_MS, None) };
    unsafe {
        ShowWindow(
            window,
            if mode == RunMode::Fullscreen {
                SW_SHOWNOACTIVATE
            } else {
                SW_SHOW
            },
        );
        UpdateWindow(window);
    }
    APP.with(|slot| {
        if let Some(app) = slot.borrow_mut().as_mut() {
            unsafe { app.render() };
        }
    });

    let mut message = MSG::default();
    while unsafe { GetMessageA(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageA(&message);
        }
    }
    APP.with(|slot| {
        if let Some(mut app) = slot.borrow_mut().take() {
            unsafe { app.destroy() };
        }
    });
    unsafe { timeEndPeriod(1) };
    0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        unsafe { show_message(configuration_text(), MB_OK | MB_ICONINFORMATION) };
        return;
    };
    let command_lower = command.to_ascii_lowercase();
    let exit_code = if command_lower == "/c" || command_lower.starts_with("/c:") {
        unsafe { show_message(configuration_text(), MB_OK | MB_ICONINFORMATION) };
        0
    } else if command_lower == "/a" {
        0
    } else if command_lower == "/w" {
        unsafe { run(RunMode::Adjustment, std::ptr::null_mut()) }
    } else if command_lower == "/s" || command_lower == "/d" {
        unsafe { run(RunMode::Fullscreen, std::ptr::null_mut()) }
    } else if command_lower == "/p" {
        match args.next().as_deref().and_then(parse_window_handle) {
            Some(parent) => unsafe { run(RunMode::Preview, parent) },
            None => {
                unsafe {
                    show_message(
                        c"Preview mode requires a parent window handle.",
                        MB_OK | MB_ICONERROR,
                    )
                };
                1
            }
        }
    } else if let Some(parent) = command_lower
        .strip_prefix("/p:")
        .and_then(parse_window_handle)
    {
        unsafe { run(RunMode::Preview, parent) }
    } else {
        unsafe {
            show_message(
                c"Supported modes: /s, /p <HWND>, /c, /w, /d.",
                MB_OK | MB_ICONERROR,
            )
        };
        1
    };
    std::process::exit(exit_code);
}
