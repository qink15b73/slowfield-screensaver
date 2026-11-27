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
use windows_sys::Win32::Graphics::Gdi::{
    BLACK_BRUSH, COLOR_WINDOW, ClientToScreen, EnumDisplayMonitors, GetDC, GetMonitorInfoA,
    GetStockObject, GetSysColorBrush, HBRUSH, HDC, HMONITOR, MONITORINFO, ReleaseDC, UpdateWindow,
};
use windows_sys::Win32::Graphics::OpenGL::*;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExA, RegOpenKeyExA, RegQueryValueExA, RegSetValueExA,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
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
const BACKGROUND_WINDOW_CLASS: &CStr = c"SlowfieldBackground";
const SETTINGS_WINDOW_CLASS: &CStr = c"SlowfieldSettings";
const SETTINGS_WINDOW_TITLE: &CStr = c"Slowfield Settings";
const SETTINGS_WIDTH: i32 = 520;
const SETTINGS_HEIGHT: i32 = 570;
const ID_SELECTION: i32 = 100;
const ID_STAR_BRIGHTNESS: i32 = 101;
const ID_DISK_OPACITY: i32 = 102;
const ID_DOPPLER: i32 = 103;
const ID_STAR_DENSITY: i32 = 104;
const ID_SKY_FLOW_SPEED: i32 = 105;
const ID_SOLID_WALLS: i32 = 106;
const ID_FLOCK_SIZE: i32 = 107;
const ID_PREDATOR_COUNT: i32 = 108;
const ID_MAX_SPEED: i32 = 109;
const ID_PREDATOR_SPEED: i32 = 110;
const ID_WALL_TURN_DISTANCE: i32 = 111;
const ID_WALL_FORCE: i32 = 112;
const ID_SAVE: i32 = 113;
const ID_CANCEL: i32 = 114;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DisplayBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayMonitor {
    bounds: DisplayBounds,
    is_primary: bool,
}

