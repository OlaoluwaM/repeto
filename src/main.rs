mod cli;
mod clock;
mod commands;

fn main() -> std::process::ExitCode {
    cli::run()
}
