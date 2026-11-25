//! Embedded one-pass GLSL renderer for Gravity Field.
//!
//! The canonical fragment shader is compiled into the `.scr`, the host supplies
//! the finite scene catalogue, and GPU back-pressure remains inside this scene.

use std::ffi::{CStr, CString};

use slowfield_core::{
    GRAVITY_SCENE_COUNT, GRAVITY_SCENE_DURATION_SECONDS, GravityFieldSettings,
    gravity_scene_descriptor,
};
use windows_sys::Win32::Graphics::OpenGL::*;

use crate::scene::{Frame, Scene, Viewport};

const GL_VERTEX_SHADER: u32 = 0x8b31;
const GL_FRAGMENT_SHADER: u32 = 0x8b30;
const GL_COMPILE_STATUS: u32 = 0x8b81;
const GL_LINK_STATUS: u32 = 0x8b82;
const GL_SYNC_GPU_COMMANDS_COMPLETE: u32 = 0x9117;
const GL_ALREADY_SIGNALED: u32 = 0x911a;
const GL_CONDITION_SATISFIED: u32 = 0x911c;
const GL_WAIT_FAILED: u32 = 0x911d;
const FRAME_INTERVAL_SECONDS: f32 = 0.010;
const FRAME_COOLDOWN_TRIGGER_SECONDS: f32 = 2.0 * FRAME_INTERVAL_SECONDS;
const FRAME_COOLDOWN_DIVISOR: f32 = 4.0;
const FRAME_COOLDOWN_MAX_SECONDS: f32 = 3.0;

type RawGlProc = unsafe extern "system" fn() -> isize;
type CreateShader = unsafe extern "system" fn(u32) -> u32;
type ShaderSource = unsafe extern "system" fn(u32, i32, *const *const i8, *const i32);
type CompileShader = unsafe extern "system" fn(u32);
type GetShaderIv = unsafe extern "system" fn(u32, u32, *mut i32);
type DeleteShader = unsafe extern "system" fn(u32);
type CreateProgram = unsafe extern "system" fn() -> u32;
type AttachShader = unsafe extern "system" fn(u32, u32);
type LinkProgram = unsafe extern "system" fn(u32);
type GetProgramIv = unsafe extern "system" fn(u32, u32, *mut i32);
type DeleteProgram = unsafe extern "system" fn(u32);
type UseProgram = unsafe extern "system" fn(u32);
type GetUniformLocation = unsafe extern "system" fn(u32, *const i8) -> i32;
type Uniform1f = unsafe extern "system" fn(i32, f32);
type Uniform2f = unsafe extern "system" fn(i32, f32, f32);
type Uniform4f = unsafe extern "system" fn(i32, f32, f32, f32, f32);
type GenVertexArrays = unsafe extern "system" fn(i32, *mut u32);
type BindVertexArray = unsafe extern "system" fn(u32);
type DeleteVertexArrays = unsafe extern "system" fn(i32, *const u32);
type FenceSync = unsafe extern "system" fn(u32, u32) -> isize;
type ClientWaitSync = unsafe extern "system" fn(isize, u32, u64) -> u32;
type DeleteSync = unsafe extern "system" fn(isize);

const VERTEX_SHADER: &CStr = c"#version 330
out vec2 vUv;
void main() {
  int id = gl_VertexID;
  float x = float((id & 1) << 2) - 1.0;
  float y = float((id & 2) << 1) - 1.0;
  vUv = vec2((x + 1.0) * 0.5, (y + 1.0) * 0.5);
  gl_Position = vec4(x, y, 0.0, 1.0);
}";
const FRAGMENT_SHADER: &str = include_str!("gravity.frag");

struct Gl {
    create_shader: CreateShader,
    shader_source: ShaderSource,
    compile_shader: CompileShader,
    get_shader_iv: GetShaderIv,
    delete_shader: DeleteShader,
    create_program: CreateProgram,
    attach_shader: AttachShader,
    link_program: LinkProgram,
    get_program_iv: GetProgramIv,
    delete_program: DeleteProgram,
    use_program: UseProgram,
    get_uniform_location: GetUniformLocation,
    uniform_1f: Uniform1f,
    uniform_2f: Uniform2f,
    uniform_4f: Uniform4f,
    gen_vertex_arrays: GenVertexArrays,
    bind_vertex_array: BindVertexArray,
    delete_vertex_arrays: DeleteVertexArrays,
    fence_sync: Option<FenceSync>,
    client_wait_sync: Option<ClientWaitSync>,
    delete_sync: Option<DeleteSync>,
}