struct FullscreenInputTarget {
    render_window: HWND,
    initial_mouse: POINT,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DpiAwareness {
    PerMonitor,
    Fallback,
}

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
        let mut app = Self {
            window,
            device_context,
            render_context,
            mode,
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
    static FULLSCREEN_INPUT_TARGET: RefCell<Option<FullscreenInputTarget>> = const { RefCell::new(None) };
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

fn with_fullscreen_input_target<T>(action: impl FnOnce(&FullscreenInputTarget) -> T) -> Option<T> {
    FULLSCREEN_INPUT_TARGET.with(|slot| slot.borrow().as_ref().map(action))
}

fn close_fullscreen_render_window() {
    let _ = with_fullscreen_input_target(|target| unsafe {
        PostMessageA(target.render_window, WM_CLOSE, 0, 0);
    });
}

fn initialize_fullscreen_input_target(render_window: HWND) {
    let mut initial_mouse = POINT::default();
    unsafe { GetCursorPos(&mut initial_mouse) };
    FULLSCREEN_INPUT_TARGET.with(|slot| {
        *slot.borrow_mut() = Some(FullscreenInputTarget {
            render_window,
            initial_mouse,
        });
    });
}

fn clear_fullscreen_input_target() {
    FULLSCREEN_INPUT_TARGET.with(|slot| *slot.borrow_mut() = None);
}

pub fn show_configuration(owner: Option<WindowHandle>) -> i32 {
    let instance = unsafe { GetModuleHandleA(std::ptr::null()) };
    if instance.is_null() || !unsafe { register_settings_window_class(instance) } {
        show_message(
            c"Could not register the Slowfield settings window.",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    let config = load_config();
    let window = unsafe {
        CreateWindowExA(
            0,
            SETTINGS_WINDOW_CLASS.as_ptr().cast(),
            SETTINGS_WINDOW_TITLE.as_ptr().cast(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            SETTINGS_WIDTH,
            SETTINGS_HEIGHT,
            owner.map_or(std::ptr::null_mut(), hwnd_from),
            std::ptr::null_mut(),
            instance,
            Box::into_raw(Box::new(config)).cast(),
        )
    };
    if window.is_null() {
        show_message(
            c"Could not create the Slowfield settings window.",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    unsafe { ShowWindow(window, SW_SHOWNORMAL) };
    unsafe { UpdateWindow(window) };
    let mut message = MSG::default();
    while unsafe { GetMessageA(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageA(&message);
        }
    }
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
            close_fullscreen_render_window();
            0
        }
        WM_MOUSEMOVE if app.mode.exits_on_input() => {
            let mut mouse = POINT {
                x: low_word_as_i16(l_param),
                y: high_word_as_i16(l_param),
            };
            unsafe { ClientToScreen(window, &mut mouse) };
            let exceeded_dead_zone = with_fullscreen_input_target(|target| {
                (mouse.x - target.initial_mouse.x).abs() > 4
                    || (mouse.y - target.initial_mouse.y).abs() > 4
            })
            .unwrap_or(false);
            if exceeded_dead_zone {
                close_fullscreen_render_window();
            }
            0
        }
        _ => unsafe { DefWindowProcA(window, message, w_param, l_param) },
    }) else {
        return unsafe { DefWindowProcA(window, message, w_param, l_param) };
    };
    result
}

unsafe fn register_settings_window_class(instance: HINSTANCE) -> bool {
    let class = WNDCLASSEXA {
        cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(settings_window_procedure),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszClassName: SETTINGS_WINDOW_CLASS.as_ptr().cast(),
        ..Default::default()
    };
    unsafe { RegisterClassExA(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
}

unsafe extern "system" fn settings_window_procedure(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        settings_window_procedure_impl(window, message, w_param, l_param)
    }));
    match result {
        Ok(result) => result,
        Err(_) => {
            unsafe { DestroyWindow(window) };
            0
        }
    }
}

unsafe fn settings_window_procedure_impl(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            let create = unsafe { &*(l_param as *const CREATESTRUCTA) };
            unsafe { SetWindowLongPtrA(window, GWLP_USERDATA, create.lpCreateParams as isize) };
            let config = unsafe { settings_config(window) };
            unsafe { create_settings_controls(window, config) };
            0
        }
        WM_COMMAND => {
            let command_id = (w_param as u32 & 0xffff) as i32;
            match command_id {
                ID_SAVE => match unsafe { read_settings_controls(window) } {
                    Some(config) if save_config(config) => {
                        unsafe { DestroyWindow(window) };
                    }
                    Some(_) => show_error(
                        "Could not save Slowfield settings to the current-user registry.",
                    ),
                    None => show_message(
                        c"Enter a value within every displayed range.",
                        MB_OK | MB_ICONWARNING,
                    ),
                },
                ID_CANCEL => {
                    unsafe { DestroyWindow(window) };
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            0
        }
        WM_NCDESTROY => {
            let state = unsafe { GetWindowLongPtrA(window, GWLP_USERDATA) };
            if state != 0 {
                unsafe { drop(Box::from_raw(state as *mut SlowfieldConfig)) };
                unsafe { SetWindowLongPtrA(window, GWLP_USERDATA, 0) };
            }
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcA(window, message, w_param, l_param) },
    }
}

unsafe fn settings_config(window: HWND) -> SlowfieldConfig {
    let pointer = unsafe { GetWindowLongPtrA(window, GWLP_USERDATA) } as *const SlowfieldConfig;
    unsafe { *pointer }
}

unsafe fn create_settings_controls(window: HWND, config: SlowfieldConfig) {
    let instance = unsafe { GetModuleHandleA(std::ptr::null()) };
    let mut y = 18;
    unsafe { add_label(window, instance, c"Startup field", 18, y) };
    let selection = unsafe {
        add_control(
            window,
            instance,
            c"COMBOBOX",
            c"",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | (CBS_DROPDOWNLIST as u32),
            180,
            y - 3,
            260,
            180,
            ID_SELECTION,
        )
    };
    for label in [
        c"Random",
        c"Prime Field",
        c"Gravity Field",
        c"Flock Field",
        c"Boltzmann Field",
    ] {
        unsafe { SendMessageA(selection, CB_ADDSTRING, 0, label.as_ptr() as isize) };
    }
    let selection_index = match config.launch_selection {
        LaunchSelection::Random => 0,
        LaunchSelection::Fixed(SceneId::Prime) => 1,
        LaunchSelection::Fixed(SceneId::Gravity) => 2,
        LaunchSelection::Fixed(SceneId::Flock) => 3,
        LaunchSelection::Fixed(SceneId::Boltzmann) => 4,
    };
    unsafe { SendMessageA(selection, CB_SETCURSEL, selection_index, 0) };

    y += 42;
    unsafe { add_label(window, instance, c"Gravity Field", 18, y) };
    y += 25;
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Star brightness (0-100)",
            ID_STAR_BRIGHTNESS,
            config.gravity.star_brightness,
            &mut y,
        )
    };
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Disk opacity (0-100)",
            ID_DISK_OPACITY,
            config.gravity.disk_opacity,
            &mut y,
        )
    };
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Doppler (0-100)",
            ID_DOPPLER,
            config.gravity.doppler,
            &mut y,
        )
    };
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Star density (50-200)",
            ID_STAR_DENSITY,
            config.gravity.star_density,
            &mut y,
        )
    };
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Sky flow speed (0-500)",
            ID_SKY_FLOW_SPEED,
            config.gravity.sky_flow_speed,
            &mut y,
        )
    };

    y += 12;
    unsafe { add_label(window, instance, c"Flock Field", 18, y) };
    y += 25;
    let solid_walls = unsafe {
        add_control(
            window,
            instance,
            c"BUTTON",
            c"Use solid walls",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | (BS_AUTOCHECKBOX as u32),
            18,
            y,
            220,
            24,
            ID_SOLID_WALLS,
        )
    };
    unsafe {
        SendMessageA(
            solid_walls,
            BM_SETCHECK,
            config.flock.solid_walls as usize,
            0,
        )
    };
    y += 30;
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Flock size (0-5000)",
            ID_FLOCK_SIZE,
            config.flock.flock_size as u32,
            &mut y,
        )
    };
    unsafe {
        add_integer_row(
            window,
            instance,
            c"Predators (0-20)",
            ID_PREDATOR_COUNT,
            config.flock.predator_count as u32,
            &mut y,
        )
    };
    unsafe {
        add_float_row(
            window,
            instance,
            c"Maximum speed (50-800)",
            ID_MAX_SPEED,
            config.flock.max_speed,
            &mut y,
        )
    };
    unsafe {
        add_float_row(
            window,
            instance,
            c"Predator speed multiplier (1-3)",
            ID_PREDATOR_SPEED,
            config.flock.predator_speed_multiplier,
            &mut y,
        )
    };
    unsafe {
        add_float_row(
            window,
            instance,
            c"Wall turn distance (0-500)",
            ID_WALL_TURN_DISTANCE,
            config.flock.wall_turn_distance,
            &mut y,
        )
    };
    unsafe {
        add_float_row(
            window,
            instance,
            c"Wall force multiplier (0-1)",
            ID_WALL_FORCE,
            config.flock.wall_force_multiplier,
            &mut y,
        )
    };

    unsafe {
        add_control(
            window,
            instance,
            c"BUTTON",
            c"Save",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | (BS_PUSHBUTTON as u32),
            280,
            y + 14,
            75,
            28,
            ID_SAVE,
        )
    };
    unsafe {
        add_control(
            window,
            instance,
            c"BUTTON",
            c"Cancel",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | (BS_PUSHBUTTON as u32),
            365,
            y + 14,
            75,
            28,
            ID_CANCEL,
        )
    };
}

