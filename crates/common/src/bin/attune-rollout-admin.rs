//! Separate one-shot executable; never links or starts the API service.
fn main() -> std::process::ExitCode {
    attune_common::rollout_offline::main_entry()
}