impl Gl {
    unsafe fn load() -> Result<Self, String> {
        macro_rules! load {
            ($name:literal, $kind:ty) => {{
                let procedure = unsafe { load_proc($name)? };
                unsafe { std::mem::transmute::<RawGlProc, $kind>(procedure) }
            }};
        }
        Ok(Self {
            create_shader: load!("glCreateShader", CreateShader),
            shader_source: load!("glShaderSource", ShaderSource),
            compile_shader: load!("glCompileShader", CompileShader),
            get_shader_iv: load!("glGetShaderiv", GetShaderIv),
            delete_shader: load!("glDeleteShader", DeleteShader),
            create_program: load!("glCreateProgram", CreateProgram),
            attach_shader: load!("glAttachShader", AttachShader),
            link_program: load!("glLinkProgram", LinkProgram),
            get_program_iv: load!("glGetProgramiv", GetProgramIv),
            delete_program: load!("glDeleteProgram", DeleteProgram),
            use_program: load!("glUseProgram", UseProgram),
            get_uniform_location: load!("glGetUniformLocation", GetUniformLocation),
            uniform_1f: load!("glUniform1f", Uniform1f),
            uniform_2f: load!("glUniform2f", Uniform2f),
            uniform_4f: load!("glUniform4f", Uniform4f),
            gen_vertex_arrays: load!("glGenVertexArrays", GenVertexArrays),
            bind_vertex_array: load!("glBindVertexArray", BindVertexArray),
            delete_vertex_arrays: load!("glDeleteVertexArrays", DeleteVertexArrays),
            fence_sync: unsafe { load_optional_proc("glFenceSync")? },
            client_wait_sync: unsafe { load_optional_proc("glClientWaitSync")? },
            delete_sync: unsafe { load_optional_proc("glDeleteSync")? },
        })
    }
}

#[derive(Default)]
struct Uniforms {
    time: i32,
    resolution: i32,
    star_gain: i32,
    disk_opacity: i32,
    doppler: i32,
    sky_seed: i32,
    sky_flow_direction: i32,
    star_density: i32,
    sky_flow_speed: i32,
    scene_center: i32,
    apparent_radius: i32,
    disk_look_a: i32,
    disk_look_b: i32,
    disk_look_c: i32,
    scene_exposure: i32,
}

pub struct GravityScene {
    viewport: Viewport,
    gl: Option<Gl>,
    program: u32,
    vao: u32,
    uniforms: Uniforms,
    frame_fence: isize,
    frame_submitted_at: f32,
    next_frame_eligible_at: f32,
    scene_random_state: u32,
    catalog_index: usize,
    scene_started_at: f32,
    settings: GravityFieldSettings,
}

impl GravityScene {
    pub fn new(_preview: bool, launch_seed: u32, settings: GravityFieldSettings) -> Self {
        let scene_random_state = nonzero_seed(launch_seed);
        let mut selection_state = scene_random_state;
        let catalog_index = next_random(&mut selection_state) as usize % GRAVITY_SCENE_COUNT;
        Self {
            viewport: Viewport {
                width: 1,
                height: 1,
            },
            gl: None,
            program: 0,
            vao: 0,
            uniforms: Uniforms::default(),
            frame_fence: 0,
            frame_submitted_at: 0.0,
            next_frame_eligible_at: 0.0,
            scene_random_state: selection_state,
            catalog_index,
            scene_started_at: 0.0,
            settings,
        }
    }

    unsafe fn compile(gl: &Gl, shader_type: u32, source: &CStr) -> Result<u32, String> {
        let shader = unsafe { (gl.create_shader)(shader_type) };
        if shader == 0 {
            return Err("OpenGL could not allocate a Gravity Field shader.".into());
        }
        let source_pointer = source.as_ptr();
        let source_length = source.to_bytes().len() as i32;
        unsafe { (gl.shader_source)(shader, 1, &source_pointer, &source_length) };
        unsafe { (gl.compile_shader)(shader) };
        let mut compiled = 0;
        unsafe { (gl.get_shader_iv)(shader, GL_COMPILE_STATUS, &mut compiled) };
        if compiled != 0 {
            Ok(shader)
        } else {
            unsafe { (gl.delete_shader)(shader) };
            Err("Gravity Field shader compilation failed.".into())
        }
    }

