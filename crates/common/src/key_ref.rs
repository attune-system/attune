use crate::models::OwnerType;

pub const KEY_LOCAL_REF_MAX_LEN: usize = 63;

pub fn canonical_key_ref(
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    local_ref: &str,
) -> Result<String, String> {
    validate_local_ref(local_ref)?;

    let prefix = match owner_type {
        OwnerType::System => {
            if owner_ref.is_some() {
                return Err("system keys cannot have an owner reference".to_string());
            }
            "system".to_string()
        }
        OwnerType::Identity => format!("identity.{}", required_owner_ref(owner_ref)?),
        OwnerType::Pack => format!("pack.{}", required_component_ref(owner_ref)?),
        OwnerType::Action => format!("action.{}", required_component_ref(owner_ref)?),
        OwnerType::Sensor => format!("sensor.{}", required_component_ref(owner_ref)?),
    };

    Ok(format!("{prefix}.{local_ref}"))
}

fn validate_local_ref(local_ref: &str) -> Result<(), String> {
    let mut chars = local_ref.chars();
    let first = chars
        .next()
        .ok_or_else(|| "local key reference cannot be empty".to_string())?;
    if local_ref.len() > KEY_LOCAL_REF_MAX_LEN {
        return Err(format!(
            "local key reference must be at most {KEY_LOCAL_REF_MAX_LEN} bytes"
        ));
    }
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("local key reference must start with a lowercase letter or number".to_string());
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-')) {
        return Err(
            "local key reference may contain only lowercase letters, numbers, underscores, and hyphens"
                .to_string(),
        );
    }
    Ok(())
}

fn required_owner_ref(owner_ref: Option<&str>) -> Result<&str, String> {
    owner_ref
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "owner reference is required for this key scope".to_string())
}

fn required_component_ref(owner_ref: Option<&str>) -> Result<&str, String> {
    let owner_ref = required_owner_ref(owner_ref)?;
    if owner_ref != owner_ref.to_ascii_lowercase()
        || owner_ref.split('.').any(|segment| {
            segment.is_empty()
                || !segment.chars().all(|ch| {
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-')
                })
        })
    {
        return Err("owner reference is not canonical".to_string());
    }
    Ok(owner_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_disjoint_owner_scoped_refs() {
        assert_eq!(
            canonical_key_ref(OwnerType::System, None, "api_token").unwrap(),
            "system.api_token"
        );
        assert_eq!(
            canonical_key_ref(OwnerType::Identity, Some("alice@example.com"), "api_token").unwrap(),
            "identity.alice@example.com.api_token"
        );
        assert_eq!(
            canonical_key_ref(OwnerType::Pack, Some("core"), "api_token").unwrap(),
            "pack.core.api_token"
        );
        assert_eq!(
            canonical_key_ref(OwnerType::Action, Some("core.echo"), "api_token").unwrap(),
            "action.core.echo.api_token"
        );
        assert_eq!(
            canonical_key_ref(OwnerType::Sensor, Some("core.timer_sensor"), "api_token",).unwrap(),
            "sensor.core.timer_sensor.api_token"
        );
    }

    #[test]
    fn rejects_local_refs_that_could_blur_the_owner_boundary() {
        for invalid in ["", "ApiToken", "api.token", "api token", "-token", "token/"] {
            assert!(
                canonical_key_ref(OwnerType::System, None, invalid).is_err(),
                "accepted invalid local ref {invalid:?}"
            );
        }
    }

    #[test]
    fn requires_exactly_the_owner_ref_expected_by_the_scope() {
        assert!(canonical_key_ref(OwnerType::System, Some("core"), "token").is_err());
        assert!(canonical_key_ref(OwnerType::Pack, None, "token").is_err());
    }

    #[test]
    fn preserves_authoritative_owner_refs_without_an_artificial_total_length_cap() {
        let owner_ref = format!("core.{}", "a".repeat(300));
        assert_eq!(
            canonical_key_ref(OwnerType::Action, Some(&owner_ref), "token").unwrap(),
            format!("action.{owner_ref}.token")
        );
    }
}
