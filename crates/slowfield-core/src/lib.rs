//! Platform-independent rules shared by the Slowfield Windows host and fields.
//!
//! This crate intentionally has no Win32 or OpenGL dependency. It owns the
//! input contracts that can be verified without a display: standard screen
//! saver invocation parsing and scene-selection policy.

mod command;
mod selection;

pub use command::{Command, CommandParseError, WindowHandle, parse_command};
pub use selection::{LaunchSelection, SceneId, SceneSelectionError, select_scene};
