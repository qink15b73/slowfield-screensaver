//! Versioned, platform-independent settings owned by Slowfield.
//!
//! Registry storage belongs to the Windows host. This module defines only the
//! validated product model so scene construction and migration rules stay
//! testable without Win32.

use crate::LaunchSelection;

pub const CONFIG_SCHEMA_VERSION: u32 = 2;

const PERCENT_MAXIMUM: u32 = 100;
const STAR_DENSITY_MINIMUM: u32 = 50;
const STAR_DENSITY_MAXIMUM: u32 = 200;
const SKY_FLOW_SPEED_MAXIMUM: u32 = 500;
const FLOCK_SIZE_MAXIMUM: i32 = 5_000;
const PREDATOR_COUNT_MAXIMUM: i32 = 20;
const BOID_SPEED_MINIMUM: f32 = 50.0;
const BOID_SPEED_MAXIMUM: f32 = 800.0;
const PREDATOR_SPEED_MULTIPLIER_MINIMUM: f32 = 1.0;
const PREDATOR_SPEED_MULTIPLIER_MAXIMUM: f32 = 3.0;
const WALL_TURN_DISTANCE_MAXIMUM: f32 = 500.0;
const WALL_FORCE_MULTIPLIER_MAXIMUM: f32 = 1.0;

/// All user-visible settings for the one Slowfield executable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlowfieldConfig {
    pub launch_selection: LaunchSelection,
    pub gravity: GravityFieldSettings,
    pub flock: FlockFieldSettings,
}

impl Default for SlowfieldConfig {
    fn default() -> Self {
        Self {
            launch_selection: LaunchSelection::Random,
            gravity: GravityFieldSettings::default(),
            flock: FlockFieldSettings::default(),
        }
    }
}

/// The legacy Gravity Field controls, represented as whole percentages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GravityFieldSettings {
    pub star_brightness: u32,
    pub disk_opacity: u32,
    pub doppler: u32,
    pub star_density: u32,
    pub sky_flow_speed: u32,
}

impl Default for GravityFieldSettings {
    fn default() -> Self {
        Self {
            star_brightness: 30,
            disk_opacity: 90,
            doppler: 60,
            star_density: 100,
            sky_flow_speed: 100,
        }
    }
}

impl GravityFieldSettings {
    pub const fn is_valid(self) -> bool {
        self.star_brightness <= PERCENT_MAXIMUM
            && self.disk_opacity <= PERCENT_MAXIMUM
            && self.doppler <= PERCENT_MAXIMUM
            && self.star_density >= STAR_DENSITY_MINIMUM
            && self.star_density <= STAR_DENSITY_MAXIMUM
            && self.sky_flow_speed <= SKY_FLOW_SPEED_MAXIMUM
    }
}

/// The controls that existed in the legacy Flock Field settings dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlockFieldSettings {
    pub solid_walls: bool,
    pub flock_size: i32,
    pub predator_count: i32,
    pub max_speed: f32,
    pub predator_speed_multiplier: f32,
    pub wall_turn_distance: f32,
    pub wall_force_multiplier: f32,
}

impl Default for FlockFieldSettings {
    fn default() -> Self {
        Self {
            solid_walls: true,
            flock_size: 1_200,
            predator_count: 3,
            max_speed: 340.0,
            predator_speed_multiplier: 1.4,
            wall_turn_distance: 100.0,
            wall_force_multiplier: 0.85,
        }
    }
}

impl FlockFieldSettings {
    pub fn is_valid(self) -> bool {
        (0..=FLOCK_SIZE_MAXIMUM).contains(&self.flock_size)
            && (0..=PREDATOR_COUNT_MAXIMUM).contains(&self.predator_count)
            && in_finite_range(self.max_speed, BOID_SPEED_MINIMUM, BOID_SPEED_MAXIMUM)
            && in_finite_range(
                self.predator_speed_multiplier,
                PREDATOR_SPEED_MULTIPLIER_MINIMUM,
                PREDATOR_SPEED_MULTIPLIER_MAXIMUM,
            )
            && in_finite_range(self.wall_turn_distance, 0.0, WALL_TURN_DISTANCE_MAXIMUM)
            && in_finite_range(
                self.wall_force_multiplier,
                0.0,
                WALL_FORCE_MULTIPLIER_MAXIMUM,
            )
    }
}

fn in_finite_range(value: f32, minimum: f32, maximum: f32) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::{FlockFieldSettings, GravityFieldSettings, SlowfieldConfig};

    #[test]
    fn defaults_match_the_reviewed_legacy_controls() {
        assert!(SlowfieldConfig::default().gravity.is_valid());
        assert!(SlowfieldConfig::default().flock.is_valid());
    }

    #[test]
    fn gravity_rejects_values_outside_its_legacy_ranges() {
        let invalid = GravityFieldSettings {
            star_density: 201,
            ..GravityFieldSettings::default()
        };
        assert!(!invalid.is_valid());
    }

    #[test]
    fn flock_rejects_non_finite_and_out_of_range_controls() {
        let invalid_speed = FlockFieldSettings {
            max_speed: f32::NAN,
            ..FlockFieldSettings::default()
        };
        let invalid_walls = FlockFieldSettings {
            wall_force_multiplier: 1.01,
            ..FlockFieldSettings::default()
        };
        assert!(!invalid_speed.is_valid());
        assert!(!invalid_walls.is_valid());
    }
}
