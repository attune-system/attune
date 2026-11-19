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

pub fn executable_recipe() -> RuntimeRecipe {
    let mut recipe = recipe();
    recipe.workspace = true;
    recipe.prepare = serde_json::from_value(serde_json::json!([{"command":["{tool_python}","-c",
        "import os,sys; assert sys.stdin.buffer.read()==b''; assert 'SELECTED' not in os.environ; assert 'ATTUNE_API_TOKEN' not in os.environ; p=os.getppid(); paths=['/proc/'+str(p)+'/environ','/proc/'+str(p)+'/fd/0']; [(exec(\"try:\\n open(path,'rb')\\nexcept PermissionError:\\n pass\\nelse:\\n raise AssertionError('parent context readable')\",{'path':path})) for path in paths]"
    ]}])).unwrap();
    recipe.launch = serde_json::from_value(
        serde_json::json!({"command":["{tool_python}","{source_file}"],"timeout_seconds":10}),
    )
    .unwrap();
    recipe
}
pub const SOURCE: &[u8] = b"import os,sys\nassert os.environ['ATTUNE_API_TOKEN']=='owned-invocation-only'\nassert os.environ['SELECTED']=='owned-caller-only'\nsys.stdout.buffer.write(b'recipe:'+sys.stdin.buffer.read())\n";
pub const STDIN: &[u8] = b"{\"exact\": [1, true]} \n";

pub fn legacy_definition(
    matches: bool,
) -> attune_common::runtime_detection::NativeRuntimeVerification {
    attune_common::runtime_detection::NativeRuntimeVerification {
        registered_version: false,
        execution_config: serde_json::json!({"interpreter":{"binary":"/bin/sh"}}),
        distributions: serde_json::json!({"verification":{"commands":[
            {"binary":"/bin/sh","args":["-c",r#"test -z "${ATTUNE_API_TOKEN+x}" && test -z "${SELECTED+x}" && printf legacy; exit 7"#],"exit_code":7,"pattern":if matches {"^legacy$"}else{"must-not-match"},"priority":1}
        ]}}),
    }
}
