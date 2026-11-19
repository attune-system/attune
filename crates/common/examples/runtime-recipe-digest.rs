//! Print the exact canonical recipe digest for an operator admission allowlist.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::env::args_os()
        .nth(1)
        .ok_or("Usage: runtime-recipe-digest <runtime.yaml>")?;
    let value: serde_json::Value = serde_yaml_ng::from_slice(&std::fs::read(file)?)?;
    let recipe = value
        .pointer("/execution_config/lifecycle")
        .or_else(|| value.get("lifecycle"))
        .unwrap_or(&value)
        .clone();
    let recipe: attune_common::runtime_lifecycle::RuntimeRecipe = serde_json::from_value(recipe)?;
    recipe.validate()?;
    println!("{}", recipe.digest()?);
    Ok(())
}
