use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use slowfield_core::{Command, LaunchSelection, SceneId, WindowHandle, select_scene};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetDC, HDC, ReleaseDC, UpdateWindow};
use windows_sys::Win32::Graphics::OpenGL::*;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

use crate::scene::{Frame, Scene, Viewport, available_scenes, create_scene};

const WINDOW_CLASS: &CStr = c"SlowfieldScreensaver";
const WINDOW_TITLE: &CStr = c"Slowfield";
const FRAME_TIMER_ID: usize = 1;
const FRAME_INTERVAL_MS: u32 = 4;
const ADJUSTMENT_WIDTH: i32 = 1120;
const ADJUSTMENT_HEIGHT: i32 = 760;

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
    unsafe fn create(
        window: HWND,
        mode: RunMode,
        scene: Box<dyn Scene>,
    ) -> Result<Self, &'static CStr> {
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
        unsafe { self.scene.render(frame) };
        unsafe { SwapBuffers(self.device_context) };
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
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
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
        }
    })
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
    let scene = match choose_scene() {
        Ok(scene) => create_scene(scene, mode == RunMode::Preview),
        Err(error) => {
            unsafe { DestroyWindow(window) };
            show_error(&error.to_string());
            return 1;
        }
    };
    let app = match unsafe { App::create(window, mode, scene) } {
        Ok(app) => app,
        Err(message) => {
            unsafe { DestroyWindow(window) };
            show_message(message, MB_OK | MB_ICONERROR);
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

fn hwnd_from(handle: WindowHandle) -> HWND {
    handle.raw() as usize as HWND
}

fn choose_scene() -> Result<SceneId, slowfield_core::SceneSelectionError> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    select_scene(LaunchSelection::Random, available_scenes(), seed)
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
