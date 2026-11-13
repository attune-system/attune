//! Core utilities run without an interpreter, subprocesses or API authority.
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::time::Duration;

fn integer(
    params: &Map<String, Value>,
    key: &str,
    default: u64,
    maximum: u64,
) -> Result<u64, &'static str> {
    let value = match params.get(key) {
        None => default,
        Some(value) => value.as_u64().ok_or("expected a nonnegative integer")?,
    };
    if value > maximum {
        return Err("integer outside allowed range");
    }
    Ok(value)
}

fn execute(action: &str, params: Value, output: &mut impl Write) -> Result<i32, &'static str> {
    let params = params
        .as_object()
        .ok_or("parameters must be a JSON object")?;
    let message = match params.get("message") {
        None => "",
        Some(value) => value.as_str().ok_or("message must be a string")?,
    };
    match action {
        "echo" => output
            .write_all(message.as_bytes())
            .map_err(|_| "stdout write failed")?,
        "noop" => {
            let code = integer(params, "exit_code", 0, 255)?;
            if !message.is_empty() {
                writeln!(output, "[NOOP] {message}").map_err(|_| "stdout write failed")?;
            }
            writeln!(output, "No operation completed successfully")
                .map_err(|_| "stdout write failed")?;
            return Ok(code as i32);
        }
        "sleep" => {
            let seconds = integer(params, "seconds", 1, 3600)?;
            if !message.is_empty() {
                writeln!(output, "{message}").map_err(|_| "stdout write failed")?;
                output.flush().map_err(|_| "stdout flush failed")?;
            }
            std::thread::sleep(Duration::from_secs(seconds));
            writeln!(output, "Slept for {seconds} seconds").map_err(|_| "stdout write failed")?;
        }
        _ => return Err("unknown core action"),
    }
    Ok(0)
}

pub fn main(action: &str) -> i32 {
    let mut output = io::stdout().lock();
    let result = serde_json::from_reader(io::stdin().lock())
        .map_err(|_| "invalid stdin JSON")
        .and_then(|params| execute(action, params, &mut output))
        .and_then(|code| {
            output.flush().map_err(|_| "stdout flush failed")?;
            Ok(code)
        });
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ERROR: {error}");
            1
        }
    }
}
