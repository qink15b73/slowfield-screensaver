//! A bounded hard-disk gas visualisation for Boltzmann Field.
//!
//! This is a pedagogical simulation, not a numerical proof of the
//! Boltzmann--Grad limit. It keeps the physical particles identical: colour is
//! derived from the current velocity direction and magnitude.

use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

use windows_sys::Win32::Graphics::OpenGL::*;

use crate::scene::{Frame, Scene, Viewport};

const FULL_PARTICLE_COUNT: usize = 420;
const PREVIEW_PARTICLE_COUNT: usize = 120;
const MINIMUM_PARTICLE_COUNT: usize = 12;
const MINIMUM_RADIUS: f32 = 2.0;
const TARGET_RADIUS_RATIO: f32 = 0.115;
const TARGET_OCCUPANCY: f32 = 0.07;
const SIMULATION_STEP_SECONDS: f32 = 1.0 / 60.0;
const MAXIMUM_STEPS_PER_RENDER: usize = 6;
const FULL_RENDER_INTERVAL_SECONDS: f32 = 1.0 / 60.0;
const PREVIEW_RENDER_INTERVAL_SECONDS: f32 = 1.0 / 30.0;
const PHASE_SECONDS: f32 = 105.0;
const STAT_SAMPLE_SECONDS: f32 = 0.35;
const MAX_STAT_SAMPLES: usize = 240;
const MAX_RECENT_COLLISIONS: usize = 96;
const COLLISION_FLASH_SECONDS: f32 = 0.22;
const VELOCITY_BINS: usize = 12;
const SPEED_BINS: usize = 18;
const SIDEBAR_MINIMUM_WIDTH: i32 = 520;
const SIDEBAR_MINIMUM_HEIGHT: i32 = 340;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Vec2 {
    x: f32,
    y: f32,
}

impl Vec2 {
    const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn length_squared(self) -> f32 {
        self.x.mul_add(self.x, self.y * self.y)
    }

    fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    fn dot(self, other: Self) -> f32 {
        self.x.mul_add(other.x, self.y * other.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::AddAssign for Vec2 {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        Self::new(self.x * scalar, self.y * scalar)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Self;

    fn div(self, scalar: f32) -> Self {
        Self::new(self.x / scalar, self.y / scalar)
    }
}

#[derive(Clone, Copy, Debug)]
struct Particle {
    position: Vec2,
    velocity: Vec2,
    collision_flash: f32,
}

#[derive(Clone, Copy, Debug)]
struct CollisionLink {
    first: usize,
    second: usize,
    age_seconds: f32,
}

#[derive(Clone, Copy, Debug)]
struct StatSample {
    phase_seconds: f32,
    normalized_entropy: f32,
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Rect {
    fn right(self) -> f32 {
        self.x + self.width
    }

    fn bottom(self) -> f32 {
        self.y + self.height
    }

    fn inset(self, amount: f32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(1.0),
            height: (self.height - amount * 2.0).max(1.0),
        }
    }
}

#[derive(Clone, Copy)]
struct Color {
    red: f32,
    green: f32,
    blue: f32,
}

impl Color {
    const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    fn mixed_with_white(self, amount: f32) -> Self {
        Self::new(
            self.red + (1.0 - self.red) * amount,
            self.green + (1.0 - self.green) * amount,
            self.blue + (1.0 - self.blue) * amount,
        )
    }
}

struct HardDiskSimulation {
    width: f32,
    height: f32,
    requested_particle_count: usize,
    radius: f32,
    speed_scale: f32,
    velocity_extent: f32,
    seed: u32,
    cycle: u32,
    phase_seconds: f32,
    sample_accumulator_seconds: f32,
    particles: Vec<Particle>,
    grid_cell_size: f32,
    grid_width: usize,
    grid_height: usize,
    grid_cells: Vec<Vec<usize>>,
    candidate_pairs: Vec<(usize, usize)>,
    collision_count: u64,
    recent_collisions: Vec<CollisionLink>,
    entropy_history: VecDeque<StatSample>,
    initial_energy: f32,
}

impl HardDiskSimulation {
    fn new(width: f32, height: f32, requested_particle_count: usize, seed: u32) -> Self {
        let mut simulation = Self {
            width: 1.0,
            height: 1.0,
            requested_particle_count,
            radius: MINIMUM_RADIUS,
            speed_scale: 1.0,
            velocity_extent: 1.0,
            seed: seed.max(1),
            cycle: 0,
            phase_seconds: 0.0,
            sample_accumulator_seconds: 0.0,
            particles: Vec::new(),
            grid_cell_size: 1.0,
            grid_width: 1,
            grid_height: 1,
            grid_cells: Vec::new(),
            candidate_pairs: Vec::new(),
            collision_count: 0,
            recent_collisions: Vec::new(),
            entropy_history: VecDeque::new(),
            initial_energy: 0.0,
        };
        simulation.resize(width, height);
        simulation
    }

    fn resize(&mut self, width: f32, height: f32) {
        self.width = width.max(1.0);
        self.height = height.max(1.0);
        let area = self.width * self.height;
        let maximum_for_minimum_radius =
            (area * TARGET_OCCUPANCY / (PI * MINIMUM_RADIUS.powi(2))).floor() as usize;
        let particle_count = self
            .requested_particle_count
            .min(maximum_for_minimum_radius.max(MINIMUM_PARTICLE_COUNT));
        self.radius = (area / particle_count.max(1) as f32)
            .sqrt()
            .mul_add(TARGET_RADIUS_RATIO, 0.0)
            .max(MINIMUM_RADIUS);
        self.speed_scale = self.width.min(self.height).max(64.0) * 0.145;
        self.velocity_extent = self.speed_scale * 1.75;
        self.resize_grid();
        self.reset();
    }

    fn reset(&mut self) {
        self.phase_seconds = 0.0;
        self.sample_accumulator_seconds = 0.0;
        self.collision_count = 0;
        self.recent_collisions.clear();
        self.entropy_history.clear();
        self.particles.clear();
        self.particles.reserve(self.requested_particle_count);

        let mut random_state = mix_bits(self.seed ^ self.cycle.wrapping_mul(0x9e37_79b9));
        let particle_count = self.requested_particle_count.min(
            (self.width * self.height * TARGET_OCCUPANCY / (PI * self.radius.powi(2))) as usize,
        );
        for _ in 0..particle_count {
            let position = self.random_non_overlapping_position(&mut random_state);
            let direction = if position.x < self.width * 0.5 {
                0.0
            } else {
                PI
            };
            let direction = direction + random_signed(&mut random_state) * 0.55;
            let speed = self.speed_scale * (0.56 + random_unit(&mut random_state) * 0.36);
            self.particles.push(Particle {
                position,
                velocity: Vec2::new(direction.cos() * speed, direction.sin() * speed),
                collision_flash: 0.0,
            });
        }
        self.initial_energy = self.total_energy();
        self.sample_statistics();
    }

    fn random_non_overlapping_position(&self, random_state: &mut u32) -> Vec2 {
        let minimum_separation_squared = (self.radius * 2.03).powi(2);
        for _ in 0..1_024 {
            let position = Vec2::new(
                random_unit(random_state) * self.width,
                random_unit(random_state) * self.height,
            );
            if self.particles.iter().all(|particle| {
                self.wrapped_delta(particle.position, position)
                    .length_squared()
                    >= minimum_separation_squared
            }) {
                return position;
            }
        }

        // This fallback is only reachable for an unexpectedly dense tiny
        // preview. The regular placement budget is ample at the capped density.
        let index = self.particles.len();
        let columns = (self.width / (self.radius * 2.5)).floor().max(1.0) as usize;
        let rows = (self.height / (self.radius * 2.5)).floor().max(1.0) as usize;
        Vec2::new(
            (index % columns) as f32 * self.width / columns as f32 + self.radius,
            (index / columns % rows) as f32 * self.height / rows as f32 + self.radius,
        )
    }

    fn resize_grid(&mut self) {
        self.grid_cell_size = (self.radius * 4.0).max(MINIMUM_RADIUS * 4.0);
        self.grid_width = (self.width / self.grid_cell_size).ceil().max(1.0) as usize;
        self.grid_height = (self.height / self.grid_cell_size).ceil().max(1.0) as usize;
        self.grid_cells.clear();
        self.grid_cells
            .resize_with(self.grid_width * self.grid_height, Vec::new);
    }

    fn step(&mut self, seconds: f32) {
        for particle in &mut self.particles {
            particle.position += particle.velocity * seconds;
            particle.position = wrap_position(particle.position, self.width, self.height);
            particle.collision_flash =
                (particle.collision_flash - seconds / COLLISION_FLASH_SECONDS).max(0.0);
        }
        for collision in &mut self.recent_collisions {
            collision.age_seconds += seconds;
        }
        self.recent_collisions
            .retain(|collision| collision.age_seconds < 4.0);

        self.rebuild_grid();
        self.collect_candidate_pairs();
        let pairs = std::mem::take(&mut self.candidate_pairs);
        for (first, second) in pairs.iter().copied() {
            self.resolve_pair(first, second);
        }
        self.candidate_pairs = pairs;

        self.phase_seconds += seconds;
        self.sample_accumulator_seconds += seconds;
        if self.phase_seconds >= PHASE_SECONDS {
            self.cycle = self.cycle.wrapping_add(1);
            self.reset();
            return;
        }
        if self.sample_accumulator_seconds >= STAT_SAMPLE_SECONDS {
            self.sample_accumulator_seconds = 0.0;
            self.sample_statistics();
        }
    }

    fn rebuild_grid(&mut self) {
        for cell in &mut self.grid_cells {
            cell.clear();
        }
        for (index, particle) in self.particles.iter().enumerate() {
            let cell = self.cell_index_for(particle.position);
            self.grid_cells[cell].push(index);
        }
    }

    fn collect_candidate_pairs(&mut self) {
        self.candidate_pairs.clear();
        let cells = &self.grid_cells;
        let pairs = &mut self.candidate_pairs;
        let cell_size = self.grid_cell_size;
        let grid_width = self.grid_width;
        let grid_height = self.grid_height;
        for (first, particle) in self.particles.iter().enumerate() {
            let cell_x = (particle.position.x / cell_size)
                .floor()
                .clamp(0.0, grid_width.saturating_sub(1) as f32) as usize;
            let cell_y = (particle.position.y / cell_size)
                .floor()
                .clamp(0.0, grid_height.saturating_sub(1) as f32) as usize;
            for offset_y in -1..=1 {
                for offset_x in -1..=1 {
                    let neighbor_x =
                        (cell_x as isize + offset_x).rem_euclid(grid_width as isize) as usize;
                    let neighbor_y =
                        (cell_y as isize + offset_y).rem_euclid(grid_height as isize) as usize;
                    let neighbor = neighbor_y * grid_width + neighbor_x;
                    for &second in &cells[neighbor] {
                        if second > first {
                            pairs.push((first, second));
                        }
                    }
                }
            }
        }
        // On a very small viewport a periodic neighbor cell can be reached by
        // more than one offset. Resolve every physical pair only once.
        self.candidate_pairs.sort_unstable();
        self.candidate_pairs.dedup();
    }

    fn resolve_pair(&mut self, first: usize, second: usize) {
        let first_particle = self.particles[first];
        let second_particle = self.particles[second];
        let offset = self.wrapped_delta(first_particle.position, second_particle.position);
        let distance_squared = offset.length_squared();
        let diameter = self.radius * 2.0;
        if distance_squared >= diameter * diameter {
            return;
        }
        let distance = distance_squared.sqrt();
        let normal = if distance > 1.0e-5 {
            offset / distance
        } else {
            unit_from_pair(first, second)
        };
        let normal_velocity = (second_particle.velocity - first_particle.velocity).dot(normal);
        let correction = normal * ((diameter - distance).max(0.0) * 0.51);
        let (left, right) = self.particles.split_at_mut(second);
        let first_particle = &mut left[first];
        let second_particle = &mut right[0];
        first_particle.position = wrap_position(
            first_particle.position - correction,
            self.width,
            self.height,
        );
        second_particle.position = wrap_position(
            second_particle.position + correction,
            self.width,
            self.height,
        );
        if normal_velocity >= 0.0 {
            return;
        }
        first_particle.velocity += normal * normal_velocity;
        second_particle.velocity += normal * -normal_velocity;
        first_particle.collision_flash = 1.0;
        second_particle.collision_flash = 1.0;
        self.collision_count = self.collision_count.saturating_add(1);
        self.recent_collisions.push(CollisionLink {
            first,
            second,
            age_seconds: 0.0,
        });
        if self.recent_collisions.len() > MAX_RECENT_COLLISIONS {
            self.recent_collisions.remove(0);
        }
    }

    fn sample_statistics(&mut self) {
        let normalized_entropy = self.normalized_velocity_entropy();
        if self.entropy_history.len() == MAX_STAT_SAMPLES {
            self.entropy_history.pop_front();
        }
        self.entropy_history.push_back(StatSample {
            phase_seconds: self.phase_seconds,
            normalized_entropy,
        });
    }

    fn normalized_velocity_entropy(&self) -> f32 {
        let mut bins = [0_u32; VELOCITY_BINS * VELOCITY_BINS];
        for particle in &self.particles {
            let x = velocity_bin(particle.velocity.x, self.velocity_extent);
            let y = velocity_bin(particle.velocity.y, self.velocity_extent);
            bins[y * VELOCITY_BINS + x] += 1;
        }
        let maximum_entropy = (VELOCITY_BINS * VELOCITY_BINS) as f32;
        (shannon_entropy(&bins) / maximum_entropy.ln()).clamp(0.0, 1.0)
    }

    fn speed_histogram(&self) -> [u32; SPEED_BINS] {
        let mut bins = [0_u32; SPEED_BINS];
        for particle in &self.particles {
            let index = ((particle.velocity.length() / self.velocity_extent) * SPEED_BINS as f32)
                .floor() as isize;
            bins[index.clamp(0, SPEED_BINS as isize - 1) as usize] += 1;
        }
        bins
    }

    fn mean_speed_squared(&self) -> f32 {
        if self.particles.is_empty() {
            return 0.0;
        }
        self.particles
            .iter()
            .map(|particle| particle.velocity.length_squared())
            .sum::<f32>()
            / self.particles.len() as f32
    }

    fn total_energy(&self) -> f32 {
        self.particles
            .iter()
            .map(|particle| particle.velocity.length_squared() * 0.5)
            .sum()
    }

    fn relative_energy_error(&self) -> f32 {
        if self.initial_energy <= f32::EPSILON {
            0.0
        } else {
            ((self.total_energy() - self.initial_energy) / self.initial_energy).abs()
        }
    }

    fn cell_coordinates_for(&self, position: Vec2) -> (usize, usize) {
        let x = (position.x / self.grid_cell_size)
            .floor()
            .clamp(0.0, self.grid_width.saturating_sub(1) as f32) as usize;
        let y = (position.y / self.grid_cell_size)
            .floor()
            .clamp(0.0, self.grid_height.saturating_sub(1) as f32) as usize;
        (x, y)
    }

    fn cell_index_for(&self, position: Vec2) -> usize {
        let (x, y) = self.cell_coordinates_for(position);
        y * self.grid_width + x
    }

    fn wrapped_delta(&self, from: Vec2, to: Vec2) -> Vec2 {
        let mut delta = to - from;
        if delta.x > self.width * 0.5 {
            delta.x -= self.width;
        } else if delta.x < -self.width * 0.5 {
            delta.x += self.width;
        }
        if delta.y > self.height * 0.5 {
            delta.y -= self.height;
        } else if delta.y < -self.height * 0.5 {
            delta.y += self.height;
        }
        delta
    }
}

fn wrap_position(mut position: Vec2, width: f32, height: f32) -> Vec2 {
    position.x = position.x.rem_euclid(width);
    position.y = position.y.rem_euclid(height);
    position
}

fn velocity_bin(component: f32, extent: f32) -> usize {
    (((component / extent + 1.0) * 0.5 * VELOCITY_BINS as f32).floor() as isize)
        .clamp(0, VELOCITY_BINS as isize - 1) as usize
}

fn shannon_entropy(bins: &[u32]) -> f32 {
    let total = bins.iter().sum::<u32>();
    if total == 0 {
        return 0.0;
    }
    bins.iter().fold(0.0, |entropy, &count| {
        if count == 0 {
            entropy
        } else {
            let probability = count as f32 / total as f32;
            entropy - probability * probability.ln()
        }
    })
}

fn next_random(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *state
}

fn random_unit(state: &mut u32) -> f32 {
    (next_random(state) >> 8) as f32 * (1.0 / 16_777_216.0)
}

fn random_signed(state: &mut u32) -> f32 {
    random_unit(state) * 2.0 - 1.0
}

fn mix_bits(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn unit_from_pair(first: usize, second: usize) -> Vec2 {
    let value = mix_bits(first as u32 ^ (second as u32).rotate_left(13));
    let angle = value as f32 * (TAU / u32::MAX as f32);
    Vec2::new(angle.cos(), angle.sin())
}

/// Boltzmann Field is intentionally fixed-rate. The host may wake more often,
/// but this scene only submits at its selected cadence.
pub struct BoltzmannScene {
    viewport: Viewport,
    preview: bool,
    simulation: HardDiskSimulation,
    last_elapsed_seconds: f32,
    last_rendered_seconds: f32,
    accumulator_seconds: f32,
}

impl BoltzmannScene {
    pub fn new(preview: bool, launch_seed: u32) -> Self {
        let requested_particle_count = if preview {
            PREVIEW_PARTICLE_COUNT
        } else {
            FULL_PARTICLE_COUNT
        };
        Self {
            viewport: Viewport {
                width: 1,
                height: 1,
            },
            preview,
            simulation: HardDiskSimulation::new(64.0, 64.0, requested_particle_count, launch_seed),
            last_elapsed_seconds: 0.0,
            last_rendered_seconds: -1.0,
            accumulator_seconds: 0.0,
        }
    }

    fn has_sidebar(&self) -> bool {
        !self.preview
            && self.viewport.width >= SIDEBAR_MINIMUM_WIDTH
            && self.viewport.height >= SIDEBAR_MINIMUM_HEIGHT
    }

    fn world_rect(&self) -> Rect {
        let width = self.viewport.width.max(1) as f32;
        let height = self.viewport.height.max(1) as f32;
        let margin = (width.min(height) * 0.022).clamp(12.0, 28.0);
        let world_width = if self.has_sidebar() {
            (width * 0.655 - margin * 1.5).max(1.0)
        } else {
            (width - margin * 2.0).max(1.0)
        };
        Rect {
            x: margin,
            y: margin,
            width: world_width,
            height: (height - margin * 2.0).max(1.0),
        }
    }

    fn sidebar_rect(&self, world: Rect) -> Option<Rect> {
        self.has_sidebar().then(|| {
            let width = self.viewport.width as f32;
            let margin = world.x;
            Rect {
                x: world.right() + margin,
                y: margin,
                width: (width - world.right() - margin * 2.0).max(1.0),
                height: world.height,
            }
        })
    }

    unsafe fn draw_background(&self, width: f32, height: f32) {
        unsafe {
            glClearColor(0.003, 0.008, 0.020, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            glBegin(GL_QUADS);
            glColor3f(0.004, 0.010, 0.026);
            glVertex2f(0.0, 0.0);
            glColor3f(0.010, 0.030, 0.066);
            glVertex2f(width, 0.0);
            glColor3f(0.004, 0.012, 0.032);
            glVertex2f(width, height);
            glColor3f(0.001, 0.004, 0.014);
            glVertex2f(0.0, height);
            glEnd();
        }
    }

    unsafe fn draw_panel(rect: Rect, accent: Color) {
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glColor4f(0.012, 0.035, 0.074, 0.80);
            glBegin(GL_QUADS);
            glVertex2f(rect.x, rect.y);
            glVertex2f(rect.right(), rect.y);
            glVertex2f(rect.right(), rect.bottom());
            glVertex2f(rect.x, rect.bottom());
            glEnd();
            glLineWidth(1.0);
            glColor4f(accent.red, accent.green, accent.blue, 0.35);
            glBegin(GL_LINE_LOOP);
            glVertex2f(rect.x, rect.y);
            glVertex2f(rect.right(), rect.y);
            glVertex2f(rect.right(), rect.bottom());
            glVertex2f(rect.x, rect.bottom());
            glEnd();
        }
    }

    unsafe fn draw_particle_field(&self, rect: Rect) {
        let simulation = &self.simulation;
        let radius = simulation.radius;
        unsafe {
            Self::draw_panel(rect, Color::new(0.20, 0.74, 0.94));
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glLineWidth(1.0);
            glBegin(GL_LINES);
            for particle in &simulation.particles {
                let color = velocity_color(particle.velocity, simulation.velocity_extent);
                let length = (particle.velocity.length() / simulation.speed_scale).clamp(0.2, 1.4)
                    * radius
                    * 3.2;
                let tail = particle.position
                    - particle.velocity / particle.velocity.length().max(1.0) * length;
                if tail.x > 0.0
                    && tail.x < simulation.width
                    && tail.y > 0.0
                    && tail.y < simulation.height
                {
                    glColor4f(color.red, color.green, color.blue, 0.16);
                    glVertex2f(rect.x + particle.position.x, rect.y + particle.position.y);
                    glColor4f(color.red, color.green, color.blue, 0.0);
                    glVertex2f(rect.x + tail.x, rect.y + tail.y);
                }
            }
            glEnd();

            // A fading segment marks the most recent collision links. It is a
            // deliberately local, visual echo of the collision histories that
            // the rigorous proof encodes as much richer combinatorial graphs.
            glLineWidth(1.0);
            glBegin(GL_LINES);
            for collision in &simulation.recent_collisions {
                let first = simulation.particles[collision.first];
                let second = simulation.particles[collision.second];
                let delta = second.position - first.position;
                if delta.x.abs() > simulation.width * 0.5 || delta.y.abs() > simulation.height * 0.5
                {
                    continue;
                }
                let opacity = (1.0 - collision.age_seconds / 4.0).powi(2) * 0.32;
                glColor4f(1.0, 0.78, 0.30, opacity);
                glVertex2f(rect.x + first.position.x, rect.y + first.position.y);
                glColor4f(1.0, 0.78, 0.30, 0.0);
                glVertex2f(rect.x + second.position.x, rect.y + second.position.y);
            }
            glEnd();

            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glPointSize((radius * 4.4).clamp(6.0, 34.0));
            glBegin(GL_POINTS);
            for particle in &simulation.particles {
                let color = velocity_color(particle.velocity, simulation.velocity_extent);
                glColor4f(
                    color.red,
                    color.green,
                    color.blue,
                    0.055 + particle.collision_flash * 0.12,
                );
                glVertex2f(rect.x + particle.position.x, rect.y + particle.position.y);
            }
            glEnd();

            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glBegin(GL_TRIANGLES);
            for particle in &simulation.particles {
                let color = velocity_color(particle.velocity, simulation.velocity_extent)
                    .mixed_with_white(particle.collision_flash * 0.62);
                emit_disk(
                    rect.x + particle.position.x,
                    rect.y + particle.position.y,
                    radius,
                    color,
                );
            }
            glEnd();
        }
    }

    unsafe fn draw_velocity_space(&self, rect: Rect) {
        let inner = rect.inset(8.0);
        let simulation = &self.simulation;
        let center = Vec2::new(inner.x + inner.width * 0.5, inner.y + inner.height * 0.5);
        unsafe {
            Self::draw_panel(rect, Color::new(0.36, 0.86, 0.96));
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glColor4f(0.44, 0.78, 0.90, 0.16);
            glBegin(GL_LINES);
            glVertex2f(inner.x, center.y);
            glVertex2f(inner.right(), center.y);
            glVertex2f(center.x, inner.y);
            glVertex2f(center.x, inner.bottom());
            glEnd();
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glPointSize(2.5);
            glBegin(GL_POINTS);
            for particle in &simulation.particles {
                let position = Vec2::new(
                    center.x
                        + particle.velocity.x / simulation.velocity_extent * inner.width * 0.46,
                    center.y
                        - particle.velocity.y / simulation.velocity_extent * inner.height * 0.46,
                );
                let color = velocity_color(particle.velocity, simulation.velocity_extent);
                glColor4f(color.red, color.green, color.blue, 0.62);
                glVertex2f(position.x, position.y);
            }
            glEnd();
        }
    }

    unsafe fn draw_speed_histogram(&self, rect: Rect) {
        let inner = rect.inset(8.0);
        let simulation = &self.simulation;
        let bins = simulation.speed_histogram();
        let particle_count = simulation.particles.len().max(1) as f32;
        let maximum = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
        let sigma_squared = (simulation.mean_speed_squared() * 0.5).max(1.0);
        unsafe {
            Self::draw_panel(rect, Color::new(0.42, 0.94, 0.69));
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glColor4f(0.42, 0.94, 0.69, 0.08);
            glBegin(GL_LINES);
            for index in 1..4 {
                let y = inner.bottom() - inner.height * index as f32 / 4.0;
                glVertex2f(inner.x, y);
                glVertex2f(inner.right(), y);
            }
            glEnd();
            let bar_width = inner.width / SPEED_BINS as f32;
            glBegin(GL_QUADS);
            for (index, count) in bins.iter().copied().enumerate() {
                let height = count as f32 / maximum * inner.height;
                let left = inner.x + index as f32 * bar_width + 1.0;
                let right = left + (bar_width - 2.0).max(1.0);
                glColor4f(0.24, 0.88, 0.62, 0.56);
                glVertex2f(left, inner.bottom());
                glVertex2f(right, inner.bottom());
                glColor4f(0.48, 1.0, 0.72, 0.84);
                glVertex2f(right, inner.bottom() - height);
                glVertex2f(left, inner.bottom() - height);
            }
            glEnd();

            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glLineWidth(1.5);
            glColor4f(1.0, 0.48, 0.72, 0.90);
            glBegin(GL_LINE_STRIP);
            for index in 0..=SPEED_BINS * 2 {
                let normalized_speed = index as f32 / (SPEED_BINS * 2) as f32;
                let speed = normalized_speed * simulation.velocity_extent;
                let bin_width = simulation.velocity_extent / SPEED_BINS as f32;
                let expected_count = particle_count * speed / sigma_squared
                    * (-(speed * speed) / (2.0 * sigma_squared)).exp()
                    * bin_width;
                let height = expected_count / maximum * inner.height;
                glVertex2f(
                    inner.x + normalized_speed * inner.width,
                    inner.bottom() - height,
                );
            }
            glEnd();
        }
    }

    unsafe fn draw_entropy_history(&self, rect: Rect) {
        let inner = rect.inset(8.0);
        let history = &self.simulation.entropy_history;
        unsafe {
            Self::draw_panel(rect, Color::new(1.0, 0.72, 0.30));
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glColor4f(1.0, 0.72, 0.30, 0.10);
            glBegin(GL_LINES);
            for index in 1..4 {
                let y = inner.bottom() - inner.height * index as f32 / 4.0;
                glVertex2f(inner.x, y);
                glVertex2f(inner.right(), y);
            }
            glEnd();
            if history.len() > 1 {
                glBlendFunc(GL_SRC_ALPHA, GL_ONE);
                glLineWidth(1.7);
                glColor4f(1.0, 0.72, 0.30, 0.92);
                glBegin(GL_LINE_STRIP);
                for sample in history {
                    let x = inner.x + sample.phase_seconds / PHASE_SECONDS * inner.width;
                    let y = inner.bottom() - sample.normalized_entropy * inner.height;
                    glVertex2f(x, y);
                }
                glEnd();
            }

            // The blue line is the exact-energy baseline. A hard-disk elastic
            // collision conserves kinetic energy, so an accurate simulation
            // should keep its cyan error marker very near the left edge.
            let energy_error = self.simulation.relative_energy_error();
            let marker_x = inner.x
                + inner.width * (1.0 + (energy_error.max(1.0e-9) / 1.0e-3).log10()).clamp(0.0, 1.0);
            let energy_y = inner.y + 5.0;
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glLineWidth(1.0);
            glColor4f(0.25, 0.75, 0.98, 0.56);
            glBegin(GL_LINES);
            glVertex2f(inner.x, energy_y);
            glVertex2f(inner.right(), energy_y);
            glEnd();
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glPointSize(4.5);
            glColor4f(0.50, 0.94, 1.0, 0.96);
            glBegin(GL_POINTS);
            glVertex2f(marker_x, energy_y);
            glEnd();
        }
    }

    unsafe fn draw_sidebar(&self, rect: Rect) {
        let gap = (rect.height * 0.024).clamp(7.0, 14.0);
        let velocity_height = rect.height * 0.355;
        let histogram_height = rect.height * 0.275;
        let entropy_height = rect.height - velocity_height - histogram_height - gap * 2.0;
        let velocity = Rect {
            height: velocity_height,
            ..rect
        };
        let histogram = Rect {
            y: velocity.bottom() + gap,
            height: histogram_height,
            ..rect
        };
        let entropy = Rect {
            y: histogram.bottom() + gap,
            height: entropy_height,
            ..rect
        };
        unsafe {
            self.draw_velocity_space(velocity);
            self.draw_speed_histogram(histogram);
            self.draw_entropy_history(entropy);
        }
    }
}

impl Scene for BoltzmannScene {
    unsafe fn activate(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        let world = self.world_rect();
        self.simulation.resize(world.width, world.height);
        self.accumulator_seconds = 0.0;
    }

    unsafe fn render(&mut self, frame: Frame) -> bool {
        let elapsed = (frame.elapsed_seconds - self.last_elapsed_seconds).clamp(0.0, 0.25);
        self.last_elapsed_seconds = frame.elapsed_seconds;
        self.accumulator_seconds += elapsed;
        let mut step_count = 0;
        while self.accumulator_seconds >= SIMULATION_STEP_SECONDS
            && step_count < MAXIMUM_STEPS_PER_RENDER
        {
            self.simulation.step(SIMULATION_STEP_SECONDS);
            self.accumulator_seconds -= SIMULATION_STEP_SECONDS;
            step_count += 1;
        }
        if step_count == MAXIMUM_STEPS_PER_RENDER {
            self.accumulator_seconds = 0.0;
        }

        let render_interval = if self.preview {
            PREVIEW_RENDER_INTERVAL_SECONDS
        } else {
            FULL_RENDER_INTERVAL_SECONDS
        };
        if frame.elapsed_seconds - self.last_rendered_seconds < render_interval {
            return false;
        }
        self.last_rendered_seconds = frame.elapsed_seconds;
        let width = self.viewport.width.max(1) as f32;
        let height = self.viewport.height.max(1) as f32;
        let world = self.world_rect();
        unsafe {
            glViewport(
                0,
                0,
                self.viewport.width.max(1),
                self.viewport.height.max(1),
            );
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();
            glOrtho(0.0, width as f64, height as f64, 0.0, -1.0, 1.0);
            glMatrixMode(GL_MODELVIEW);
            glLoadIdentity();
            glDisable(GL_DEPTH_TEST);
            glEnable(GL_BLEND);
            self.draw_background(width, height);
            self.draw_particle_field(world);
            if let Some(sidebar) = self.sidebar_rect(world) {
                self.draw_sidebar(sidebar);
            }
            glDisable(GL_BLEND);
        }
        true
    }

    unsafe fn prepare_present(&mut self, _frame: Frame) {}

    unsafe fn presented(&mut self, _frame: Frame, _succeeded: bool) {}

    unsafe fn deactivate(&mut self) {}
}

fn velocity_color(velocity: Vec2, extent: f32) -> Color {
    let hue = (velocity.y.atan2(velocity.x) / TAU).rem_euclid(1.0);
    let brightness = (0.52 + 0.34 * (velocity.length() / extent).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    hue_to_color(hue, 0.76, brightness)
}

fn hue_to_color(hue: f32, saturation: f32, lightness: f32) -> Color {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue * 6.0;
    let secondary = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match sector as i32 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let offset = lightness - chroma * 0.5;
    Color::new(red + offset, green + offset, blue + offset)
}

unsafe fn emit_disk(center_x: f32, center_y: f32, radius: f32, color: Color) {
    const SIDES: usize = 10;
    unsafe {
        for side in 0..SIDES {
            let first_angle = side as f32 * TAU / SIDES as f32;
            let second_angle = (side + 1) as f32 * TAU / SIDES as f32;
            glColor4f(color.red, color.green, color.blue, 0.91);
            glVertex2f(center_x, center_y);
            glColor4f(color.red, color.green, color.blue, 0.84);
            glVertex2f(
                center_x + first_angle.cos() * radius,
                center_y + first_angle.sin() * radius,
            );
            glVertex2f(
                center_x + second_angle.cos() * radius,
                center_y + second_angle.sin() * radius,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HardDiskSimulation, Particle, VELOCITY_BINS, Vec2, hue_to_color, shannon_entropy};

    const STEP: f32 = 1.0 / 60.0;

    #[test]
    fn head_on_equal_mass_collision_exchanges_normal_velocity() {
        let mut simulation = HardDiskSimulation::new(120.0, 80.0, 2, 9);
        simulation.radius = 4.0;
        simulation.particles = vec![
            Particle {
                position: Vec2::new(40.0, 40.0),
                velocity: Vec2::new(10.0, 0.0),
                collision_flash: 0.0,
            },
            Particle {
                position: Vec2::new(47.0, 40.0),
                velocity: Vec2::new(-10.0, 0.0),
                collision_flash: 0.0,
            },
        ];

        simulation.resolve_pair(0, 1);

        assert!((simulation.particles[0].velocity.x + 10.0).abs() < 1.0e-4);
        assert!((simulation.particles[1].velocity.x - 10.0).abs() < 1.0e-4);
        assert_eq!(simulation.collision_count, 1);
    }

    #[test]
    fn fixed_step_simulation_keeps_particles_finite_and_energy_stable() {
        let mut simulation = HardDiskSimulation::new(1_000.0, 640.0, 180, 0x1122_3344);
        let initial_energy = simulation.total_energy();
        for _ in 0..600 {
            simulation.step(STEP);
        }

        for particle in &simulation.particles {
            assert!(particle.position.x.is_finite() && particle.position.y.is_finite());
            assert!(particle.velocity.x.is_finite() && particle.velocity.y.is_finite());
            assert!((0.0..simulation.width).contains(&particle.position.x));
            assert!((0.0..simulation.height).contains(&particle.position.y));
        }
        let relative_error = ((simulation.total_energy() - initial_energy) / initial_energy).abs();
        assert!(
            relative_error < 2.0e-4,
            "relative energy error: {relative_error}"
        );
        assert!(simulation.collision_count > 0);
    }

    #[test]
    fn velocity_entropy_is_zero_for_one_bin_and_maximal_for_equal_bins() {
        assert_eq!(shannon_entropy(&[4, 0, 0]), 0.0);
        assert!((shannon_entropy(&[1, 1, 1, 1]) - 4.0_f32.ln()).abs() < 1.0e-6);

        let simulation = HardDiskSimulation::new(640.0, 480.0, 96, 7);
        assert!((0.0..=1.0).contains(&simulation.normalized_velocity_entropy()));
        assert_eq!(VELOCITY_BINS, 12);
    }

    #[test]
    fn hue_conversion_stays_in_the_displayable_range() {
        for index in 0..=24 {
            let color = hue_to_color(index as f32 / 24.0, 0.76, 0.70);
            assert!((0.0..=1.0).contains(&color.red));
            assert!((0.0..=1.0).contains(&color.green));
            assert!((0.0..=1.0).contains(&color.blue));
        }
    }
}
