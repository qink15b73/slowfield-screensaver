//! Deterministic fixed-step flock simulation used by Flock Field.
//!
//! Rendering is deliberately separate. This module retains the behavioral
//! boundaries of the former standalone scene: prey use a bounded spatial-grid
//! query, predators retain exact short-list behavior, and all updates read one
//! stable frame before committing their next states together.

use std::f32::consts::TAU;

use windows_sys::Win32::Graphics::OpenGL::*;

use crate::scene::{Frame, Scene, Viewport};

const EPSILON: f32 = 1.0e-6;
const MINIMUM_STEP_SECONDS: f32 = 1.0e-5;
const MAXIMUM_STEP_SECONDS: f32 = 1.0 / 30.0;
const PREY_CELL_RANGE: i32 = 1;
const MAX_PREY_NEIGHBOR_CELLS: usize = 9;

pub const MINIMUM_FLOCK_SIZE: i32 = 0;
pub const MAXIMUM_FLOCK_SIZE: i32 = 5_000;
pub const MINIMUM_PREDATOR_COUNT: i32 = 0;
pub const MAXIMUM_PREDATOR_COUNT: i32 = 20;
pub const MINIMUM_BOID_SPEED: f32 = 50.0;
pub const MAXIMUM_BOID_SPEED: f32 = 800.0;
pub const MINIMUM_PREDATOR_SPEED_MULTIPLIER: f32 = 1.0;
pub const MAXIMUM_PREDATOR_SPEED_MULTIPLIER: f32 = 3.0;
pub const MINIMUM_WALL_TURN_DISTANCE: f32 = 0.0;
pub const MAXIMUM_WALL_TURN_DISTANCE: f32 = 500.0;
pub const MINIMUM_WALL_FORCE_MULTIPLIER: f32 = 0.0;
pub const MAXIMUM_WALL_FORCE_MULTIPLIER: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length_squared(self) -> f32 {
        self.x.mul_add(self.x, self.y * self.y)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    fn normalized(self) -> Self {
        let magnitude = self.length();
        if magnitude > EPSILON {
            self / magnitude
        } else {
            Self::default()
        }
    }

    fn limited(self, maximum: f32) -> Self {
        if self.length_squared() > maximum * maximum {
            self.normalized() * maximum
        } else {
            self
        }
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

impl std::ops::SubAssign for Vec2 {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        Self::new(self.x * scalar, self.y * scalar)
    }
}

impl std::ops::MulAssign<f32> for Vec2 {
    fn mul_assign(&mut self, scalar: f32) {
        *self = *self * scalar;
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Self;

    fn div(self, scalar: f32) -> Self {
        Self::new(self.x / scalar, self.y / scalar)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boid {
    pub position: Vec2,
    pub velocity: Vec2,
    wander_angle: f32,
    random_state: u32,
    pub is_predator: bool,
}

impl Default for Boid {
    fn default() -> Self {
        Self {
            position: Vec2::default(),
            velocity: Vec2::default(),
            wander_angle: 0.0,
            random_state: 1,
            is_predator: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlockSettings {
    pub initial_flock_size: i32,
    pub initial_predator_count: i32,
    pub boid_size: f32,
    pub max_speed: f32,
    pub max_force: f32,
    pub alignment_radius: f32,
    pub cohesion_radius: f32,
    pub separation_radius: f32,
    pub alignment_weight: f32,
    pub cohesion_weight: f32,
    pub separation_weight: f32,
    pub wander_weight: f32,
    pub wander_jitter: f32,
    pub max_sampled_prey_candidates: i32,
    pub predator_speed_multiplier: f32,
    pub predator_force_multiplier: f32,
    pub predator_perception_radius: f32,
    pub predator_pursuit_weight: f32,
    pub predator_separation_radius: f32,
    pub predator_separation_weight: f32,
    pub predator_wander_weight: f32,
    pub predator_avoidance_radius: f32,
    pub predator_avoidance_weight: f32,
    pub wall_turn_distance: f32,
    pub wall_force_multiplier: f32,
    pub wall_braking_safety_factor: f32,
    pub wrap_edges: bool,
}

impl Default for FlockSettings {
    fn default() -> Self {
        Self {
            initial_flock_size: 1_200,
            initial_predator_count: 3,
            boid_size: 6.0,
            max_speed: 340.0,
            max_force: 450.0,
            alignment_radius: 78.0,
            cohesion_radius: 110.0,
            separation_radius: 22.0,
            alignment_weight: 0.72,
            cohesion_weight: 0.58,
            separation_weight: 1.35,
            wander_weight: 0.22,
            wander_jitter: 1.5,
            max_sampled_prey_candidates: 96,
            predator_speed_multiplier: 1.4,
            predator_force_multiplier: 1.45,
            predator_perception_radius: 220.0,
            predator_pursuit_weight: 1.15,
            predator_separation_radius: 36.0,
            predator_separation_weight: 1.0,
            predator_wander_weight: 0.08,
            predator_avoidance_radius: 145.0,
            predator_avoidance_weight: 2.4,
            wall_turn_distance: 100.0,
            wall_force_multiplier: 0.85,
            wall_braking_safety_factor: 2.5,
            wrap_edges: false,
        }
    }
}

/// Applies the range and finite-value checks exposed by Flock Field's future
/// settings UI. Internal steering values intentionally remain untouched.
pub fn sanitize_control_panel_settings(mut settings: FlockSettings) -> FlockSettings {
    let defaults = FlockSettings::default();
    settings.initial_flock_size = settings
        .initial_flock_size
        .clamp(MINIMUM_FLOCK_SIZE, MAXIMUM_FLOCK_SIZE);
    settings.initial_predator_count = settings
        .initial_predator_count
        .clamp(MINIMUM_PREDATOR_COUNT, MAXIMUM_PREDATOR_COUNT);
    settings.max_speed = sanitize_finite(
        settings.max_speed,
        defaults.max_speed,
        MINIMUM_BOID_SPEED,
        MAXIMUM_BOID_SPEED,
    );
    settings.predator_speed_multiplier = sanitize_finite(
        settings.predator_speed_multiplier,
        defaults.predator_speed_multiplier,
        MINIMUM_PREDATOR_SPEED_MULTIPLIER,
        MAXIMUM_PREDATOR_SPEED_MULTIPLIER,
    );
    settings.wall_turn_distance = sanitize_finite(
        settings.wall_turn_distance,
        defaults.wall_turn_distance,
        MINIMUM_WALL_TURN_DISTANCE,
        MAXIMUM_WALL_TURN_DISTANCE,
    );
    settings.wall_force_multiplier = sanitize_finite(
        settings.wall_force_multiplier,
        defaults.wall_force_multiplier,
        MINIMUM_WALL_FORCE_MULTIPLIER,
        MAXIMUM_WALL_FORCE_MULTIPLIER,
    );
    settings
}

fn sanitize_finite(value: f32, fallback: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

pub struct FlockSimulation {
    width: f32,
    height: f32,
    settings: FlockSettings,
    seed: u32,
    step_index: u64,
    cell_size: f32,
    grid_width: i32,
    grid_height: i32,
    boids: Vec<Boid>,
    grid_cells: Vec<Vec<usize>>,
    predator_indices: Vec<usize>,
}

impl FlockSimulation {
    pub fn new(width: f32, height: f32, settings: FlockSettings, seed: u32) -> Self {
        let mut simulation = Self {
            width: width.max(1.0),
            height: height.max(1.0),
            settings,
            seed: if seed == 0 { 1 } else { seed },
            step_index: 0,
            cell_size: 1.0,
            grid_width: 1,
            grid_height: 1,
            boids: Vec::new(),
            grid_cells: Vec::new(),
            predator_indices: Vec::new(),
        };
        simulation.reset(simulation.settings.initial_flock_size);
        simulation
    }

    pub fn reset(&mut self, flock_size: i32) {
        let flock_size = flock_size.max(0) as usize;
        let predator_count = self.settings.initial_predator_count.max(0) as usize;
        let total_boid_count = flock_size + predator_count;
        self.boids.clear();
        self.boids.reserve(total_boid_count);
        let mut random_state = self.seed;
        for index in 0..total_boid_count {
            let is_predator = index >= flock_size;
            let position = self.constrain_position(Vec2::new(
                random_unit(&mut random_state) * self.width,
                random_unit(&mut random_state) * self.height,
            ));
            let mut boid = Boid {
                position,
                is_predator,
                random_state: next_random(&mut random_state) | 1,
                ..Boid::default()
            };
            boid.velocity = random_direction(&mut boid.random_state) * self.max_speed_for(boid);
            boid.wander_angle = random_unit(&mut boid.random_state) * TAU;
            self.boids.push(boid);
        }
        self.step_index = 0;
        self.resize_grid();
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width.max(1.0);
        self.height = height.max(1.0);
        for boid in &mut self.boids {
            boid.position = constrain_position_for(
                self.settings.wrap_edges,
                self.width,
                self.height,
                boid.position,
            );
        }
        self.resize_grid();
    }

    pub fn step(&mut self, seconds: f32) {
        if self.boids.is_empty() {
            return;
        }
        let delta_seconds = seconds.clamp(MINIMUM_STEP_SECONDS, MAXIMUM_STEP_SECONDS);
        self.rebuild_grid();
        let next_boids = (0..self.boids.len())
            .map(|index| self.step_boid(index, delta_seconds))
            .collect();
        self.boids = next_boids;
        self.step_index = self.step_index.wrapping_add(1);
    }

    pub fn boids(&self) -> &[Boid] {
        &self.boids
    }

    pub const fn settings(&self) -> FlockSettings {
        self.settings
    }

    fn resize_grid(&mut self) {
        self.cell_size = self
            .settings
            .alignment_radius
            .max(self.settings.cohesion_radius)
            .max(self.settings.separation_radius)
            .max(1.0);
        self.grid_width = (self.width / self.cell_size).ceil().max(1.0) as i32;
        self.grid_height = (self.height / self.cell_size).ceil().max(1.0) as i32;
        self.grid_cells = (0..(self.grid_width * self.grid_height))
            .map(|_| Vec::new())
            .collect();
        self.predator_indices.clear();
    }

    fn rebuild_grid(&mut self) {
        let required_cell_count = (self.grid_width * self.grid_height) as usize;
        if self.grid_cells.len() != required_cell_count {
            self.resize_grid();
        }
        for cell in &mut self.grid_cells {
            cell.clear();
        }
        self.predator_indices.clear();
        for (index, boid) in self.boids.iter().enumerate() {
            if boid.is_predator {
                self.predator_indices.push(index);
            } else {
                let cell_index = self.cell_index_for(boid.position);
                self.grid_cells[cell_index].push(index);
            }
        }
    }

    fn step_boid(&self, index: usize, delta_seconds: f32) -> Boid {
        let boid = self.boids[index];
        let mut average_velocity = Vec2::default();
        let mut average_offset = Vec2::default();
        let mut separation = Vec2::default();
        let mut predator_avoidance = Vec2::default();
        let mut closest_prey_offset = Vec2::default();
        let mut closest_prey_distance_squared = f32::INFINITY;
        let mut alignment_count = 0;
        let mut cohesion_count = 0;
        let mut separation_count = 0;
        let mut predator_separation_count = 0;
        let mut predator_avoidance_count = 0;

        if boid.is_predator {
            for neighbor in self
                .boids
                .iter()
                .copied()
                .filter(|candidate| !candidate.is_predator)
            {
                let offset = self.neighbor_offset(boid.position, neighbor.position);
                let distance_squared = offset.length_squared();
                if distance_squared > EPSILON
                    && distance_squared <= self.settings.predator_perception_radius.powi(2)
                    && distance_squared < closest_prey_distance_squared
                {
                    closest_prey_offset = offset;
                    closest_prey_distance_squared = distance_squared;
                }
            }
            for other_index in &self.predator_indices {
                if *other_index == index {
                    continue;
                }
                let offset = self.neighbor_offset(boid.position, self.boids[*other_index].position);
                let distance_squared = offset.length_squared();
                if distance_squared > EPSILON
                    && distance_squared <= self.settings.predator_separation_radius.powi(2)
                {
                    separation -= offset / distance_squared;
                    predator_separation_count += 1;
                }
            }
        } else {
            let (alignment, cohesion, separation_sum, counts) = self.prey_neighbors(index, boid);
            average_velocity = alignment;
            average_offset = cohesion;
            separation = separation_sum;
            alignment_count = counts.alignment;
            cohesion_count = counts.cohesion;
            separation_count = counts.separation;
            for predator_index in &self.predator_indices {
                let offset =
                    self.neighbor_offset(boid.position, self.boids[*predator_index].position);
                let distance_squared = offset.length_squared();
                if distance_squared > EPSILON
                    && distance_squared <= self.settings.predator_avoidance_radius.powi(2)
                {
                    predator_avoidance -= offset / distance_squared;
                    predator_avoidance_count += 1;
                }
            }
        }

        let mut steering = Vec2::default();
        if boid.is_predator {
            if closest_prey_distance_squared.is_finite() {
                steering += self.steer_toward(boid, closest_prey_offset)
                    * self.settings.predator_pursuit_weight;
            }
            if predator_separation_count > 0 {
                steering += self.steer_toward(boid, separation / predator_separation_count as f32)
                    * self.settings.predator_separation_weight;
            }
        } else {
            if alignment_count > 0 {
                steering += self.steer_toward(boid, average_velocity / alignment_count as f32)
                    * self.settings.alignment_weight;
            }
            if cohesion_count > 0 {
                steering += self.steer_toward(boid, average_offset / cohesion_count as f32)
                    * self.settings.cohesion_weight;
            }
            if separation_count > 0 {
                steering += self.steer_toward(boid, separation / separation_count as f32)
                    * self.settings.separation_weight;
            }
            if predator_avoidance_count > 0 {
                steering += self
                    .steer_toward(boid, predator_avoidance / predator_avoidance_count as f32)
                    * self.settings.predator_avoidance_weight;
            }
        }

        let mut next = boid;
        next.wander_angle +=
            random_signed(&mut next.random_state) * self.settings.wander_jitter * delta_seconds;
        let forward = boid.velocity.normalized();
        let wander_direction = Vec2::new(
            next.wander_angle.cos() * 0.7 + forward.x,
            next.wander_angle.sin() * 0.7 + forward.y,
        );
        let wander_weight = if boid.is_predator {
            self.settings.predator_wander_weight
        } else {
            self.settings.wander_weight
        };
        steering += self.steer_toward(boid, wander_direction) * wander_weight;

        let wall_steering = self.wall_avoidance(boid);
        let wall_priority =
            (wall_steering.length() / self.max_force_for(boid).max(EPSILON)).clamp(0.0, 1.0);
        steering *= 1.0 - wall_priority;
        steering += wall_steering;
        steering = steering.limited(self.max_force_for(boid));
        next.velocity =
            (boid.velocity + steering * delta_seconds).limited(self.max_speed_for(boid));
        next.position = self.constrain_position(boid.position + next.velocity * delta_seconds);
        self.constrain_motion(&mut next);
        next
    }

    fn prey_neighbors(&self, index: usize, boid: Boid) -> (Vec2, Vec2, Vec2, NeighborCounts) {
        let center_cell_x =
            ((boid.position.x / self.cell_size) as i32).clamp(0, self.grid_width - 1);
        let center_cell_y =
            ((boid.position.y / self.cell_size) as i32).clamp(0, self.grid_height - 1);
        let cells_to_scan_x = if self.settings.wrap_edges {
            self.grid_width.min(PREY_CELL_RANGE * 2 + 1)
        } else {
            PREY_CELL_RANGE * 2 + 1
        };
        let cells_to_scan_y = if self.settings.wrap_edges {
            self.grid_height.min(PREY_CELL_RANGE * 2 + 1)
        } else {
            PREY_CELL_RANGE * 2 + 1
        };
        let scanned_cell_count = cells_to_scan_x * cells_to_scan_y;
        let sampling_seed = mix_random(boid.random_state ^ self.step_index as u32 ^ 0x9e37_79b9);
        let first_cell_offset = sampling_seed as i32 % scanned_cell_count;
        let mut cells = Vec::with_capacity(MAX_PREY_NEIGHBOR_CELLS);
        for cell_order in 0..scanned_cell_count {
            let rotated_cell_offset = (first_cell_offset + cell_order) % scanned_cell_count;
            let mut scanned_cell_y =
                center_cell_y - PREY_CELL_RANGE + rotated_cell_offset / cells_to_scan_x;
            let mut scanned_cell_x =
                center_cell_x - PREY_CELL_RANGE + rotated_cell_offset % cells_to_scan_x;
            if self.settings.wrap_edges {
                scanned_cell_y = scanned_cell_y.rem_euclid(self.grid_height);
                scanned_cell_x = scanned_cell_x.rem_euclid(self.grid_width);
            } else if scanned_cell_y < 0
                || scanned_cell_y >= self.grid_height
                || scanned_cell_x < 0
                || scanned_cell_x >= self.grid_width
            {
                continue;
            }
            let cell_index = (scanned_cell_y * self.grid_width + scanned_cell_x) as usize;
            let cell = &self.grid_cells[cell_index];
            if cell.is_empty() {
                continue;
            }
            let start = if cell.len() > 1 {
                mix_random(sampling_seed ^ rotated_cell_offset as u32) as usize % cell.len()
            } else {
                0
            };
            cells.push(CandidateCell {
                grid_index: cell_index,
                start,
                offset: 0,
            });
        }

        let query_radius_squared = self
            .settings
            .alignment_radius
            .max(self.settings.cohesion_radius)
            .max(self.settings.separation_radius)
            .powi(2);
        let mut alignment = Vec2::default();
        let mut cohesion = Vec2::default();
        let mut separation = Vec2::default();
        let mut counts = NeighborCounts::default();
        let maximum_candidates = self.settings.max_sampled_prey_candidates.max(0) as usize;
        let mut sampled_candidate_count = 0;
        let mut current_cell = 0;
        while !cells.is_empty()
            && (maximum_candidates == 0 || sampled_candidate_count < maximum_candidates)
        {
            let candidate = &mut cells[current_cell];
            let cell = &self.grid_cells[candidate.grid_index];
            if candidate.offset == cell.len() {
                cells.swap_remove(current_cell);
                if cells.is_empty() {
                    break;
                }
                current_cell %= cells.len();
                continue;
            }
            let neighbor_index = cell[(candidate.start + candidate.offset) % cell.len()];
            candidate.offset += 1;
            if neighbor_index != index {
                sampled_candidate_count += 1;
                let neighbor = self.boids[neighbor_index];
                let offset = self.neighbor_offset(boid.position, neighbor.position);
                let distance_squared = offset.length_squared();
                if distance_squared > EPSILON && distance_squared <= query_radius_squared {
                    if distance_squared <= self.settings.alignment_radius.powi(2) {
                        alignment += neighbor.velocity;
                        counts.alignment += 1;
                    }
                    if distance_squared <= self.settings.cohesion_radius.powi(2) {
                        cohesion += offset;
                        counts.cohesion += 1;
                    }
                    if distance_squared <= self.settings.separation_radius.powi(2) {
                        separation -= offset / distance_squared;
                        counts.separation += 1;
                    }
                }
            }
            current_cell = (current_cell + 1) % cells.len();
        }
        (alignment, cohesion, separation, counts)
    }

    fn cell_index_for(&self, position: Vec2) -> usize {
        let cell_x = ((position.x / self.cell_size) as i32).clamp(0, self.grid_width - 1);
        let cell_y = ((position.y / self.cell_size) as i32).clamp(0, self.grid_height - 1);
        (cell_y * self.grid_width + cell_x) as usize
    }

    fn neighbor_offset(&self, from: Vec2, to: Vec2) -> Vec2 {
        let mut offset = to - from;
        if self.settings.wrap_edges {
            if offset.x > self.width * 0.5 {
                offset.x -= self.width;
            } else if offset.x < -self.width * 0.5 {
                offset.x += self.width;
            }
            if offset.y > self.height * 0.5 {
                offset.y -= self.height;
            } else if offset.y < -self.height * 0.5 {
                offset.y += self.height;
            }
        }
        offset
    }

    fn max_speed_for(&self, boid: Boid) -> f32 {
        self.settings.max_speed
            * if boid.is_predator {
                self.settings.predator_speed_multiplier
            } else {
                1.0
            }
    }

    fn max_force_for(&self, boid: Boid) -> f32 {
        self.settings.max_force
            * if boid.is_predator {
                self.settings.predator_force_multiplier
            } else {
                1.0
            }
    }

    fn steer_toward(&self, boid: Boid, desired: Vec2) -> Vec2 {
        if desired.length_squared() <= EPSILON {
            Vec2::default()
        } else {
            (desired.normalized() * self.max_speed_for(boid) - boid.velocity)
                .limited(self.max_force_for(boid))
        }
    }

    fn wall_avoidance(&self, boid: Boid) -> Vec2 {
        if self.settings.wrap_edges {
            return Vec2::default();
        }
        let minimum_turn_distance = self.settings.wall_turn_distance.max(1.0);
        let braking_force = self.max_force_for(boid).max(EPSILON);
        let maximum_wall_force =
            self.max_force_for(boid) * self.settings.wall_force_multiplier.clamp(0.0, 1.0);
        let safety_factor = self.settings.wall_braking_safety_factor.max(0.0);
        let mut steering = Vec2::default();
        add_wall_force(
            &mut steering,
            boid.position.x,
            -boid.velocity.x,
            Vec2::new(1.0, 0.0),
            minimum_turn_distance,
            braking_force,
            safety_factor,
        );
        add_wall_force(
            &mut steering,
            self.width - boid.position.x,
            boid.velocity.x,
            Vec2::new(-1.0, 0.0),
            minimum_turn_distance,
            braking_force,
            safety_factor,
        );
        add_wall_force(
            &mut steering,
            boid.position.y,
            -boid.velocity.y,
            Vec2::new(0.0, 1.0),
            minimum_turn_distance,
            braking_force,
            safety_factor,
        );
        add_wall_force(
            &mut steering,
            self.height - boid.position.y,
            boid.velocity.y,
            Vec2::new(0.0, -1.0),
            minimum_turn_distance,
            braking_force,
            safety_factor,
        );
        if steering.length_squared() <= EPSILON {
            Vec2::default()
        } else {
            steering.limited(1.0) * maximum_wall_force
        }
    }

    fn constrain_position(&self, position: Vec2) -> Vec2 {
        constrain_position_for(self.settings.wrap_edges, self.width, self.height, position)
    }

    fn constrain_motion(&self, boid: &mut Boid) {
        if self.settings.wrap_edges {
            return;
        }
        let maximum_x = below(self.width);
        let maximum_y = below(self.height);
        if (boid.position.x <= 0.0 && boid.velocity.x < 0.0)
            || (boid.position.x >= maximum_x && boid.velocity.x > 0.0)
        {
            boid.velocity.x = -boid.velocity.x;
        }
        if (boid.position.y <= 0.0 && boid.velocity.y < 0.0)
            || (boid.position.y >= maximum_y && boid.velocity.y > 0.0)
        {
            boid.velocity.y = -boid.velocity.y;
        }
    }
}

#[derive(Default)]
struct NeighborCounts {
    alignment: i32,
    cohesion: i32,
    separation: i32,
}

struct CandidateCell {
    grid_index: usize,
    start: usize,
    offset: usize,
}

fn add_wall_force(
    steering: &mut Vec2,
    distance: f32,
    outward_speed: f32,
    inward: Vec2,
    minimum_turn_distance: f32,
    braking_force: f32,
    safety_factor: f32,
) {
    let braking_distance = outward_speed.max(0.0).powi(2) / (2.0 * braking_force);
    let turn_distance = minimum_turn_distance + braking_distance * safety_factor;
    if distance >= turn_distance {
        return;
    }
    let proximity = (1.0 - distance / turn_distance).clamp(0.0, 1.0);
    *steering += inward * proximity.powi(3);
}

fn constrain_position_for(wrap_edges: bool, width: f32, height: f32, mut position: Vec2) -> Vec2 {
    if wrap_edges {
        position.x = wrap(position.x, width);
        position.y = wrap(position.y, height);
    } else {
        position.x = position.x.clamp(0.0, below(width));
        position.y = position.y.clamp(0.0, below(height));
    }
    position
}

fn below(value: f32) -> f32 {
    f32::from_bits(value.to_bits() - 1)
}

fn wrap(value: f32, maximum: f32) -> f32 {
    let value = value % maximum;
    if value < 0.0 { value + maximum } else { value }
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

fn random_direction(state: &mut u32) -> Vec2 {
    let angle = random_unit(state) * TAU;
    Vec2::new(angle.cos(), angle.sin())
}

fn mix_random(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

/// Flock Field owns a fixed 60 Hz simulation clock while the Slowfield host
/// remains responsible for waking and presenting the OpenGL window.
pub struct FlockScene {
    viewport: Viewport,
    simulation: FlockSimulation,
    last_elapsed_seconds: f32,
    accumulator_seconds: f32,
}

impl FlockScene {
    pub fn new(preview: bool) -> Self {
        let mut settings = FlockSettings::default();
        if preview {
            // The small Control Panel preview stays bounded regardless of the
            // later persisted full-screen population setting.
            settings.initial_flock_size = 120;
            settings.boid_size = 2.5;
        }
        let settings = sanitize_control_panel_settings(settings);
        Self {
            viewport: Viewport {
                width: 1,
                height: 1,
            },
            simulation: FlockSimulation::new(1.0, 1.0, settings, 0x6f0c_1e1d),
            last_elapsed_seconds: 0.0,
            accumulator_seconds: 0.0,
        }
    }

    unsafe fn draw_boid(boid: Boid, size: f32) {
        let heading = boid.velocity.y.atan2(boid.velocity.x);
        let side_angle = 2.45;
        unsafe {
            glVertex2f(
                boid.position.x + heading.cos() * size,
                boid.position.y + heading.sin() * size,
            );
            glVertex2f(
                boid.position.x + (heading + side_angle).cos() * size * 0.78,
                boid.position.y + (heading + side_angle).sin() * size * 0.78,
            );
            glVertex2f(
                boid.position.x + (heading - side_angle).cos() * size * 0.78,
                boid.position.y + (heading - side_angle).sin() * size * 0.78,
            );
        }
    }
}

impl Scene for FlockScene {
    unsafe fn activate(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.simulation
            .resize(viewport.width.max(1) as f32, viewport.height.max(1) as f32);
    }

    unsafe fn render(&mut self, frame: Frame) -> bool {
        let elapsed = (frame.elapsed_seconds - self.last_elapsed_seconds).clamp(0.0, 0.25);
        self.last_elapsed_seconds = frame.elapsed_seconds;
        self.accumulator_seconds += elapsed;
        while self.accumulator_seconds >= 1.0 / 60.0 {
            self.simulation.step(1.0 / 60.0);
            self.accumulator_seconds -= 1.0 / 60.0;
        }
        let width = self.viewport.width as f64;
        let height = self.viewport.height as f64;
        unsafe {
            glViewport(0, 0, self.viewport.width, self.viewport.height);
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();
            glOrtho(0.0, width, height, 0.0, -1.0, 1.0);
            glMatrixMode(GL_MODELVIEW);
            glLoadIdentity();
            glDisable(GL_BLEND);
            glClearColor(0.01, 0.02, 0.05, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            let settings = self.simulation.settings();
            glBegin(GL_TRIANGLES);
            for boid in self.simulation.boids().iter().copied() {
                if boid.is_predator {
                    glColor3f(0.96, 0.20, 0.16);
                    Self::draw_boid(boid, settings.boid_size * 1.65);
                } else {
                    let speed_ratio = (boid.velocity.length() / settings.max_speed).min(1.0);
                    glColor3f(0.20 + 0.25 * speed_ratio, 0.66 + 0.28 * speed_ratio, 0.95);
                    Self::draw_boid(boid, settings.boid_size);
                }
            }
            glEnd();
        }
        true
    }

    unsafe fn prepare_present(&mut self, _frame: Frame) {}

    unsafe fn presented(&mut self, _frame: Frame, _succeeded: bool) {}

    unsafe fn deactivate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::{
        FlockSettings, FlockSimulation, MAXIMUM_PREDATOR_COUNT, MINIMUM_FLOCK_SIZE, Vec2,
        sanitize_control_panel_settings,
    };

    const STEP: f32 = 1.0 / 60.0;

    fn predator_count(simulation: &FlockSimulation) -> usize {
        simulation
            .boids()
            .iter()
            .filter(|boid| boid.is_predator)
            .count()
    }

    fn assert_valid(simulation: &FlockSimulation) {
        let settings = simulation.settings();
        for boid in simulation.boids() {
            let speed_limit = settings.max_speed
                * if boid.is_predator {
                    settings.predator_speed_multiplier
                } else {
                    1.0
                };
            assert!(boid.position.x.is_finite() && boid.position.y.is_finite());
            assert!(boid.velocity.x.is_finite() && boid.velocity.y.is_finite());
            assert!((0.0..simulation.width).contains(&boid.position.x));
            assert!((0.0..simulation.height).contains(&boid.position.y));
            assert!(boid.velocity.length() <= speed_limit + 1.0e-3);
        }
    }

    #[test]
    fn fixed_step_evolution_is_deterministic() {
        let settings = FlockSettings {
            initial_flock_size: 96,
            ..FlockSettings::default()
        };
        let mut first = FlockSimulation::new(800.0, 600.0, settings, 0x1234_5678);
        let mut second = FlockSimulation::new(800.0, 600.0, settings, 0x1234_5678);
        for _ in 0..120 {
            first.step(STEP);
            second.step(STEP);
        }
        assert_eq!(first.boids(), second.boids());
        assert_eq!(predator_count(&first), 3);
        assert_valid(&first);
    }

    #[test]
    fn predator_population_and_speed_limits_are_preserved() {
        let settings = FlockSettings {
            initial_flock_size: 64,
            initial_predator_count: 3,
            ..FlockSettings::default()
        };
        let mut simulation = FlockSimulation::new(800.0, 600.0, settings, 0xf00d_face);
        assert_eq!(simulation.boids().len(), 67);
        assert_eq!(predator_count(&simulation), 3);
        for boid in simulation.boids() {
            let expected_speed = settings.max_speed
                * if boid.is_predator {
                    settings.predator_speed_multiplier
                } else {
                    1.0
                };
            assert!((boid.velocity.length() - expected_speed).abs() < 1.0e-3);
        }
        for _ in 0..180 {
            simulation.step(STEP);
        }
        assert_eq!(predator_count(&simulation), 3);
        assert_valid(&simulation);
    }

    #[test]
    fn solid_walls_keep_boids_inside_the_physical_world() {
        let settings = FlockSettings {
            initial_flock_size: 96,
            ..FlockSettings::default()
        };
        let mut simulation = FlockSimulation::new(320.0, 180.0, settings, 0xb011_dab1);
        for _ in 0..240 {
            simulation.step(STEP);
            assert_valid(&simulation);
        }
    }

    #[test]
    fn dense_capped_wrapped_flock_remains_deterministic() {
        let settings = FlockSettings {
            initial_flock_size: 96,
            max_sampled_prey_candidates: 16,
            wrap_edges: true,
            ..FlockSettings::default()
        };
        let mut first = FlockSimulation::new(48.0, 48.0, settings, 0xd3e5_e123);
        let mut second = FlockSimulation::new(48.0, 48.0, settings, 0xd3e5_e123);
        for _ in 0..120 {
            first.step(STEP);
            second.step(STEP);
        }
        assert_eq!(first.boids(), second.boids());
        assert_valid(&first);
    }

    #[test]
    fn wrapped_small_grids_do_not_scan_one_cell_more_than_once() {
        let settings = FlockSettings {
            wrap_edges: true,
            initial_predator_count: 0,
            max_sampled_prey_candidates: 96,
            ..FlockSettings::default()
        };
        let mut simulation = FlockSimulation::new(120.0, 120.0, settings, 0x52);
        for _ in 0..1_000 {
            simulation.step(STEP);
            assert_eq!(
                simulation.boids().len(),
                settings.initial_flock_size as usize
            );
        }
    }

    #[test]
    fn settings_limit_public_controls_without_changing_other_values() {
        let sanitized = sanitize_control_panel_settings(FlockSettings {
            initial_flock_size: -1,
            initial_predator_count: 999,
            max_speed: f32::NAN,
            predator_speed_multiplier: 99.0,
            wall_turn_distance: -5.0,
            wall_force_multiplier: -1.0,
            ..FlockSettings::default()
        });
        assert_eq!(sanitized.initial_flock_size, MINIMUM_FLOCK_SIZE);
        assert_eq!(sanitized.initial_predator_count, MAXIMUM_PREDATOR_COUNT);
        assert_eq!(sanitized.max_speed, FlockSettings::default().max_speed);
        assert_eq!(sanitized.predator_speed_multiplier, 3.0);
        assert_eq!(sanitized.wall_turn_distance, 0.0);
        assert_eq!(sanitized.wall_force_multiplier, 0.0);
    }

    #[test]
    fn resizing_and_empty_flocks_are_safe() {
        let settings = FlockSettings {
            initial_flock_size: 0,
            initial_predator_count: 0,
            ..FlockSettings::default()
        };
        let mut simulation = FlockSimulation::new(400.0, 300.0, settings, 17);
        simulation.step(STEP);
        assert!(simulation.boids().is_empty());
        simulation.reset(32);
        simulation.resize(96.0, 64.0);
        for _ in 0..60 {
            simulation.step(STEP);
        }
        assert_valid(&simulation);
    }

    #[test]
    fn vector_length_is_euclidean() {
        assert_eq!(Vec2 { x: 3.0, y: 4.0 }.length(), 5.0);
    }
}
