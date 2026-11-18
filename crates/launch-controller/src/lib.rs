//! Trusted Docker launch actuator. Engine credentials belong only to this
//! process; requests identify CP-admitted launches, never engine configuration.
pub mod cache;
pub mod engine;
pub mod run;

pub mod actuator;
pub mod capture;
pub mod control_plane;
#[cfg(test)]
mod test_support;
