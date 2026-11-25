#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod host;
mod scene;

use slowfield_core::{Command, parse_command};

fn main() {
    let command_line = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let exit_code = match parse_command(&command_line) {
        Ok(Command::Configure { owner }) => host::show_configuration(owner),
        Ok(Command::Install) => 0,
        Ok(command) => unsafe { host::run(command) },
        Err(error) => {
            host::show_error(&error.to_string());
            1
        }
    };
    std::process::exit(exit_code);
}
