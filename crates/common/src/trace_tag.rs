use crate::{Error, Result};

pub const MAX_TRACE_TAG_LENGTH: usize = 255;

fn normalize_identity_component(identity_ref: &str) -> String {
    let mut out = String::with_capacity(identity_ref.len());
    for ch in identity_ref.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn manual_trace_tag(identity_ref: &str, timestamp_millis: i64) -> Result<String> {
    let mut identity_component = normalize_identity_component(identity_ref);
    if identity_component.is_empty() {
        identity_component = "unknown".to_string();
    }
    let max_identity_len = 200;
    if identity_component.len() > max_identity_len {
        identity_component.truncate(max_identity_len);
    }
    normalize_trace_tag(&format!("manual-{identity_component}-{timestamp_millis}"))
}

pub fn normalize_trace_tag(value: &str) -> Result<String> {
    let normalized = value.trim();
    validate_trace_tag(normalized)?;
    Ok(normalized.to_string())
}

pub fn validate_trace_tag(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::validation("trace tag cannot be empty"));
    }
    if value.len() > MAX_TRACE_TAG_LENGTH {
        return Err(Error::validation(format!(
            "trace tag must be at most {MAX_TRACE_TAG_LENGTH} characters"
        )));
    }
    if value.chars().any(|ch| ch.is_control()) {
        return Err(Error::validation(
            "trace tag cannot contain control characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{manual_trace_tag, normalize_trace_tag, validate_trace_tag, MAX_TRACE_TAG_LENGTH};

    #[test]
    fn normalize_trace_tag_trims_and_returns_value() {
        let normalized = normalize_trace_tag("  core.timer.42  ").expect("normalize succeeds");
        assert_eq!(normalized, "core.timer.42");
    }

    #[test]
    fn validate_trace_tag_rejects_empty() {
        assert!(validate_trace_tag("").is_err());
    }

    #[test]
    fn validate_trace_tag_rejects_control_characters() {
        assert!(validate_trace_tag("core.timer.\n42").is_err());
    }

    #[test]
    fn validate_trace_tag_rejects_over_max_length() {
        let too_long = "a".repeat(MAX_TRACE_TAG_LENGTH + 1);
        assert!(validate_trace_tag(&too_long).is_err());
    }

    #[test]
    fn manual_trace_tag_normalizes_identity_component() {
        let value = manual_trace_tag("Test@Attune.Local", 12345).expect("manual trace tag");
        assert_eq!(value, "manual-test-attune.local-12345");
    }

    #[test]
    fn manual_trace_tag_uses_unknown_for_empty_component() {
        let value = manual_trace_tag("$$$", 12345).expect("manual trace tag");
        assert_eq!(value, "manual-unknown-12345");
    }
}