unsafe fn add_label(window: HWND, instance: HINSTANCE, text: &CStr, x: i32, y: i32) {
    unsafe {
        add_control(
            window,
            instance,
            c"STATIC",
            text,
            WS_CHILD | WS_VISIBLE,
            x,
            y,
            245,
            20,
            0,
        )
    };
}

unsafe fn add_integer_row(
    window: HWND,
    instance: HINSTANCE,
    label: &CStr,
    id: i32,
    value: u32,
    y: &mut i32,
) {
    unsafe { add_label(window, instance, label, 18, *y + 4) };
    let edit = unsafe {
        add_control(
            window,
            instance,
            c"EDIT",
            c"",
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WS_TABSTOP
                | (ES_AUTOHSCROLL as u32)
                | (ES_NUMBER as u32),
            300,
            *y,
            140,
            23,
            id,
        )
    };
    unsafe {
        SetWindowTextA(
            edit,
            CString::new(value.to_string())
                .expect("numeric text has no NUL")
                .as_ptr()
                .cast(),
        )
    };
    *y += 28;
}

unsafe fn add_float_row(
    window: HWND,
    instance: HINSTANCE,
    label: &CStr,
    id: i32,
    value: f32,
    y: &mut i32,
) {
    unsafe { add_label(window, instance, label, 18, *y + 4) };
    let edit = unsafe {
        add_control(
            window,
            instance,
            c"EDIT",
            c"",
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | (ES_AUTOHSCROLL as u32),
            300,
            *y,
            140,
            23,
            id,
        )
    };
    unsafe {
        SetWindowTextA(
            edit,
            CString::new(format!("{value:.3}"))
                .expect("numeric text has no NUL")
                .as_ptr()
                .cast(),
        )
    };
    *y += 28;
}

