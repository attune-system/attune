use std::io::{self, Write};
use std::time::Duration;

fn run() -> Result<(), &'static str> {
    let parameters = core_actions::read_parameters()?;
    let message = core_actions::message(&parameters)?;
    let seconds = core_actions::integer(&parameters, "seconds", 1, 3600)?;
    let mut output = io::stdout().lock();
    if !message.is_empty() {
        writeln!(output, "{message}").map_err(|_| "stdout write failed")?;
        output.flush().map_err(|_| "stdout flush failed")?;
    }
    std::thread::sleep(Duration::from_secs(seconds));
    writeln!(output, "Slept for {seconds} seconds").map_err(|_| "stdout write failed")?;
    output.flush().map_err(|_| "stdout flush failed")
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ERROR: {error}");
        std::process::exit(1);
    }
}
