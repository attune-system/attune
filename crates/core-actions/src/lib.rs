//! Shared stdin JSON parsing and parameter validation for native core actions.
use serde_json::{Map, Value};
use std::io;

pub fn read_parameters() -> Result<Map<String, Value>, &'static str> {
    match serde_json::from_reader(io::stdin().lock()).map_err(|_| "invalid stdin JSON")? {
        Value::Object(parameters) => Ok(parameters),
        _ => Err("parameters must be a JSON object"),
    }
}

pub fn message(parameters: &Map<String, Value>) -> Result<&str, &'static str> {
    match parameters.get("message") {
        None => Ok(""),
        Some(value) => value.as_str().ok_or("message must be a string"),
    }
}

pub fn integer(
    parameters: &Map<String, Value>,
    key: &str,
    default: u64,
    maximum: u64,
) -> Result<u64, &'static str> {
    let value = match parameters.get(key) {
        None => default,
        Some(value) => value.as_u64().ok_or("expected a nonnegative integer")?,
    };
    if value > maximum {
        return Err("integer outside allowed range");
    }
    Ok(value)
}
