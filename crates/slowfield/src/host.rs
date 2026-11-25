use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use slowfield_core::{
    CONFIG_SCHEMA_VERSION, Command, FlockFieldSettings, GravityFieldSettings, LaunchSelection,
    SceneId, SlowfieldConfig, WindowHandle, select_scene,
};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetDC, HDC, ReleaseDC, UpdateWindow};
use windows_sys::Win32::Graphics::OpenGL::*;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegOpenKeyExA,
    RegQueryValueExA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

use crate::scene::{Frame, Scene, Viewport, available_scenes, create_scene};

const WINDOW_CLASS: &CStr = c"SlowfieldScreensaver";
const WINDOW_TITLE: &CStr = c"Slowfield";
const FRAME_TIMER_ID: usize = 1;
const FRAME_INTERVAL_MS: u32 = 4;
const ADJUSTMENT_WIDTH: i32 = 1120;
const ADJUSTMENT_HEIGHT: i32 = 760;
const CONFIG_KEY: &CStr = c"Software\\Qink\\Slowfield";
const CONFIG_SCHEMA_VALUE: &CStr = c"ConfigSchemaVersion";
const CONFIG_SELECTION_VALUE: &CStr = c"LaunchSelection";
const LEGACY_SELECTION_SCHEMA_VERSION: u32 = 1;
const LEGACY_GRAVITY_KEY: &CStr = c"Software\\BlackHoleScreensaver";
const LEGACY_FLOCK_KEY: &CStr = c"Software\\Qink\\BoidsScreensaver";
const WGL_CONTEXT_MAJOR_VERSION_ARB: i32 = 0x2091;
const WGL_CONTEXT_MINOR_VERSION_ARB: i32 = 0x2092;
const WGL_CONTEXT_PROFILE_MASK_ARB: i32 = 0x9126;
const WGL_CONTEXT_COMPATIBILITY_PROFILE_BIT_ARB: i32 = 0x0000_0002;

type WglCreateContextAttribsArb = unsafe extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunMode {
    Fullscreen,
    Preview,
    Adjustment,
}

impl RunMode {
    const fn exits_on_input(self) -> bool {
        matches!(self, Self::Fullscreen)
    }
}

struct App {
    window: HWND,
    device_context: HDC,
    render_context: HGLRC,
    mode: RunMode,
    initial_mouse: POINT,
    launch: Instant,
    scene: Box<dyn Scene>,
}

