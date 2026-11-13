use std::io::{self, Write};

fn run() -> Result<i32, &'static str> {
    let parameters = core_actions::read_parameters()?;
    let message = core_actions::message(&parameters)?;
    let exit_code = core_actions::integer(&parameters, "exit_code", 0, 255)?;
    let mut output = io::stdout().lock();
    if !message.is_empty() {
        writeln!(output, "[NOOP] {message}").map_err(|_| "stdout write failed")?;
    }
    writeln!(output, "No operation completed successfully").map_err(|_| "stdout write failed")?;
    output.flush().map_err(|_| "stdout flush failed")?;
    Ok(exit_code as i32)
}

fn main() {
    let exit_code = match run() {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("ERROR: {error}");
            1
        }
    };
    std::process::exit(exit_code);
}
