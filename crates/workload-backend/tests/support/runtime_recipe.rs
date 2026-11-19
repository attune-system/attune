//! Same guest recipe and result assertions for both actual provider lanes.
use attune_common::runtime_lifecycle::{RuntimeRecipe, VerifiedTool};
use std::collections::BTreeMap;

pub fn recipe() -> RuntimeRecipe {
    serde_json::from_value(serde_json::json!({
        "tools":[{"name":"python","binaries":["/usr/bin/python3"],
            "args":["-c", "import os,sys; assert os.environ['PATH']=='/usr/bin:/bin'; assert 'SELECTED' not in os.environ; assert 'ATTUNE_API_TOKEN' not in os.environ; assert 'ATTUNE_RUNTIME_RECIPE_DIGESTS' not in os.environ; sys.stdout.write('Python '+sys.version.split()[0])"],
            "version_pattern":"Python (?<version>[0-9]+\\.[0-9]+\\.[0-9]+)","primary":true}],
        "prepare":[{"command":["must-not-run-private-preparation"]}],
        "launch":{"command":["must-not-run-private-launch"]}
    })).unwrap()
}

pub fn valid_result(bytes: &[u8]) -> bool {
    let Ok(tools) = serde_json::from_slice::<BTreeMap<String, VerifiedTool>>(bytes) else {
        return false;
    };
    tools.len() == 1
        && tools.get("python").is_some_and(|tool| {
            tool.executable == std::path::Path::new("/usr/bin/python3")
                && tool.identity.is_absolute()
                && tool.identity.starts_with("/usr")
                && tool.version.starts_with("3.")
        })
}
