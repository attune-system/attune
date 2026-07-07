use std::{env, fs, path::Path};

use attune_api::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/attune-openapi.json".to_string());

    let openapi = ApiDoc::openapi();
    let payload = serde_json::to_vec_pretty(&openapi)?;

    if let Some(parent) = Path::new(&output_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(&output_path, payload)?;
    println!("Wrote OpenAPI spec to {}", output_path);

    Ok(())
}
