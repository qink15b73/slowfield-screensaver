use std::fmt;

/// A visual system that can be hosted by Slowfield.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneId {
    Prime,
    Gravity,
    Flock,
    Boltzmann,
}

impl SceneId {
    pub const ALL: [Self; 4] = [Self::Prime, Self::Gravity, Self::Flock, Self::Boltzmann];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Prime => "Prime Field",
            Self::Gravity => "Gravity Field",
            Self::Flock => "Flock Field",
            Self::Boltzmann => "Boltzmann Field",
        }
    }

    pub const fn storage_value(self) -> u32 {
        match self {
            Self::Prime => 1,
            Self::Gravity => 2,
            Self::Flock => 3,
            Self::Boltzmann => 4,
        }
    }

    pub const fn from_storage(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Prime),
            2 => Some(Self::Gravity),
            3 => Some(Self::Flock),
            4 => Some(Self::Boltzmann),
            _ => None,
        }
    }
}

/// The persisted user intent for choosing the field at process startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchSelection {
    Random,
    Fixed(SceneId),
}

impl LaunchSelection {
    /// `0` is deliberately random so an uninitialized registry DWORD and an
    /// explicit default have identical, safe behavior. Unknown values return
    /// `None` so the configuration layer can reject a partial/future schema.
    pub const fn from_storage(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Random),
            value => match SceneId::from_storage(value) {
                Some(scene) => Some(Self::Fixed(scene)),
                None => None,
            },
        }
    }

    pub const fn storage_value(self) -> u32 {
        match self {
            Self::Random => 0,
            Self::Fixed(scene) => scene.storage_value(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSelectionError {
    NoAvailableScenes,
    RequestedSceneUnavailable(SceneId),
}

impl fmt::Display for SceneSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAvailableScenes => formatter.write_str("no scenes are available"),
            Self::RequestedSceneUnavailable(scene) => {
                write!(formatter, "{} is not available", scene.display_name())
            }
        }
    }
}

impl std::error::Error for SceneSelectionError {}

/// Selects a scene without owning random-number generation. The host supplies
/// `random_value`, keeping the policy deterministic and easy to test.
pub fn select_scene(
    preference: LaunchSelection,
    available: &[SceneId],
    random_value: u64,
) -> Result<SceneId, SceneSelectionError> {
    match preference {
        LaunchSelection::Fixed(scene) if available.contains(&scene) => Ok(scene),
        LaunchSelection::Fixed(scene) => Err(SceneSelectionError::RequestedSceneUnavailable(scene)),
        LaunchSelection::Random => available
            .get((random_value as usize) % available.len().max(1))
            .copied()
            .ok_or(SceneSelectionError::NoAvailableScenes),
    }
}

#[cfg(test)]
mod tests {
    use super::{LaunchSelection, SceneId, SceneSelectionError, select_scene};

    #[test]
    fn fixed_selection_uses_the_requested_available_scene() {
        assert_eq!(
            select_scene(
                LaunchSelection::Fixed(SceneId::Gravity),
                &[SceneId::Prime, SceneId::Gravity],
                0,
            ),
            Ok(SceneId::Gravity)
        );
    }

    #[test]
    fn fixed_selection_rejects_an_unavailable_scene() {
        assert_eq!(
            select_scene(LaunchSelection::Fixed(SceneId::Flock), &[SceneId::Prime], 0),
            Err(SceneSelectionError::RequestedSceneUnavailable(
                SceneId::Flock
            ))
        );
    }

    #[test]
    fn random_selection_is_deterministic_and_wraps_over_available_scenes() {
        let available = [SceneId::Prime, SceneId::Flock];
        assert_eq!(
            select_scene(LaunchSelection::Random, &available, 0),
            Ok(SceneId::Prime)
        );
        assert_eq!(
            select_scene(LaunchSelection::Random, &available, 1),
            Ok(SceneId::Flock)
        );
        assert_eq!(
            select_scene(LaunchSelection::Random, &available, 2),
            Ok(SceneId::Prime)
        );
    }

    #[test]
    fn random_selection_rejects_an_empty_scene_set() {
        assert_eq!(
            select_scene(LaunchSelection::Random, &[], 3),
            Err(SceneSelectionError::NoAvailableScenes)
        );
    }

    #[test]
    fn persisted_selection_values_round_trip() {
        for selection in [
            LaunchSelection::Random,
            LaunchSelection::Fixed(SceneId::Prime),
            LaunchSelection::Fixed(SceneId::Gravity),
            LaunchSelection::Fixed(SceneId::Flock),
            LaunchSelection::Fixed(SceneId::Boltzmann),
        ] {
            assert_eq!(
                LaunchSelection::from_storage(selection.storage_value()),
                Some(selection)
            );
        }
    }

    #[test]
    fn unknown_persisted_selection_is_rejected() {
        assert_eq!(LaunchSelection::from_storage(99), None);
    }
}
