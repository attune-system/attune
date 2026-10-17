//! Structural witness only, not a fabricated receipt authority or DB fixture.
//! C must freeze the typed leaf from held authenticated contexts in production.
use attune_common::{
    repositories::runtime_read::{RuntimeReadOrigin, RuntimeReadParent, RuntimeReadTarget},
    secret_provenance::{SecretOrigin, SecretProvenance},
};

#[test]
fn accepted_first_receipt_wrapper_needs_its_authenticated_leaf_in_the_frozen_parent_closure() {
    let typed_receipt = SecretProvenance {
        origins: vec![SecretOrigin::Entity {
            entity_type: "declared_input:execution_config:value".into(),
            entity_id: 41,
            path: "/input".into(),
        }],
        templates: vec![],
    };
    let mut captured = RuntimeReadOrigin {
        target: RuntimeReadTarget::Execution { execution: 7 },
        provenance: vec![SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: "execution_config".into(),
                entity_id: 7,
                path: "/input".into(),
            }],
            templates: vec![],
        }],
        parents: vec![RuntimeReadParent {
            entity_type: "execution_config".into(),
            entity_id: 7,
            path: "/input".into(),
            provenance: typed_receipt,
        }],
    };
    // This is C snapshot + retained_parents' observed structural shape after an
    // accepted first receipt. It is within every size/count/target bound.
    assert_eq!(
        captured.validate().unwrap_err().to_string(),
        "Permission denied: Runtime read evidence is unavailable or exceeds its bound"
    );
    // Demonstrate the exact missing structural edge, NOT how to authorize it.
    // The production leaf must come from A's held context, never this literal.
    captured.parents.push(RuntimeReadParent {
        entity_type: "declared_input:execution_config:value".into(),
        entity_id: 41,
        path: "/input".into(),
        provenance: SecretProvenance {
            origins: vec![SecretOrigin::Local {
                source_kind: "parameter_schema".into(),
                source_ref: Some("/input".into()),
            }],
            templates: vec![],
        },
    });
    assert!(captured.validate().is_ok());
    captured.parents[1].path = "/different".into();
    assert!(
        captured.validate().is_err(),
        "a same-receipt different-site tuple cannot satisfy the original edge"
    );
}
