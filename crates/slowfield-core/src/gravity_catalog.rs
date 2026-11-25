//! Deterministic choreography for Gravity Field's bounded scene catalogue.
//!
//! The renderer is intentionally separate. These values are its immutable
//! host-owned composition decisions: one descriptor starts every 45 seconds,
//! and the cyclic catalogue visits every approved combination exactly once.

pub const GRAVITY_SCENE_DURATION_SECONDS: f32 = 45.0;
const INCLINATION_COUNT: usize = 4;
const POSITION_COUNT: usize = 9;
const APPARENT_RADIUS_COUNT: usize = 3;
const SKY_DIRECTION_COUNT: usize = 8;
pub const GRAVITY_SCENE_COUNT: usize =
    INCLINATION_COUNT * POSITION_COUNT * APPARENT_RADIUS_COUNT * SKY_DIRECTION_COUNT;

const INCLINATIONS: [f32; INCLINATION_COUNT] = [
    0.0,
    std::f32::consts::FRAC_PI_8,
    std::f32::consts::FRAC_PI_4,
    std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_8,
];
const POSITIONS: [(f32, f32); POSITION_COUNT] = [
    (0.33, 0.33),
    (0.50, 0.33),
    (0.66, 0.33),
    (0.33, 0.50),
    (0.50, 0.50),
    (0.66, 0.50),
    (0.33, 0.66),
    (0.50, 0.66),
    (0.66, 0.66),
];
const APPARENT_RADII: [f32; APPARENT_RADIUS_COUNT] = [0.1000, 0.1175, 0.1350];
const SKY_FLOW_DIRECTIONS: [(f32, f32); SKY_DIRECTION_COUNT] = [
    (1.000_000_0, 0.000_000_0),
    (
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    (0.000_000_0, 1.000_000_0),
    (
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    (-1.000_000_0, 0.000_000_0),
    (
        -std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    ),
    (0.000_000_0, -1.000_000_0),
    (
        std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    ),
];
const INCLINATION_PHASE_BY_DIRECTION: [usize; SKY_DIRECTION_COUNT] = [0, 1, 2, 3, 0, 1, 2, 3];
const POSITION_PHASE_BY_DIRECTION: [usize; SKY_DIRECTION_COUNT] = [0, 2, 3, 4, 5, 6, 7, 8];
const RADIUS_PHASE_BY_DIRECTION: [usize; SKY_DIRECTION_COUNT] = [0, 2, 0, 1, 2, 0, 1, 2];

/// One immutable Gravity Field composition. The `catalog_index` is retained as
/// a stable identity for deriving scene-local procedural seeds later.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GravitySceneDescriptor {
    pub catalog_index: usize,
    pub inclination: f32,
    pub center: (f32, f32),
    pub apparent_radius: f32,
    pub sky_flow_direction: (f32, f32),
}

/// Returns a descriptor from the cyclic, finite catalogue. Values beyond the
/// catalogue length wrap so a long-running saver has deterministic behavior.
pub fn gravity_scene_descriptor(catalog_index: usize) -> GravitySceneDescriptor {
    let catalog_index = catalog_index % GRAVITY_SCENE_COUNT;
    let direction_cycle = catalog_index % SKY_DIRECTION_COUNT;
    let remaining = catalog_index / SKY_DIRECTION_COUNT;
    let radius_cycle = remaining % APPARENT_RADIUS_COUNT;
    let remaining = remaining / APPARENT_RADIUS_COUNT;
    let position_cycle = remaining % POSITION_COUNT;
    let inclination_cycle = remaining / POSITION_COUNT;
    let inclination_index =
        (inclination_cycle + INCLINATION_PHASE_BY_DIRECTION[direction_cycle]) % INCLINATION_COUNT;
    let position_index =
        (position_cycle + POSITION_PHASE_BY_DIRECTION[direction_cycle]) % POSITION_COUNT;
    let apparent_radius_index =
        (radius_cycle + RADIUS_PHASE_BY_DIRECTION[direction_cycle]) % APPARENT_RADIUS_COUNT;
    GravitySceneDescriptor {
        catalog_index,
        inclination: INCLINATIONS[inclination_index],
        center: POSITIONS[position_index],
        apparent_radius: APPARENT_RADII[apparent_radius_index],
        sky_flow_direction: SKY_FLOW_DIRECTIONS[direction_cycle],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{GRAVITY_SCENE_COUNT, gravity_scene_descriptor};

    #[test]
    fn catalogue_contains_every_approved_combination_once() {
        let mut descriptors = HashSet::new();
        for index in 0..GRAVITY_SCENE_COUNT {
            let descriptor = gravity_scene_descriptor(index);
            descriptors.insert((
                descriptor.inclination.to_bits(),
                descriptor.center.0.to_bits(),
                descriptor.center.1.to_bits(),
                descriptor.apparent_radius.to_bits(),
                descriptor.sky_flow_direction.0.to_bits(),
                descriptor.sky_flow_direction.1.to_bits(),
            ));
        }
        assert_eq!(descriptors.len(), GRAVITY_SCENE_COUNT);
    }

    #[test]
    fn each_adjacent_catalogue_entry_changes_every_composition_dimension() {
        for index in 0..GRAVITY_SCENE_COUNT {
            let current = gravity_scene_descriptor(index);
            let next = gravity_scene_descriptor(index + 1);
            assert_ne!(current.inclination, next.inclination);
            assert_ne!(current.center, next.center);
            assert_ne!(current.apparent_radius, next.apparent_radius);
            assert_ne!(current.sky_flow_direction, next.sky_flow_direction);
        }
    }

    #[test]
    fn descriptor_sequence_wraps_at_the_catalogue_boundary() {
        assert_eq!(
            gravity_scene_descriptor(0),
            gravity_scene_descriptor(GRAVITY_SCENE_COUNT)
        );
    }
}