    unsafe fn location(gl: &Gl, program: u32, name: &CStr) -> i32 {
        unsafe { (gl.get_uniform_location)(program, name.as_ptr()) }
    }

    unsafe fn upload_frame(&self, frame: Frame) {
        let gl = self.gl.as_ref().expect("Gravity Field must be activated");
        let descriptor = gravity_scene_descriptor(self.catalog_index);
        unsafe {
            set_1f(gl, self.uniforms.time, frame.elapsed_seconds);
            set_2f(
                gl,
                self.uniforms.resolution,
                self.viewport.width as f32,
                self.viewport.height as f32,
            );
            set_1f(
                gl,
                self.uniforms.star_gain,
                self.settings.star_brightness as f32 / 100.0,
            );
            set_1f(
                gl,
                self.uniforms.disk_opacity,
                self.settings.disk_opacity as f32 / 100.0,
            );
            set_1f(
                gl,
                self.uniforms.doppler,
                self.settings.doppler as f32 / 100.0,
            );
            set_1f(
                gl,
                self.uniforms.sky_seed,
                scene_seed(descriptor.catalog_index, self.scene_random_state),
            );
            set_2f(
                gl,
                self.uniforms.sky_flow_direction,
                descriptor.sky_flow_direction.0,
                descriptor.sky_flow_direction.1,
            );
            set_1f(
                gl,
                self.uniforms.star_density,
                self.settings.star_density as f32 / 100.0,
            );
            set_1f(
                gl,
                self.uniforms.sky_flow_speed,
                self.settings.sky_flow_speed as f32 / 100.0,
            );
            set_2f(
                gl,
                self.uniforms.scene_center,
                descriptor.center.0,
                descriptor.center.1,
            );
            set_1f(
                gl,
                self.uniforms.apparent_radius,
                descriptor.apparent_radius,
            );
            set_4f(
                gl,
                self.uniforms.disk_look_a,
                5500.0,
                descriptor.inclination,
                0.0,
                1.80,
            );
            set_4f(gl, self.uniforms.disk_look_b, 8.00, 0.90, 0.60, 2.50);
            set_4f(gl, self.uniforms.disk_look_c, 2.20, 1.60, 7.00, 5.00);
            set_1f(gl, self.uniforms.scene_exposure, 1.40);
        }
    }

    fn advance_scene_to(&mut self, elapsed_seconds: f32) {
        while elapsed_seconds - self.scene_started_at >= GRAVITY_SCENE_DURATION_SECONDS {
            self.scene_started_at += GRAVITY_SCENE_DURATION_SECONDS;
            self.catalog_index = (self.catalog_index + 1) % GRAVITY_SCENE_COUNT;
        }
    }

    unsafe fn previous_frame_complete(&mut self, now: f32) -> bool {
        if self.frame_fence == 0 {
            return true;
        }
        let gl = self.gl.as_ref().expect("Gravity Field must be activated");
        let (Some(wait), Some(delete)) = (gl.client_wait_sync, gl.delete_sync) else {
            unsafe { glFinish() };
            self.frame_fence = 0;
            self.complete_frame_schedule(now);
            return false;
        };
        let result = unsafe { wait(self.frame_fence, 0, 0) };
        if matches!(
            result,
            GL_ALREADY_SIGNALED | GL_CONDITION_SATISFIED | GL_WAIT_FAILED
        ) {
            unsafe { delete(self.frame_fence) };
            self.frame_fence = 0;
            if result == GL_WAIT_FAILED {
                unsafe { glFinish() };
            }
            self.complete_frame_schedule(now);
        }
        false
    }

    fn complete_frame_schedule(&mut self, completed_at: f32) {
        let elapsed = (completed_at - self.frame_submitted_at).max(0.0);
        let cooldown = if elapsed > FRAME_COOLDOWN_TRIGGER_SECONDS {
            ((elapsed - FRAME_INTERVAL_SECONDS) / FRAME_COOLDOWN_DIVISOR)
                .min(FRAME_COOLDOWN_MAX_SECONDS)
        } else {
            0.0
        };
        self.frame_submitted_at = 0.0;
        self.next_frame_eligible_at = completed_at + cooldown;
    }
}