impl App {
    unsafe fn create(window: HWND, mode: RunMode, scene: Box<dyn Scene>) -> Result<Self, String> {
        let device_context = unsafe { GetDC(window) };
        if device_context.is_null() {
            return Err("Could not acquire a drawing surface.".into());
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
            return Err("Could not configure an OpenGL pixel format.".into());
        }

        let mut render_context = unsafe { wglCreateContext(device_context) };
        if render_context.is_null()
            || unsafe { wglMakeCurrent(device_context, render_context) } == 0
        {
            if !render_context.is_null() {
                unsafe { wglDeleteContext(render_context) };
            }
            unsafe { ReleaseDC(window, device_context) };
            return Err("Could not create an OpenGL rendering context.".into());
        }

        unsafe {
            glDisable(GL_DEPTH_TEST);
            glDisable(GL_CULL_FACE);
        }
        // Every released field shares this baseline: Prime/Flock use legacy
        // compatibility drawing while Gravity requires GLSL 330. Failing
        // early prevents a random Gravity selection becoming an opaque shader
        // failure on a driver that cannot create the required context.
        let Some(create_context) = (unsafe { wgl_create_context_attribs() }) else {
            unsafe { wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) };
            unsafe { wglDeleteContext(render_context) };
            unsafe { ReleaseDC(window, device_context) };
            return Err("Slowfield requires an OpenGL 3.3 compatibility context.".into());
        };
        let attributes = [
            WGL_CONTEXT_MAJOR_VERSION_ARB,
            3,
            WGL_CONTEXT_MINOR_VERSION_ARB,
            3,
            WGL_CONTEXT_PROFILE_MASK_ARB,
            WGL_CONTEXT_COMPATIBILITY_PROFILE_BIT_ARB,
            0,
        ];
        let upgraded_context =
            unsafe { create_context(device_context, std::ptr::null_mut(), attributes.as_ptr()) };
        if upgraded_context.is_null() {
            unsafe { wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) };
            unsafe { wglDeleteContext(render_context) };
            unsafe { ReleaseDC(window, device_context) };
            return Err("Slowfield requires an OpenGL 3.3 compatibility context.".into());
        }
        unsafe { wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) };
        unsafe { wglDeleteContext(render_context) };
        render_context = upgraded_context;
        if unsafe { wglMakeCurrent(device_context, render_context) } == 0 {
            unsafe { wglDeleteContext(render_context) };
            unsafe { ReleaseDC(window, device_context) };
            return Err("Could not activate the OpenGL compatibility context.".into());
        }
        let mut initial_mouse = POINT::default();
        unsafe { GetCursorPos(&mut initial_mouse) };
        let mut app = Self {
            window,
            device_context,
            render_context,
            mode,
            initial_mouse,
            launch: Instant::now(),
            scene,
        };
        if let Err(error) = unsafe { app.scene.activate() } {
            unsafe { wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) };
            unsafe { wglDeleteContext(render_context) };
            unsafe { ReleaseDC(window, device_context) };
            return Err(error);
        }
        unsafe { app.resize() };
        Ok(app)
    }

    unsafe fn resize(&mut self) {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.window, &mut rect) };
        let viewport = Viewport {
            width: (rect.right - rect.left).max(1),
            height: (rect.bottom - rect.top).max(1),
        };
        unsafe { glViewport(0, 0, viewport.width, viewport.height) };
        self.scene.resize(viewport);
    }

    unsafe fn render(&mut self) {
        let frame = Frame {
            elapsed_seconds: self.launch.elapsed().as_secs_f32(),
        };
        if unsafe { self.scene.render(frame) } {
            unsafe { self.scene.prepare_present(frame) };
            let succeeded = unsafe { SwapBuffers(self.device_context) } != 0;
            let presented_frame = Frame {
                elapsed_seconds: self.launch.elapsed().as_secs_f32(),
            };
            unsafe { self.scene.presented(presented_frame, succeeded) };
        }
    }

    unsafe fn destroy(&mut self) {
        unsafe { self.scene.deactivate() };
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
}

unsafe fn wgl_create_context_attribs() -> Option<WglCreateContextAttribsArb> {
    let procedure = unsafe { wglGetProcAddress(c"wglCreateContextAttribsARB".as_ptr().cast()) }?;
    let address = procedure as usize;
    if matches!(address, 1..=3) || address == usize::MAX {
        return None;
    }
    // WGL documents this extension entry point with this exact ABI.
    Some(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, WglCreateContextAttribsArb>(
            procedure,
        )
    })
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs a window-message action without retaining a `RefCell` borrow across
/// Win32 or OpenGL calls. `SwapBuffers` can synchronously dispatch nested
/// messages on some drivers; keeping `App` in the slot during that work would
/// otherwise turn a legitimate nested message into a `RefCell` panic.
fn with_app<T>(action: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|slot| {
        let mut app = { slot.borrow_mut().take()? };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(&mut app)));
        *slot.borrow_mut() = Some(app);
        match outcome {
            Ok(value) => Some(value),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

pub fn show_configuration() -> i32 {
    show_message(
        c"Slowfield\n\nA quiet collection of slow generative fields.\n\nThe configuration UI will arrive with the shared settings schema. For now, Slowfield selects its available field at launch.\n\nRun modes:\n  /s  full-screen saver\n  /p <HWND>  control-panel preview\n  /w  resizable diagnostic preview\n  /d  full-screen debug view",
        MB_OK | MB_ICONINFORMATION,
    );
    0
}

pub fn show_error(text: &str) {
    let sanitized = text.replace('\0', " ");
    let message =
        CString::new(sanitized).unwrap_or_else(|_| CString::new("Slowfield failed.").unwrap());
    unsafe {
        MessageBoxA(
            std::ptr::null_mut(),
            message.as_ptr().cast(),
            WINDOW_TITLE.as_ptr().cast(),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    // A panic may not cross the Win32 ABI boundary. The scene is intentionally
    // best-effort during a saver session: if a renderer fails, close the
    // window cleanly instead of invoking Rust's aborting panic guard.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        window_procedure_impl(window, message, w_param, l_param)
    }));
    match result {
        Ok(result) => result,
        Err(_) => {
            unsafe { PostMessageA(window, WM_CLOSE, 0, 0) };
            0
        }
    }
}

