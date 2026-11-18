//! Container execution effects called directly by trusted worker/sensor loops.
//! Admission, retries, origins, encryption, custody and completion stay there.
//! This library is not a controller service or an HTTP control plane.
mod artifact_file;
pub mod cache;
pub mod configuration;
pub mod engine;
pub mod kubernetes;
mod kubernetes_io;
pub mod planning;
mod probe_capture;
mod process_policy;
pub mod runner_protocol;
#[cfg(test)]
mod test_support;
pub mod transfer_runner;