#[expect(
    clippy::too_many_arguments,
    reason = "The helper intentionally names the Win32 child-window geometry and identity."
)]
unsafe fn add_control(
    parent: HWND,
    instance: HINSTANCE,
    class_name: &CStr,
    text: &CStr,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: i32,
) -> HWND {
    unsafe {
        CreateWindowExA(
            0,
            class_name.as_ptr().cast(),
            text.as_ptr().cast(),
            style,
            x,
            y,
            width,
            height,
            parent,
            id as usize as HMENU,
            instance,
            std::ptr::null(),
        )
    }
}

unsafe fn read_settings_controls(window: HWND) -> Option<SlowfieldConfig> {
    let selection = unsafe { SendMessageA(GetDlgItem(window, ID_SELECTION), CB_GETCURSEL, 0, 0) };
    let launch_selection = match selection {
        0 => LaunchSelection::Random,
        1 => LaunchSelection::Fixed(SceneId::Prime),
        2 => LaunchSelection::Fixed(SceneId::Gravity),
        3 => LaunchSelection::Fixed(SceneId::Flock),
        4 => LaunchSelection::Fixed(SceneId::Boltzmann),
        _ => return None,
    };
    let gravity = GravityFieldSettings {
        star_brightness: unsafe { read_control_u32(window, ID_STAR_BRIGHTNESS)? },
        disk_opacity: unsafe { read_control_u32(window, ID_DISK_OPACITY)? },
        doppler: unsafe { read_control_u32(window, ID_DOPPLER)? },
        star_density: unsafe { read_control_u32(window, ID_STAR_DENSITY)? },
        sky_flow_speed: unsafe { read_control_u32(window, ID_SKY_FLOW_SPEED)? },
    };
    let flock = FlockFieldSettings {
        solid_walls: unsafe { SendMessageA(GetDlgItem(window, ID_SOLID_WALLS), BM_GETCHECK, 0, 0) }
            == 1,
        flock_size: unsafe { read_control_i32(window, ID_FLOCK_SIZE)? },
        predator_count: unsafe { read_control_i32(window, ID_PREDATOR_COUNT)? },
        max_speed: unsafe { read_control_f32(window, ID_MAX_SPEED)? },
        predator_speed_multiplier: unsafe { read_control_f32(window, ID_PREDATOR_SPEED)? },
        wall_turn_distance: unsafe { read_control_f32(window, ID_WALL_TURN_DISTANCE)? },
        wall_force_multiplier: unsafe { read_control_f32(window, ID_WALL_FORCE)? },
    };
    (gravity.is_valid() && flock.is_valid()).then_some(SlowfieldConfig {
        launch_selection,
        gravity,
        flock,
    })
}

