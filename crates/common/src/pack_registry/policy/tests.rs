use super::*;
use serde_json::{json, Value};

fn input(value: Value) -> PolicyInput {
    PolicyInput::from_json(&value.to_string()).unwrap()
}

fn create(value: Value) -> IndexPolicy {
    input(value).create().unwrap()
}

fn assert_error(error: PolicyError, field: PolicyField, kind: PolicyErrorKind) {
    assert_eq!(error, PolicyError::new(field, kind));
}

#[test]
fn create_defaults_and_serialized_policy_fields() {
    let policy = create(json!({}));
    assert_eq!(policy, IndexPolicy::default());
    assert_eq!(policy.update_mode(), UpdateMode::Manual);
    assert_eq!(policy.allowed_git_ref_pattern(), DEFAULT_GIT_REF_PATTERN);
    assert!(!policy.allow_unindexed_git_refs());
    assert_eq!(
        serde_json::to_value(&policy).unwrap(),
        json!({
            "update_mode": "manual",
            "refresh_interval_seconds": null,
            "lazy_refresh_min_interval_seconds": null,
            "allowed_git_ref_pattern": DEFAULT_GIT_REF_PATTERN,
            "allow_unindexed_git_refs": false
        })
    );
    for candidate in ["1.0.2", "12.10.3", "000.00.0"] {
        assert!(policy.matches_git_ref(candidate));
    }
    for candidate in [
        "v1.2.3", "1.2", "1.2.3.4", "main", " 1.2.3", "1.2.3\n", "١.٢.٣",
    ] {
        assert!(!policy.matches_git_ref(candidate));
    }
    assert_eq!(
        create(json!({"update_mode":"automatic"})).refresh_interval_seconds(),
        Some(3600)
    );
    assert_eq!(
        create(json!({"update_mode":"lazy"})).lazy_refresh_min_interval_seconds(),
        Some(30)
    );
}

#[test]
fn deserialization_preserves_omission_and_null() {
    assert_eq!(
        PolicyInput::from_json("{}").unwrap(),
        PolicyInput::default()
    );
    let nulls = PolicyInput::from_json(
        r#"{
        "update_mode":null,"refresh_interval_seconds":null,
        "lazy_refresh_min_interval_seconds":null,"allowed_git_ref_pattern":null,
        "allow_unindexed_git_refs":null
    }"#,
    )
    .unwrap();
    assert_eq!(nulls.update_mode, PolicyValue::Null);
    assert_eq!(nulls.refresh_interval_seconds, PolicyValue::Null);
    assert_eq!(nulls.lazy_refresh_min_interval_seconds, PolicyValue::Null);
    assert_eq!(nulls.allowed_git_ref_pattern, PolicyValue::Null);
    assert_eq!(nulls.allow_unindexed_git_refs, PolicyValue::Null);
    let serde: PolicyInput = serde_json::from_str(r#"{"refresh_interval_seconds":null}"#).unwrap();
    assert_eq!(serde.refresh_interval_seconds, PolicyValue::Null);
    assert_eq!(serde.update_mode, PolicyValue::Omitted);
}

#[test]
fn all_mode_transitions_preserve_active_values_or_default_and_clear() {
    for from in ["manual", "automatic", "lazy"] {
        let mut seed = json!({"update_mode":from, "allowed_git_ref_pattern":"main"});
        if from == "automatic" {
            seed["refresh_interval_seconds"] = json!(123);
        }
        if from == "lazy" {
            seed["lazy_refresh_min_interval_seconds"] = json!(456);
        }
        let stored = create(seed);
        assert_eq!(stored.patched(&input(json!({}))).unwrap(), stored);
        for to in ["manual", "automatic", "lazy"] {
            let patched = stored.patched(&input(json!({"update_mode":to}))).unwrap();
            let automatic = match (from, to) {
                ("automatic", "automatic") => Some(123),
                (_, "automatic") => Some(3600),
                _ => None,
            };
            let lazy = match (from, to) {
                ("lazy", "lazy") => Some(456),
                (_, "lazy") => Some(30),
                _ => None,
            };
            assert_eq!(
                patched.refresh_interval_seconds(),
                automatic,
                "{from} -> {to}"
            );
            assert_eq!(
                patched.lazy_refresh_min_interval_seconds(),
                lazy,
                "{from} -> {to}"
            );
            assert_eq!(patched.allowed_git_ref_pattern(), "main");
            assert_eq!(
                patched.update_mode(),
                match to {
                    "automatic" => UpdateMode::Automatic,
                    "lazy" => UpdateMode::Lazy,
                    _ => UpdateMode::Manual,
                }
            );
        }
    }
}

