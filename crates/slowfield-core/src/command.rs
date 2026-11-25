use std::fmt;

/// A non-zero raw Windows window handle. The Win32 host converts this to
/// `HWND`; keeping it as an integer here makes parsing portable and testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowHandle(u64);

impl WindowHandle {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// The command modes understood by a Windows `.scr` executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Fullscreen,
    Debug,
    Preview { parent: WindowHandle },
    Configure { owner: Option<WindowHandle> },
    Adjustment,
    Install,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandParseError {
    MissingPreviewParent,
    InvalidWindowHandle,
    UnexpectedArguments,
    UnsupportedMode,
}

impl fmt::Display for CommandParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingPreviewParent => "preview mode requires a parent window handle",
            Self::InvalidWindowHandle => "the supplied window handle is invalid",
            Self::UnexpectedArguments => "this mode does not accept arguments",
            Self::UnsupportedMode => "supported modes: /s, /p <HWND>, /c, /w, /d, /a",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CommandParseError {}

/// Parses the command line provided to a Windows screen saver after its
/// executable name. `/p <HWND>`, `/p:<HWND>`, `/c`, and `/c:<HWND>` follow
/// the conventions used by the Windows screen-saver control panel.
pub fn parse_command(text: &str) -> Result<Command, CommandParseError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Command::Configure { owner: None });
    }

    let (mode, suffix) = split_mode(text);
    if mode.eq_ignore_ascii_case("/s") {
        without_arguments(suffix, Command::Fullscreen)
    } else if mode.eq_ignore_ascii_case("/d") {
        without_arguments(suffix, Command::Debug)
    } else if mode.eq_ignore_ascii_case("/w") {
        without_arguments(suffix, Command::Adjustment)
    } else if mode.eq_ignore_ascii_case("/a") {
        without_arguments(suffix, Command::Install)
    } else if mode.eq_ignore_ascii_case("/p") {
        let parent = required_handle(suffix, CommandParseError::MissingPreviewParent)?;
        Ok(Command::Preview { parent })
    } else if mode.eq_ignore_ascii_case("/c") {
        Ok(Command::Configure {
            owner: optional_handle(suffix)?,
        })
    } else {
        Err(CommandParseError::UnsupportedMode)
    }
}

fn split_mode(text: &str) -> (&str, &str) {
    let separator = text.find(char::is_whitespace).unwrap_or(text.len());
    let (candidate, trailing) = text.split_at(separator);
    if let Some((mode, inline_argument)) = candidate.split_once(':') {
        (mode, inline_argument)
    } else {
        (candidate, trailing)
    }
}

fn without_arguments(suffix: &str, command: Command) -> Result<Command, CommandParseError> {
    if normalize_suffix(suffix).is_empty() {
        Ok(command)
    } else {
        Err(CommandParseError::UnexpectedArguments)
    }
}

fn required_handle(
    suffix: &str,
    missing_error: CommandParseError,
) -> Result<WindowHandle, CommandParseError> {
    let suffix = normalize_suffix(suffix);
    if suffix.is_empty() {
        return Err(missing_error);
    }
    parse_window_handle(suffix).ok_or(CommandParseError::InvalidWindowHandle)
}

fn optional_handle(suffix: &str) -> Result<Option<WindowHandle>, CommandParseError> {
    let suffix = normalize_suffix(suffix);
    if suffix.is_empty() {
        return Ok(None);
    }
    parse_window_handle(suffix)
        .map(Some)
        .ok_or(CommandParseError::InvalidWindowHandle)
}

fn normalize_suffix(suffix: &str) -> &str {
    suffix.trim().trim_start_matches(':').trim()
}

fn parse_window_handle(text: &str) -> Option<WindowHandle> {
    let value =
        if let Some(hexadecimal) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            u64::from_str_radix(hexadecimal, 16).ok()?
        } else {
            text.parse::<u64>().ok()?
        };
    (value != 0).then_some(WindowHandle(value))
}

#[cfg(test)]
mod tests {
    use super::{Command, CommandParseError, WindowHandle, parse_command};

    #[test]
    fn empty_command_opens_configuration_without_an_owner() {
        assert_eq!(parse_command("  "), Ok(Command::Configure { owner: None }));
    }

    #[test]
    fn parses_all_standard_modes() {
        assert_eq!(parse_command("/s"), Ok(Command::Fullscreen));
        assert_eq!(parse_command("/D"), Ok(Command::Debug));
        assert_eq!(parse_command("/w"), Ok(Command::Adjustment));
        assert_eq!(parse_command("/a"), Ok(Command::Install));
        assert_eq!(
            parse_command("/p 42"),
            Ok(Command::Preview {
                parent: WindowHandle(42)
            })
        );
    }

    #[test]
    fn parses_inline_hexadecimal_handles() {
        assert_eq!(
            parse_command("/p:0x2A"),
            Ok(Command::Preview {
                parent: WindowHandle(42)
            })
        );
        assert_eq!(
            parse_command("/c:99"),
            Ok(Command::Configure {
                owner: Some(WindowHandle(99))
            })
        );
    }

    #[test]
    fn rejects_incomplete_and_extra_arguments() {
        assert_eq!(
            parse_command("/p"),
            Err(CommandParseError::MissingPreviewParent)
        );
        assert_eq!(
            parse_command("/p zero"),
            Err(CommandParseError::InvalidWindowHandle)
        );
        assert_eq!(
            parse_command("/s extra"),
            Err(CommandParseError::UnexpectedArguments)
        );
    }

    #[test]
    fn rejects_unknown_or_zero_handles() {
        assert_eq!(parse_command("/x"), Err(CommandParseError::UnsupportedMode));
        assert_eq!(
            parse_command("/c:0"),
            Err(CommandParseError::InvalidWindowHandle)
        );
    }
}
