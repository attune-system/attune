use crate::{Error, Result};

pub const MAX_TRACE_TAG_LENGTH: usize = 255;

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
    use super::{MAX_TRACE_TAG_LENGTH, normalize_trace_tag, validate_trace_tag};

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
}