impl Scene for GravityScene {
    unsafe fn activate(&mut self) -> Result<(), String> {
        let gl = unsafe { Gl::load() }?;
        let fragment_source = CString::new(FRAGMENT_SHADER)
            .map_err(|_| "Embedded Gravity Field shader contains an interior NUL byte.")?;
        let vertex = unsafe { Self::compile(&gl, GL_VERTEX_SHADER, VERTEX_SHADER) }?;
        let fragment = match unsafe { Self::compile(&gl, GL_FRAGMENT_SHADER, &fragment_source) } {
            Ok(shader) => shader,
            Err(error) => {
                unsafe { (gl.delete_shader)(vertex) };
                return Err(error);
            }
        };
        let program = unsafe { (gl.create_program)() };
        if program == 0 {
            unsafe {
                (gl.delete_shader)(vertex);
                (gl.delete_shader)(fragment);
            }
            return Err("OpenGL could not allocate a Gravity Field program.".into());
        }
        unsafe {
            (gl.attach_shader)(program, vertex);
            (gl.attach_shader)(program, fragment);
            (gl.link_program)(program);
            (gl.delete_shader)(vertex);
            (gl.delete_shader)(fragment);
        }
        let mut linked = 0;
        unsafe { (gl.get_program_iv)(program, GL_LINK_STATUS, &mut linked) };
        if linked == 0 {
            unsafe { (gl.delete_program)(program) };
            return Err("Gravity Field shader linking failed.".into());
        }
        let mut vao = 0;
        unsafe { (gl.gen_vertex_arrays)(1, &mut vao) };
        if vao == 0 {
            unsafe { (gl.delete_program)(program) };
            return Err("OpenGL could not allocate a Gravity Field vertex array.".into());
        }
        self.uniforms = unsafe {
            Uniforms {
                time: Self::location(&gl, program, c"iTime"),
                resolution: Self::location(&gl, program, c"iResolution"),
                star_gain: Self::location(&gl, program, c"uStarGain"),
                disk_opacity: Self::location(&gl, program, c"uDiskOpacity"),
                doppler: Self::location(&gl, program, c"uDoppler"),
                sky_seed: Self::location(&gl, program, c"uSkySeed"),
                sky_flow_direction: Self::location(&gl, program, c"uSkyFlowDirection"),
                star_density: Self::location(&gl, program, c"uStarDensity"),
                sky_flow_speed: Self::location(&gl, program, c"uSkyFlowSpeed"),
                scene_center: Self::location(&gl, program, c"uSceneCenter"),
                apparent_radius: Self::location(&gl, program, c"uApparentRadius"),
                disk_look_a: Self::location(&gl, program, c"uDiskLookA"),
                disk_look_b: Self::location(&gl, program, c"uDiskLookB"),
                disk_look_c: Self::location(&gl, program, c"uDiskLookC"),
                scene_exposure: Self::location(&gl, program, c"uSceneExposure"),
            }
        };
        self.gl = Some(gl);
        self.program = program;
        self.vao = vao;
        Ok(())
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    unsafe fn render(&mut self, frame: Frame) -> bool {
        if !unsafe { self.previous_frame_complete(frame.elapsed_seconds) }
            || frame.elapsed_seconds < self.next_frame_eligible_at
        {
            return false;
        }
        self.advance_scene_to(frame.elapsed_seconds);
        let gl = self.gl.as_ref().expect("Gravity Field must be activated");
        unsafe {
            glViewport(0, 0, self.viewport.width, self.viewport.height);
            glDisable(GL_BLEND);
            glClearColor(0.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            (gl.use_program)(self.program);
            self.upload_frame(frame);
            (gl.bind_vertex_array)(self.vao);
            glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
            (gl.bind_vertex_array)(0);
            (gl.use_program)(0);
        }
        true
    }

    unsafe fn prepare_present(&mut self, frame: Frame) {
        let gl = self.gl.as_ref().expect("Gravity Field must be activated");
        self.frame_submitted_at = frame.elapsed_seconds;
        if let (Some(fence_sync), Some(_), Some(_)) =
            (gl.fence_sync, gl.client_wait_sync, gl.delete_sync)
        {
            self.frame_fence = unsafe { fence_sync(GL_SYNC_GPU_COMMANDS_COMPLETE, 0) };
        }
    }

    unsafe fn presented(&mut self, frame: Frame, succeeded: bool) {
        let gl = self.gl.as_ref().expect("Gravity Field must be activated");
        if succeeded && self.frame_fence != 0 {
            unsafe { glFlush() };
            return;
        }
        if self.frame_fence != 0 {
            if let Some(delete) = gl.delete_sync {
                unsafe { delete(self.frame_fence) };
            }
            self.frame_fence = 0;
        }
        unsafe { glFinish() };
        self.complete_frame_schedule(frame.elapsed_seconds);
    }

    unsafe fn deactivate(&mut self) {
        let Some(gl) = self.gl.take() else {
            return;
        };
        unsafe {
            (gl.use_program)(0);
            if self.frame_fence != 0 {
                if let Some(delete) = gl.delete_sync {
                    delete(self.frame_fence);
                }
                self.frame_fence = 0;
            }
            if self.vao != 0 {
                (gl.delete_vertex_arrays)(1, &self.vao);
                self.vao = 0;
            }
            if self.program != 0 {
                (gl.delete_program)(self.program);
                self.program = 0;
            }
        }
    }
}

unsafe fn load_proc(name: &str) -> Result<RawGlProc, String> {
    let name = CString::new(name).expect("OpenGL entry point names contain no NUL bytes");
    let procedure = unsafe { wglGetProcAddress(name.as_ptr().cast()) }
        .ok_or_else(|| format!("OpenGL does not expose {}.", name.to_string_lossy()))?;
    let address = procedure as usize;
    if matches!(address, 1..=3) || address == usize::MAX {
        return Err(format!(
            "OpenGL returned an invalid pointer for {}.",
            name.to_string_lossy()
        ));
    }
    Ok(procedure)
}

unsafe fn load_optional_proc<T>(name: &str) -> Result<Option<T>, String> {
    let name = CString::new(name).expect("OpenGL entry point names contain no NUL bytes");
    let Some(procedure) = (unsafe { wglGetProcAddress(name.as_ptr().cast()) }) else {
        return Ok(None);
    };
    let address = procedure as usize;
    if matches!(address, 1..=3) || address == usize::MAX {
        return Ok(None);
    }
    // The caller binds `T` to the documented ABI for this entry point.
    Ok(Some(unsafe {
        std::mem::transmute_copy::<RawGlProc, T>(&procedure)
    }))
}

unsafe fn set_1f(gl: &Gl, location: i32, value: f32) {
    if location >= 0 {
        unsafe { (gl.uniform_1f)(location, value) };
    }
}

unsafe fn set_2f(gl: &Gl, location: i32, x: f32, y: f32) {
    if location >= 0 {
        unsafe { (gl.uniform_2f)(location, x, y) };
    }
}

unsafe fn set_4f(gl: &Gl, location: i32, x: f32, y: f32, z: f32, w: f32) {
    if location >= 0 {
        unsafe { (gl.uniform_4f)(location, x, y, z, w) };
    }
}

fn scene_seed(catalog_index: usize, scene_random_state: u32) -> f32 {
    let mut value = (catalog_index as u32 + 1).wrapping_mul(0x9e37_79b9);
    value ^= scene_random_state;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    (value & 0x00ff_ffff) as f32 * (1.0 / 16_777_216.0)
}

fn nonzero_seed(value: u32) -> u32 {
    if value == 0 { 0x6d2b_79f5 } else { value }
}

fn next_random(state: &mut u32) -> u32 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    *state = nonzero_seed(value);
    *state
}

#[cfg(test)]
mod tests {
    use super::{FRAGMENT_SHADER, scene_seed};

    #[test]
    fn embedded_shader_preserves_its_required_source_contract() {
        assert!(FRAGMENT_SHADER.starts_with("#version 330"));
        for uniform in ["iTime", "iResolution", "uSceneCenter", "uApparentRadius"] {
            assert!(FRAGMENT_SHADER.contains(uniform));
        }
    }

    #[test]
    fn procedural_seed_is_stable_and_normalized() {
        assert_eq!(scene_seed(12, 7), scene_seed(12, 7));
        assert_ne!(scene_seed(12, 7), scene_seed(13, 7));
        assert_ne!(scene_seed(12, 7), scene_seed(12, 8));
        assert!((0.0..1.0).contains(&scene_seed(0, 7)));
    }
}