#[test]
fn all_mode_transitions_handle_explicit_intervals_and_nulls_atomically() {
    for from in ["manual", "automatic", "lazy"] {
        let stored = create(json!({"update_mode":from}));
        let before = stored.clone();
        for to in ["manual", "automatic", "lazy"] {
            for (field, active_mode) in [
                (PolicyField::RefreshIntervalSeconds, "automatic"),
                (PolicyField::LazyRefreshMinIntervalSeconds, "lazy"),
            ] {
                let mut patch = json!({"update_mode":to});
                patch[field.as_str()] = Value::Null;
                let result = stored.patched(&input(patch.clone()));
                if to == active_mode {
                    assert_error(result.unwrap_err(), field, PolicyErrorKind::NullNotAllowed);
                } else {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap()[field.as_str()],
                        Value::Null
                    );
                }
                patch[field.as_str()] = json!(42);
                let result = stored.patched(&input(patch));
                if to == active_mode {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap()[field.as_str()],
                        json!(42)
                    );
                } else {
                    assert_error(
                        result.unwrap_err(),
                        field,
                        PolicyErrorKind::IntervalNotApplicable,
                    );
                }
                assert_eq!(stored, before);
            }
        }
    }
}

#[test]
fn create_and_patch_interval_bounds_and_null_applicability() {
    for mode in ["manual", "automatic", "lazy"] {
        let stored = create(json!({"update_mode":mode}));
        for (field, active_mode) in [
            (PolicyField::RefreshIntervalSeconds, "automatic"),
            (PolicyField::LazyRefreshMinIntervalSeconds, "lazy"),
        ] {
            for value in [
                Value::Null,
                json!(0),
                json!(1),
                json!(604800),
                json!(604801),
                json!(u64::MAX),
            ] {
                let mut doc = json!({"update_mode":mode});
                doc[field.as_str()] = value.clone();
                let input = input(doc);
                let result = input.create();
                assert_eq!(stored.patched(&input), result);
                if value.is_null() {
                    if mode == active_mode {
                        assert_error(result.unwrap_err(), field, PolicyErrorKind::NullNotAllowed);
                    } else {
                        assert!(result.is_ok());
                    }
                } else if mode != active_mode {
                    assert_error(
                        result.unwrap_err(),
                        field,
                        PolicyErrorKind::IntervalNotApplicable,
                    );
                } else if [json!(1), json!(604800)].contains(&value) {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap()[field.as_str()],
                        value
                    );
                } else {
                    assert_error(
                        result.unwrap_err(),
                        field,
                        PolicyErrorKind::IntervalOutOfRange,
                    );
                }
            }
        }
    }
}

#[test]
fn non_nullable_fields_and_stage_a_unindexed_rejection() {
    for mode in ["manual", "automatic", "lazy"] {
        let stored = create(json!({"update_mode":mode}));
        for field in [
            PolicyField::UpdateMode,
            PolicyField::AllowedGitRefPattern,
            PolicyField::AllowUnindexedGitRefs,
        ] {
            let patch = input(json!({field.as_str():null}));
            assert_error(
                patch.create().unwrap_err(),
                field,
                PolicyErrorKind::NullNotAllowed,
            );
            assert_error(
                stored.patched(&patch).unwrap_err(),
                field,
                PolicyErrorKind::NullNotAllowed,
            );
        }
        let enabled = input(json!({"update_mode":mode,"allow_unindexed_git_refs":true}));
        for error in [
            enabled.create().unwrap_err(),
            stored.patched(&enabled).unwrap_err(),
        ] {
            assert_error(
                error,
                PolicyField::AllowUnindexedGitRefs,
                PolicyErrorKind::FeatureNotAvailable,
            );
            assert_eq!(error.to_string(), "feature_not_available");
        }
        assert_eq!(
            stored
                .patched(&input(json!({"allow_unindexed_git_refs":false})))
                .unwrap(),
            stored
        );
    }
}

