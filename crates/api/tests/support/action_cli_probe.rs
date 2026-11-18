//! Owned standalone executable for argv/stdin process-boundary tests.
use std::io::Read;
fn main() {
    for argument in std::env::args().skip(1) {
        println!("arg:{}:{argument}", argument.len());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    println!("stdin:{}:{input}", input.len());
    println!("cwd:{}", std::env::current_dir().unwrap().display());
    println!("token:{}", std::env::var_os("ATTUNE_API_TOKEN").is_some());
    println!(
        "parent:{}",
        std::env::var_os("SENSITIVE_PARENT_VAR").is_some()
    );
}
