use attune_common::{
    config::Config,
    models::{ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType},
    repositories::{
        artifact::{
            classify_artifact, ArtifactRepository, CreateArtifactInput, UpdateArtifactInput,
        },
        identity::{CreateIdentityInput, IdentityRepository},
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        Create, Delete, Patch, Update,
    },
    test_database::TestDatabase,
};

#[tokio::test]
async fn ownership_migration_does_not_trust_historical_refs_or_rebind_label_edits() {
    let config = Config::load().unwrap();
    let db = TestDatabase::create_before(
        &config.database,
        "20261015003001_artifact_runtime_owner_bindings.sql",
    )
    .await
    .unwrap();
    let input = |reference: &str| CreateArtifactInput {
        r#ref: reference.into(),
        scope: OwnerType::System,
        owner: String::new(),
        r#type: ArtifactType::Progress,
        visibility: ArtifactVisibility::Private,
        classification: classify_artifact(reference, ArtifactType::Progress),
        retention_policy: RetentionPolicyType::Versions,
        retention_limit: 5,
        name: None,
        description: None,
        content_type: None,
        data: None,
    };
    let historical = ArtifactRepository::create(db.pool(), input("migration.historical"))
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261015003001_artifact_runtime_owner_bindings.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    ArtifactRepository::update(
        db.pool(),
        historical.id,
        UpdateArtifactInput {
            name: Some(Patch::Set("Label only".into())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::Artifact(historical.id), 1)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    let admitted = ArtifactRepository::create(db.pool(), input("migration.new"))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let target = RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::Artifact(admitted.id), 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target.pack_id(), None);
    assert_eq!(target.owner_id(), None);
    tx.commit().await.unwrap();
    assert!(sqlx::query(
        "UPDATE artifact_runtime_owner_binding SET owner_ref = 'forged' WHERE artifact = $1"
    )
    .bind(admitted.id)
    .execute(db.pool())
    .await
    .is_err());
    let human = IdentityRepository::create(
        db.pool(),
        CreateIdentityInput {
            login: "artifact-owner-human".into(),
            display_name: None,
            password_hash: None,
            attributes: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let identity_artifact = ArtifactRepository::create(
        db.pool(),
        CreateArtifactInput {
            scope: OwnerType::Identity,
            owner: human.id.to_string(),
            ..input("migration.human_owned")
        },
    )
    .await
    .unwrap();
    IdentityRepository::delete(db.pool(), human.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Artifact(identity_artifact.id),
        99,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(target.owner_id(), Some(human.id));
    assert_eq!(target.context().owner_identity_id, Some(human.id));
    assert!(
        target.accepts_new_effects(),
        "Human deletion does not revoke retained artifact ownership"
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}
