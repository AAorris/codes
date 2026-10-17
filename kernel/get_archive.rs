//! `get-archive`: see `archive_cli.rs`.

#[allow(dead_code)]
#[path = "archive.rs"]
mod archive;
#[allow(dead_code)]
#[path = "archive_cli.rs"]
mod archive_cli;

fn main() -> std::process::ExitCode {
    archive_cli::get()
}