unsafe fn window_procedure_impl(
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

    let Some(result) = with_app(|app| match message {
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
            if app.mode.exits_on_input() =>
        {
            unsafe { PostMessageA(window, WM_CLOSE, 0, 0) };
            0
        }
        WM_MOUSEMOVE if app.mode.exits_on_input() => {
            let mut mouse = POINT {
                x: low_word_as_i16(l_param),
                y: high_word_as_i16(l_param),
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
    }) else {
        return unsafe { DefWindowProcA(window, message, w_param, l_param) };
    };
    result
}

const fn low_word_as_i16(value: LPARAM) -> i32 {
    (value as u32 & 0xffff) as i16 as i32
}

const fn high_word_as_i16(value: LPARAM) -> i32 {
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
    unsafe { RegisterClassExA(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
}

unsafe fn create_window(instance: HINSTANCE, mode: RunMode, preview_parent: HWND) -> HWND {
    let (style, ex_style, x, y, width, height, parent) = match mode {
        RunMode::Preview => {
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
        }
        RunMode::Adjustment => (
            WS_OVERLAPPEDWINDOW,
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ADJUSTMENT_WIDTH,
            ADJUSTMENT_HEIGHT,
            std::ptr::null_mut(),
        ),
        RunMode::Fullscreen => (
            WS_POPUP,
            WS_EX_TOPMOST,
            unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
            unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
            std::ptr::null_mut(),
        ),
    };
    unsafe {
        CreateWindowExA(
            ex_style,
            WINDOW_CLASS.as_ptr().cast(),
            if mode == RunMode::Adjustment {
                c"Slowfield - Live Preview".as_ptr().cast()
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

pub unsafe fn run(command: Command) -> i32 {
    let (mode, preview_parent) = match command {
        Command::Fullscreen | Command::Debug => (RunMode::Fullscreen, std::ptr::null_mut()),
        Command::Preview { parent } => (RunMode::Preview, hwnd_from(parent)),
        Command::Adjustment => (RunMode::Adjustment, std::ptr::null_mut()),
        Command::Configure { .. } | Command::Install => return 0,
    };
    let instance = unsafe { GetModuleHandleA(std::ptr::null()) };
    if instance.is_null()
        || !unsafe { register_window_class(instance, mode == RunMode::Fullscreen) }
    {
        show_message(
            c"Could not register the Slowfield window class.",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    let window = unsafe { create_window(instance, mode, preview_parent) };
    if window.is_null() {
        show_message(
            c"Could not create the Slowfield window.",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    let config = load_config();
    let (scene_id, launch_seed) = match choose_scene(config.launch_selection) {
        Ok(selection) => selection,
        Err(error) => {
            unsafe { DestroyWindow(window) };
            show_error(&error.to_string());
            return 1;
        }
    };
    let scene = create_scene(
        scene_id,
        mode == RunMode::Preview,
        launch_seed,
        config.gravity,
        config.flock,
    );
    let app = match unsafe { App::create(window, mode, scene) } {
        Ok(app) => app,
        Err(message) => {
            unsafe { DestroyWindow(window) };
            show_error(&message);
            return 1;
        }
    };
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    // At the default Windows timer granularity, a nominal 16 ms animation
    // frame jitters visibly. Keep the system request scoped to our one saver
    // process and restore it during teardown.
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
    let _ = with_app(|app| unsafe { app.render() });

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

fn hwnd_from(handle: WindowHandle) -> HWND {
    handle.raw() as usize as HWND
}

fn choose_scene(
    selection: LaunchSelection,
) -> Result<(SceneId, u32), slowfield_core::SceneSelectionError> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    select_scene(selection, available_scenes(), seed).map(|scene| (scene, seed as u32))
}

/// Loads a complete Slowfield schema v2 when available. The original v1 schema
/// carried only scene selection and is safely upgraded in memory. No valid new
/// schema means defaults plus an individually-safe, read-only legacy import.
fn load_config() -> SlowfieldConfig {
    match load_slowfield_config() {
        Some(StoredConfig::V2(config)) => config,
        Some(StoredConfig::V1(launch_selection)) => SlowfieldConfig {
            launch_selection,
            ..import_legacy_config()
        },
        None => import_legacy_config(),
    }
}

enum StoredConfig {
    V1(LaunchSelection),
    V2(SlowfieldConfig),
}

fn load_slowfield_config() -> Option<StoredConfig> {
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExA(
            HKEY_CURRENT_USER,
            CONFIG_KEY.as_ptr().cast(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return None;
    }
    let config = (|| {
        let schema = unsafe { read_registry_dword(key, CONFIG_SCHEMA_VALUE) }?;
        let stored_selection = unsafe { read_registry_dword(key, CONFIG_SELECTION_VALUE) }?;
        let launch_selection = LaunchSelection::from_storage(stored_selection)?;
        match schema {
            LEGACY_SELECTION_SCHEMA_VERSION => Some(StoredConfig::V1(launch_selection)),
            CONFIG_SCHEMA_VERSION => {
                let gravity = unsafe { read_gravity_settings(key, "Gravity")? };
                let flock = unsafe { read_flock_settings(key, "Flock")? };
                Some(StoredConfig::V2(SlowfieldConfig {
                    launch_selection,
                    gravity,
                    flock,
                }))
            }
            _ => None,
        }
    })();
    unsafe { RegCloseKey(key) };
    config
}

fn import_legacy_config() -> SlowfieldConfig {
    let mut config = SlowfieldConfig::default();
    with_registry_key(LEGACY_GRAVITY_KEY, |key| {
        config.gravity = unsafe { import_legacy_gravity_settings(key, config.gravity) };
    });
    with_registry_key(LEGACY_FLOCK_KEY, |key| {
        config.flock = unsafe { import_legacy_flock_settings(key, config.flock) };
    });
    config
}

fn with_registry_key(name: &CStr, action: impl FnOnce(HKEY)) {
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExA(
            HKEY_CURRENT_USER,
            name.as_ptr().cast(),
            0,
            KEY_READ,
            &mut key,
        )
    } == 0
    {
        action(key);
        unsafe { RegCloseKey(key) };
    }
}

unsafe fn read_gravity_settings(key: HKEY, prefix: &str) -> Option<GravityFieldSettings> {
    let settings = GravityFieldSettings {
        star_brightness: unsafe { read_named_dword(key, prefix, "StarBrightness")? },
        disk_opacity: unsafe { read_named_dword(key, prefix, "DiskOpacity")? },
        doppler: unsafe { read_named_dword(key, prefix, "Doppler")? },
        star_density: unsafe { read_named_dword(key, prefix, "StarDensity")? },
        sky_flow_speed: unsafe { read_named_dword(key, prefix, "SkyFlowSpeed")? },
    };
    settings.is_valid().then_some(settings)
}

unsafe fn read_flock_settings(key: HKEY, prefix: &str) -> Option<FlockFieldSettings> {
    let solid_walls = unsafe { read_named_dword(key, prefix, "SolidWalls")? };
    let settings = FlockFieldSettings {
        solid_walls: match solid_walls {
            0 => false,
            1 => true,
            _ => return None,
        },
        flock_size: unsafe { read_named_dword(key, prefix, "FlockSize")? }
            .try_into()
            .ok()?,
        predator_count: unsafe { read_named_dword(key, prefix, "PredatorCount")? }
            .try_into()
            .ok()?,
        max_speed: unsafe { read_named_float(key, prefix, "MaxSpeed")? },
        predator_speed_multiplier: unsafe {
            read_named_float(key, prefix, "PredatorSpeedMultiplier")?
        },
        wall_turn_distance: unsafe { read_named_float(key, prefix, "WallTurnDistance")? },
        wall_force_multiplier: unsafe { read_named_float(key, prefix, "WallForceMultiplier")? },
    };
    settings.is_valid().then_some(settings)
}

unsafe fn import_legacy_gravity_settings(
    key: HKEY,
    mut settings: GravityFieldSettings,
) -> GravityFieldSettings {
    if let Some(value) = unsafe { read_registry_dword(key, c"StarBrightness") } {
        let candidate = GravityFieldSettings {
            star_brightness: value,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"DiskOpacity") } {
        let candidate = GravityFieldSettings {
            disk_opacity: value,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"Doppler") } {
        let candidate = GravityFieldSettings {
            doppler: value,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"StarDensity") } {
        let candidate = GravityFieldSettings {
            star_density: value,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"SkyFlowSpeed") } {
        let candidate = GravityFieldSettings {
            sky_flow_speed: value,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    settings
}

unsafe fn import_legacy_flock_settings(
    key: HKEY,
    mut settings: FlockFieldSettings,
) -> FlockFieldSettings {
    if let Some(value) = unsafe { read_registry_dword(key, c"SolidWalls") }
        && let Some(solid_walls) = match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    {
        let candidate = FlockFieldSettings {
            solid_walls,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"FlockSize") }
        && let Ok(flock_size) = value.try_into()
    {
        let candidate = FlockFieldSettings {
            flock_size,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(value) = unsafe { read_registry_dword(key, c"PredatorCount") }
        && let Ok(predator_count) = value.try_into()
    {
        let candidate = FlockFieldSettings {
            predator_count,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(max_speed) = unsafe { read_registry_float(key, c"MaxSpeed") } {
        let candidate = FlockFieldSettings {
            max_speed,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(predator_speed_multiplier) =
        unsafe { read_registry_float(key, c"PredatorSpeedMultiplier") }
    {
        let candidate = FlockFieldSettings {
            predator_speed_multiplier,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(wall_turn_distance) = unsafe { read_registry_float(key, c"WallTurnDistance") } {
        let candidate = FlockFieldSettings {
            wall_turn_distance,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    if let Some(wall_force_multiplier) = unsafe { read_registry_float(key, c"WallForceMultiplier") }
    {
        let candidate = FlockFieldSettings {
            wall_force_multiplier,
            ..settings
        };
        if candidate.is_valid() {
            settings = candidate;
        }
    }
    settings
}

unsafe fn read_named_dword(key: HKEY, prefix: &str, suffix: &str) -> Option<u32> {
    let name = registry_value_name(prefix, suffix)?;
    unsafe { read_registry_dword(key, &name) }
}

unsafe fn read_named_float(key: HKEY, prefix: &str, suffix: &str) -> Option<f32> {
    let name = registry_value_name(prefix, suffix)?;
    unsafe { read_registry_float(key, &name) }
}

fn registry_value_name(prefix: &str, suffix: &str) -> Option<CString> {
    let separator = if prefix.is_empty() { "" } else { "." };
    CString::new(format!("{prefix}{separator}{suffix}")).ok()
}

unsafe fn read_registry_dword(key: HKEY, name: &CStr) -> Option<u32> {
    let mut value_type = 0;
    let mut value = 0_u32;
    let mut byte_count = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegQueryValueExA(
            key,
            name.as_ptr().cast(),
            std::ptr::null(),
            &mut value_type,
            (&mut value as *mut u32).cast(),
            &mut byte_count,
        )
    };
    (result == 0 && value_type == REG_DWORD && byte_count == std::mem::size_of::<u32>() as u32)
        .then_some(value)
}

unsafe fn read_registry_float(key: HKEY, name: &CStr) -> Option<f32> {
    let mut value_type = 0;
    let mut bytes = [0_u8; 64];
    let mut byte_count = bytes.len() as u32;
    let result = unsafe {
        RegQueryValueExA(
            key,
            name.as_ptr().cast(),
            std::ptr::null(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut byte_count,
        )
    };
    if result != 0 || value_type != REG_SZ || byte_count == 0 || byte_count > bytes.len() as u32 {
        return None;
    }
    let text = std::str::from_utf8(&bytes[..byte_count as usize])
        .ok()?
        .strip_suffix('\0')?
        .trim();
    let value = text.parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn show_message(text: &CStr, flags: u32) {
    unsafe {
        MessageBoxA(
            std::ptr::null_mut(),
            text.as_ptr().cast(),
            WINDOW_TITLE.as_ptr().cast(),
            flags,
        );
    }
}
