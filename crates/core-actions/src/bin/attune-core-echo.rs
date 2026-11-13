use std::io::{self, Write};

fn run() -> Result<(), &'static str> {
    let parameters = core_actions::read_parameters()?;
    let message = core_actions::message(&parameters)?;
    let mut output = io::stdout().lock();
    output
        .write_all(message.as_bytes())
        .map_err(|_| "stdout write failed")?;
    output.flush().map_err(|_| "stdout flush failed")
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ERROR: {error}");
        std::process::exit(1);
    }
}
