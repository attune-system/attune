//! Private one-shot start framing. Never an object body, public digest or API.
use attune_common::workload_launch::LaunchInvocation;
use std::io::{self, Read};

pub const START_MAGIC: &[u8] = b"attune-process-v1\0";
pub const MAX_START_CONTEXT: usize = 4 * 1024 * 1024;

pub fn invalid() -> io::Error {
    io::Error::other("invalid private process start context")
}
pub fn encode_start(invocation: &LaunchInvocation) -> io::Result<Vec<u8>> {
    validate(invocation)?;
    let context = serde_json::to_vec(invocation).map_err(|_| invalid())?;
    if context.is_empty() || context.len() > MAX_START_CONTEXT {
        return Err(invalid());
    }
    let mut header = Vec::with_capacity(START_MAGIC.len() + 4 + context.len());
    header.extend_from_slice(START_MAGIC);
    header.extend_from_slice(&(context.len() as u32).to_be_bytes());
    header.extend_from_slice(&context);
    Ok(header)
}
pub fn decode_context(context: &[u8]) -> io::Result<LaunchInvocation> {
    if context.is_empty() || context.len() > MAX_START_CONTEXT {
        return Err(invalid());
    }
    let invocation = serde_json::from_slice(context).map_err(|_| invalid())?;
    validate(&invocation)?;
    Ok(invocation)
}
fn validate(invocation: &LaunchInvocation) -> io::Result<()> {
    if invocation.argv.is_empty()
        || invocation.argv[0].is_empty()
        || invocation.argv.len() > 4096
        || invocation.argv.iter().any(|value| value.contains('\0'))
    {
        return Err(invalid());
    }
    let mut bytes = 0usize;
    for value in &invocation.argv {
        bytes = bytes.checked_add(value.len()).ok_or_else(invalid)?;
    }
    for (name, value) in &invocation.env {
        let upper = name.to_ascii_uppercase();
        if name.is_empty()
            || name.contains(['=', '\0'])
            || value.contains('\0')
            || upper.starts_with("ATTUNE__")
            || (upper.starts_with("ATTUNE_LAUNCH_")
                && !private_helper_challenge(invocation, name, value))
            || upper.starts_with("DOCKER_")
            || upper.starts_with("KUBERNETES_")
            || matches!(
                upper.as_str(),
                "ATTUNE_RUNTIME_LOG_CUSTODY_KEY"
                    | "ATTUNE_WORKLOAD_ENROLLMENT_TOKEN"
                    | "ATTUNE_AGENT_BOOTSTRAP_TOKEN"
                    | "ATTUNE_CONFIG"
                    | "DATABASE_URL"
                    | "MQ_URL"
                    | "JWT_SECRET"
                    | "ENCRYPTION_KEY"
                    | "KUBECONFIG"
            )
        {
            return Err(invalid());
        }
        bytes = bytes
            .checked_add(name.len())
            .and_then(|sum| sum.checked_add(value.len()))
            .ok_or_else(invalid)?;
    }
    if bytes > MAX_START_CONTEXT {
        return Err(invalid());
    }
    Ok(())
}
fn private_helper_challenge(invocation: &LaunchInvocation, name: &str, value: &str) -> bool {
    use crate::planning::{
        CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES, CANDIDATE_PYTHON_PREP_CHALLENGE_ENV,
        RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES, RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV,
    };
    use attune_common::workload_launch::{LaunchRoot, RUNNER_ROOT};
    let mode = invocation.argv.get(1).map(String::as_str);
    let expected_bytes = match name {
        RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV
            if matches!(
                mode,
                Some("run-runtime-recipe" | "run-candidate-runtime-recipe")
            ) =>
        {
            RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES
        }
        CANDIDATE_PYTHON_PREP_CHALLENGE_ENV if mode == Some("run-candidate-python") => {
            CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES
        }
        _ => return false,
    };
    value.len() == expected_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && ["linux-amd64", "linux-arm64"]
            .iter()
            .any(|file| invocation.argv.first() == Some(&format!("{RUNNER_ROOT}/{file}")))
        && invocation.working_root == LaunchRoot::Pack
}

/// Exact reads avoid consuming any of the application's original stdin.
/// Used by deterministic codec tests; the executable uses cancellable FD reads.
pub fn read_start(mut source: impl Read) -> io::Result<LaunchInvocation> {
    let mut magic = vec![0; START_MAGIC.len()];
    source.read_exact(&mut magic).map_err(|_| invalid())?;
    if magic != START_MAGIC {
        return Err(invalid());
    }
    let mut length = [0; 4];
    source.read_exact(&mut length).map_err(|_| invalid())?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_START_CONTEXT {
        return Err(invalid());
    }
    let mut context = vec![0; length];
    source.read_exact(&mut context).map_err(|_| invalid())?;
    decode_context(&context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::LaunchRoot;
    use std::{collections::BTreeMap, io::Cursor};
    fn invocation() -> LaunchInvocation {
        LaunchInvocation {
            argv: vec!["/bin/echo".into(), "owned-private-argument".into()],
            env: BTreeMap::from([("SELECTED".into(), "owned-private-value".into())]),
            working_root: LaunchRoot::Scratch,
        }
    }
    #[test]
    fn exact_frame_leaves_literal_and_empty_application_stdin_unchanged() {
        for input in [b"".as_slice(), b"{\"selected\":\"value\"}\n".as_slice()] {
            let mut body = encode_start(&invocation()).unwrap();
            body.extend_from_slice(input);
            let mut cursor = Cursor::new(body);
            let decoded = read_start(&mut cursor).unwrap();
            assert_eq!(decoded, invocation());
            let mut remaining = Vec::new();
            cursor.read_to_end(&mut remaining).unwrap();
            assert_eq!(remaining, input);
        }
    }
    #[test]
    fn every_truncation_and_malformed_or_oversized_header_rejects_without_private_errors() {
        let frame = encode_start(&invocation()).unwrap();
        for length in 0..frame.len() {
            let error = read_start(Cursor::new(&frame[..length])).unwrap_err();
            assert!(!error.to_string().contains("owned-private"));
        }
        for context in [b"{owned-private-malformed}".as_slice(), b"{}".as_slice()] {
            let error = decode_context(context).unwrap_err();
            assert!(!error.to_string().contains("owned-private"));
        }
        let mut frame = START_MAGIC.to_vec();
        frame.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(read_start(Cursor::new(frame)).is_err());
        let mut denied = invocation();
        denied
            .env
            .insert("ENCRYPTION_KEY".into(), "owned-private-manager-key".into());
        assert!(encode_start(&denied).is_err());
    }
}