unsafe fn read_control_u32(window: HWND, id: i32) -> Option<u32> {
    unsafe { read_control_text(window, id) }.parse().ok()
}

unsafe fn read_control_i32(window: HWND, id: i32) -> Option<i32> {
    unsafe { read_control_text(window, id) }.parse().ok()
}

unsafe fn read_control_f32(window: HWND, id: i32) -> Option<f32> {
    let value = unsafe { read_control_text(window, id) }
        .parse::<f32>()
        .ok()?;
    value.is_finite().then_some(value)
}

unsafe fn read_control_text(window: HWND, id: i32) -> String {
    let mut buffer = [0_u8; 64];
    let length =
        unsafe { GetDlgItemTextA(window, id, buffer.as_mut_ptr().cast(), buffer.len() as i32) };
    std::str::from_utf8(&buffer[..length as usize])
        .ok()
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

fn save_config(config: SlowfieldConfig) -> bool {
    if !config.gravity.is_valid() || !config.flock.is_valid() {
        return false;
    }
    let mut key: HKEY = std::ptr::null_mut();
    let created = unsafe {
        RegCreateKeyExA(
            HKEY_CURRENT_USER,
            CONFIG_KEY.as_ptr().cast(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } == 0;
    if !created {
        return false;
    }
    let saved = unsafe {
        write_registry_dword(key, CONFIG_SCHEMA_VALUE, 0)
            && write_registry_dword(
                key,
                CONFIG_SELECTION_VALUE,
                config.launch_selection.storage_value(),
            )
            && write_registry_dword(
                key,
                c"Gravity.StarBrightness",
                config.gravity.star_brightness,
            )
            && write_registry_dword(key, c"Gravity.DiskOpacity", config.gravity.disk_opacity)
            && write_registry_dword(key, c"Gravity.Doppler", config.gravity.doppler)
            && write_registry_dword(key, c"Gravity.StarDensity", config.gravity.star_density)
            && write_registry_dword(key, c"Gravity.SkyFlowSpeed", config.gravity.sky_flow_speed)
            && write_registry_dword(key, c"Flock.SolidWalls", config.flock.solid_walls as u32)
            && write_registry_dword(key, c"Flock.FlockSize", config.flock.flock_size as u32)
            && write_registry_dword(
                key,
                c"Flock.PredatorCount",
                config.flock.predator_count as u32,
            )
            && write_registry_float(key, c"Flock.MaxSpeed", config.flock.max_speed)
            && write_registry_float(
                key,
                c"Flock.PredatorSpeedMultiplier",
                config.flock.predator_speed_multiplier,
            )
            && write_registry_float(
                key,
                c"Flock.WallTurnDistance",
                config.flock.wall_turn_distance,
            )
            && write_registry_float(
                key,
                c"Flock.WallForceMultiplier",
                config.flock.wall_force_multiplier,
            )
            && write_registry_dword(key, CONFIG_SCHEMA_VALUE, CONFIG_SCHEMA_VERSION)
    };
    unsafe { RegCloseKey(key) };
    saved
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

unsafe fn register_background_window_class(instance: HINSTANCE) -> bool {
    let class = WNDCLASSEXA {
        cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
        lpfnWndProc: Some(background_window_procedure),
        hInstance: instance,
        hbrBackground: unsafe { GetStockObject(BLACK_BRUSH) as HBRUSH },
        lpszClassName: BACKGROUND_WINDOW_CLASS.as_ptr().cast(),
        ..Default::default()
    };
    unsafe { RegisterClassExA(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
}

unsafe extern "system" fn background_window_procedure(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match message {
        WM_SETCURSOR => {
            unsafe { SetCursor(std::ptr::null_mut()) };
            1
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            close_fullscreen_render_window();
            0
        }
        WM_MOUSEMOVE => {
            let mut mouse = POINT {
                x: low_word_as_i16(l_param),
                y: high_word_as_i16(l_param),
            };
            unsafe { ClientToScreen(window, &mut mouse) };
            let exceeded_dead_zone = with_fullscreen_input_target(|target| {
                (mouse.x - target.initial_mouse.x).abs() > 4
                    || (mouse.y - target.initial_mouse.y).abs() > 4
            })
            .unwrap_or(false);
            if exceeded_dead_zone {
                close_fullscreen_render_window();
            }
            0
        }
        _ => unsafe { DefWindowProcA(window, message, w_param, l_param) },
    }
}

unsafe fn create_window(
    instance: HINSTANCE,
    mode: RunMode,
    preview_parent: HWND,
    fullscreen_bounds: DisplayBounds,
) -> HWND {
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
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            fullscreen_bounds.x,
            fullscreen_bounds.y,
            fullscreen_bounds.width,
            fullscreen_bounds.height,
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

unsafe fn create_background_window(instance: HINSTANCE, bounds: DisplayBounds) -> HWND {
    unsafe {
        CreateWindowExA(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            BACKGROUND_WINDOW_CLASS.as_ptr().cast(),
            WINDOW_TITLE.as_ptr().cast(),
            WS_POPUP,
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    }
}

fn configure_dpi_awareness() -> DpiAwareness {
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } != 0 {
        DpiAwareness::PerMonitor
    } else {
        DpiAwareness::Fallback
    }
}

unsafe fn enumerate_display_monitors() -> Option<Vec<DisplayMonitor>> {
    let mut monitors = Vec::new();
    let completed = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_display_monitor),
            (&mut monitors as *mut Vec<DisplayMonitor>) as isize,
        )
    } != 0;
    (completed && !monitors.is_empty()).then_some(monitors)
}

unsafe extern "system" fn collect_display_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> i32 {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoA(monitor, &mut info) } == 0 {
        return 0;
    }
    let width = info.rcMonitor.right - info.rcMonitor.left;
    let height = info.rcMonitor.bottom - info.rcMonitor.top;
    if width <= 0 || height <= 0 {
        return 1;
    }
    let monitors = unsafe { &mut *(data as *mut Vec<DisplayMonitor>) };
    monitors.push(DisplayMonitor {
        bounds: DisplayBounds {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            width,
            height,
        },
        is_primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
    });
    1
}

fn primary_display(monitors: &[DisplayMonitor]) -> DisplayMonitor {
    monitors
        .iter()
        .copied()
        .find(|monitor| monitor.is_primary)
        .unwrap_or(monitors[0])
}

fn background_display_bounds(
    monitors: &[DisplayMonitor],
    render_bounds: DisplayBounds,
    dpi_awareness: DpiAwareness,
) -> Vec<DisplayBounds> {
    monitors
        .iter()
        .filter_map(|monitor| (monitor.bounds != render_bounds).then_some(monitor.bounds))
        // A per-monitor-aware process receives physical monitor rectangles.
        // If the older API fallback was the only available option, extend the
        // black window past its lower/right edges to avoid a DPI-rounding seam.
        .map(|bounds| match dpi_awareness {
            DpiAwareness::PerMonitor => bounds,
            DpiAwareness::Fallback => DisplayBounds {
                width: bounds.width.saturating_add(1),
                height: bounds.height.saturating_add(1),
                ..bounds
            },
        })
        .collect()
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
    let dpi_awareness = configure_dpi_awareness();
    let monitors = if mode == RunMode::Fullscreen {
        match unsafe { enumerate_display_monitors() } {
            Some(monitors) => monitors,
            None => {
                show_message(
                    c"Could not identify a display for Slowfield.",
                    MB_OK | MB_ICONERROR,
                );
                return 1;
            }
        }
    } else {
        Vec::new()
    };
    let render_bounds = if mode == RunMode::Fullscreen {
        primary_display(&monitors).bounds
    } else {
        DisplayBounds::default()
    };
    let window = unsafe { create_window(instance, mode, preview_parent, render_bounds) };
    if window.is_null() {
        show_message(
            c"Could not create the Slowfield window.",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    let mut background_windows = Vec::new();
    if mode == RunMode::Fullscreen {
        if !unsafe { register_background_window_class(instance) } {
            unsafe { DestroyWindow(window) };
            show_message(
                c"Could not register the Slowfield background window class.",
                MB_OK | MB_ICONERROR,
            );
            return 1;
        }
        let bounds = background_display_bounds(&monitors, render_bounds, dpi_awareness);
        for bounds in bounds {
            let background = unsafe { create_background_window(instance, bounds) };
            if background.is_null() {
                for background in background_windows {
                    unsafe { DestroyWindow(background) };
                }
                unsafe { DestroyWindow(window) };
                show_message(
                    c"Could not create a Slowfield background window.",
                    MB_OK | MB_ICONERROR,
                );
                return 1;
            }
            background_windows.push(background);
        }
        initialize_fullscreen_input_target(window);
    }
    let config = load_config();
    let (scene_id, launch_seed) = match choose_scene(config.launch_selection) {
        Ok(selection) => selection,
        Err(error) => {
            for background in background_windows {
                unsafe { DestroyWindow(background) };
            }
            clear_fullscreen_input_target();
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
            for background in background_windows {
                unsafe { DestroyWindow(background) };
            }
            clear_fullscreen_input_target();
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
        for background in &background_windows {
            ShowWindow(*background, SW_SHOWNOACTIVATE);
            UpdateWindow(*background);
        }
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
    for background in background_windows {
        unsafe { DestroyWindow(background) };
    }
    clear_fullscreen_input_target();
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

unsafe fn write_registry_dword(key: HKEY, name: &CStr, value: u32) -> bool {
    unsafe {
        RegSetValueExA(
            key,
            name.as_ptr().cast(),
            0,
            REG_DWORD,
            (&value as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
        ) == 0
    }
}

unsafe fn write_registry_float(key: HKEY, name: &CStr, value: f32) -> bool {
    let value = match CString::new(format!("{value:.3}")) {
        Ok(value) => value,
        Err(_) => return false,
    };
    unsafe {
        RegSetValueExA(
            key,
            name.as_ptr().cast(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            value.as_bytes_with_nul().len() as u32,
        ) == 0
    }
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

#[cfg(test)]
mod tests {
    use super::{
        DisplayBounds, DisplayMonitor, DpiAwareness, background_display_bounds, primary_display,
    };

    fn display(x: i32, is_primary: bool) -> DisplayMonitor {
        DisplayMonitor {
            bounds: DisplayBounds {
                x,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
            is_primary,
        }
    }

    #[test]
    fn primary_display_prefers_the_display_flagged_by_windows() {
        assert_eq!(
            primary_display(&[display(-1_920, false), display(0, true)]),
            display(0, true)
        );
    }

    #[test]
    fn primary_display_falls_back_to_the_first_display() {
        assert_eq!(
            primary_display(&[display(-1_920, false), display(0, false)]),
            display(-1_920, false)
        );
    }

    #[test]
    fn only_non_rendering_displays_receive_background_windows() {
        let primary = display(0, true);
        let left = display(-1_920, false);
        let right = display(1_920, false);
        assert_eq!(
            background_display_bounds(
                &[left, primary, right],
                primary.bounds,
                DpiAwareness::PerMonitor,
            ),
            vec![left.bounds, right.bounds]
        );
    }

    #[test]
    fn fallback_backgrounds_extend_past_lower_and_right_edges() {
        let primary = display(0, true);
        let secondary = display(1_920, false);
        assert_eq!(
            background_display_bounds(
                &[primary, secondary],
                primary.bounds,
                DpiAwareness::Fallback,
            ),
            vec![DisplayBounds {
                width: secondary.bounds.width + 1,
                height: secondary.bounds.height + 1,
                ..secondary.bounds
            }]
        );
    }
}