#[test]
fn malformed_primitive_types_are_rejected_with_known_field() {
    for (field, values) in [
        (
            PolicyField::UpdateMode,
            vec![json!(true), json!(1), json!([]), json!({})],
        ),
        (
            PolicyField::RefreshIntervalSeconds,
            vec![json!(true), json!(1.0), json!("1"), json!([]), json!({})],
        ),
        (
            PolicyField::LazyRefreshMinIntervalSeconds,
            vec![json!(false), json!(1.5), json!("30"), json!([]), json!({})],
        ),
        (
            PolicyField::AllowedGitRefPattern,
            vec![json!(true), json!(1), json!([]), json!({})],
        ),
        (
            PolicyField::AllowUnindexedGitRefs,
            vec![json!("false"), json!(0), json!([]), json!({})],
        ),
    ] {
        for value in values {
            let doc = json!({field.as_str():value}).to_string();
            assert_error(
                PolicyInput::from_json(&doc).unwrap_err(),
                field,
                PolicyErrorKind::WrongType,
            );
            assert!(serde_json::from_str::<PolicyInput>(&doc).is_err());
        }
    }
    for mode in ["Automatic", "MANUAL", " lazy", "scheduled", ""] {
        assert_error(
            PolicyInput::from_json(&json!({"update_mode":mode}).to_string()).unwrap_err(),
            PolicyField::UpdateMode,
            PolicyErrorKind::InvalidMode,
        );
    }
    for field in [
        PolicyField::RefreshIntervalSeconds,
        PolicyField::LazyRefreshMinIntervalSeconds,
    ] {
        assert_error(
            PolicyInput::from_json(&json!({field.as_str():-1}).to_string()).unwrap_err(),
            field,
            PolicyErrorKind::IntervalOutOfRange,
        );
        let exponent = format!(r#"{{"{}":1e0}}"#, field.as_str());
        assert_error(
            PolicyInput::from_json(&exponent).unwrap_err(),
            field,
            PolicyErrorKind::WrongType,
        );
    }
}

#[test]
fn direct_json_rejects_unknown_duplicate_and_malformed_documents_without_leaks() {
    for field in [
        PolicyField::UpdateMode,
        PolicyField::RefreshIntervalSeconds,
        PolicyField::LazyRefreshMinIntervalSeconds,
        PolicyField::AllowedGitRefPattern,
        PolicyField::AllowUnindexedGitRefs,
    ] {
        let key = field.as_str();
        let doc = format!(r#"{{"{key}":null,"{key}":null}}"#);
        assert_error(
            PolicyInput::from_json(&doc).unwrap_err(),
            field,
            PolicyErrorKind::DuplicateField,
        );
        assert!(serde_json::from_str::<PolicyInput>(&doc).is_err());
    }
    let escaped_duplicate = r#"{"update_mode":"manual","update_\u006dode":"lazy"}"#;
    assert_error(
        PolicyInput::from_json(escaped_duplicate).unwrap_err(),
        PolicyField::UpdateMode,
        PolicyErrorKind::DuplicateField,
    );
    for doc in [
        r#"{"SECRET_UNKNOWN":"SECRET_VALUE"}"#,
        r#"{"update_mode":"SECRET_VALUE"}"#,
        r#"{"allowed_git_ref_pattern":{"SECRET_VALUE":1}}"#,
        r#"{"update_mode":SECRET_VALUE}"#,
    ] {
        let error = PolicyInput::from_json(doc).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SECRET"));
        let serde_error = serde_json::from_str::<PolicyInput>(doc).unwrap_err();
        assert!(!serde_error.to_string().contains("SECRET"));
    }
    assert_eq!(
        PolicyInput::from_json(r#"{"unknown":null}"#).unwrap_err(),
        PolicyError::document(PolicyErrorKind::UnknownField)
    );
    for doc in [
        "",
        "null",
        "[]",
        "true",
        "1",
        r#""SECRET""#,
        "{",
        "{} {}",
        "{} SECRET",
        "{1:2}",
    ] {
        assert_eq!(
            PolicyInput::from_json(doc).unwrap_err().kind,
            PolicyErrorKind::InvalidDocument
        );
    }
}

#[test]
fn patches_do_not_mutate_on_late_failure_and_effective_noops_compare_equal() {
    let stored = create(json!({"update_mode":"automatic", "refresh_interval_seconds":123}));
    let before = stored.clone();
    for patch in [
        json!({"update_mode":"lazy", "lazy_refresh_min_interval_seconds":42, "allowed_git_ref_pattern":"[SECRET"}),
        json!({"update_mode":"lazy", "allow_unindexed_git_refs":true}),
        json!({"update_mode":"manual", "refresh_interval_seconds":42}),
        json!({"refresh_interval_seconds":0}),
    ] {
        assert!(stored.patched(&input(patch)).is_err());
        assert_eq!(stored, before);
    }
    for patch in [json!({}), serde_json::to_value(&stored).unwrap()] {
        assert_eq!(stored.patched(&input(patch)).unwrap(), stored);
    }
    let changed = stored
        .patched(&input(json!({"allowed_git_ref_pattern":"main"})))
        .unwrap();
    assert_eq!(stored, before);
    assert_ne!(changed, stored);
    assert!(changed.matches_git_ref("main"));
}

#[test]
fn regex_absolute_matching_survives_options_alternation_and_newlines() {
    for (pattern, accepted, rejected) in [
        (
            "main|release",
            vec!["main", "release"],
            vec!["xmain", "releasex", "main\n"],
        ),
        ("a|ab", vec!["a", "ab"], vec!["abc"]),
        (
            "(?m)^main$",
            vec!["main"],
            vec!["main\n", "x\nmain", "main\nx"],
        ),
        ("(?i)main", vec!["main", "MAIN"], vec![" MAIN", "MAIN\n"]),
        ("(?s).*", vec!["main\nother", ""], vec![]),
        (".*", vec!["main", ""], vec!["main\nother", "main\n"]),
        (
            "(?x) main # trailing comment",
            vec!["main"],
            vec!["main\n", "xmain"],
        ),
        (
            "(?mx)^main$ # comment",
            vec!["main"],
            vec!["main\nx", "x\nmain"],
        ),
        (" main ", vec![" main "], vec!["main"]),
        ("café", vec!["café"], vec!["cafe", "CAFÉ", "xcafé"]),
    ] {
        let policy = create(json!({"allowed_git_ref_pattern":pattern}));
        assert_eq!(policy.allowed_git_ref_pattern(), pattern);
        for candidate in accepted {
            assert!(
                policy.matches_git_ref(candidate),
                "{pattern:?} / {candidate:?}"
            );
        }
        for candidate in rejected {
            assert!(
                !policy.matches_git_ref(candidate),
                "{pattern:?} / {candidate:?}"
            );
        }
    }
}

#[test]
fn regex_byte_bounds_syntax_and_compiled_size_are_enforced_and_sanitized() {
    for source in [
        "a".to_owned(),
        "a".repeat(4096),
        "é".repeat(2048),
        "💡".repeat(1024),
    ] {
        let policy = create(json!({"allowed_git_ref_pattern":source}));
        assert!(policy.matches_git_ref(&source));
    }
    for source in [
        String::new(),
        "a".repeat(4097),
        "é".repeat(2049),
        "💡".repeat(1025),
    ] {
        assert_error(
            input(json!({"allowed_git_ref_pattern":source}))
                .create()
                .unwrap_err(),
            PolicyField::AllowedGitRefPattern,
            PolicyErrorKind::PatternLength,
        );
    }
    for source in [
        "[SECRET",
        r"(SECRET)\1",
        "(?=SECRET)",
        "SECRET)|bypass(?:",
        "(",
    ] {
        let error = input(json!({"allowed_git_ref_pattern":source}))
            .create()
            .unwrap_err();
        assert_error(
            error,
            PolicyField::AllowedGitRefPattern,
            PolicyErrorKind::InvalidPattern,
        );
        assert!(!format!("{error:?} {error}").contains("SECRET"));
    }
    assert_error(
        input(json!({"allowed_git_ref_pattern":"a{1000000}"}))
            .create()
            .unwrap_err(),
        PolicyField::AllowedGitRefPattern,
        PolicyErrorKind::PatternTooComplex,
    );
}

#[test]
fn reusable_serde_policy_object_works_in_yaml_without_claiming_import_validation() {
    let input: PolicyInput =
        serde_yaml_ng::from_str("update_mode: lazy\nrefresh_interval_seconds: null\n").unwrap();
    assert_eq!(input.refresh_interval_seconds, PolicyValue::Null);
    assert_eq!(
        input.create().unwrap().lazy_refresh_min_interval_seconds(),
        Some(30)
    );
    assert!(
        serde_yaml_ng::from_str::<PolicyInput>("update_mode: manual\nupdate_mode: lazy\n").is_err()
    );
    assert!(serde_yaml_ng::from_str::<PolicyInput>("unknown: secret\n").is_err());
}
