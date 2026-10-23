mod helpers;

use attune_common::{
    inquiry_schema::{fingerprint_request, InquirySchemaBinding},
    models::{
        inquiry::{InquiryResponseOption, InquiryResponseOptionStyle},
        InquiryStatus,
    },
    repositories::{
        inquiry::{CreateInquiryInput, InquiryRepository},
        inquiry_schema::InquirySchemaRepository,
        Create, Delete,
    },
    value_schema::{compile, Limits, RegistryView, SchemaContext, SourceIdentity},
};
use helpers::create_test_pool;
use serde_json::json;

#[tokio::test]
async fn inquiry_contracts_are_immutable_digest_verified_and_owned_by_the_inquiry() {
    let db = create_test_pool().await.unwrap();
    let schema = json!({"approved": {"type": "boolean", "required": true}});
    let inquiry = InquiryRepository::create(
        &db,
        CreateInquiryInput {
            created_by_execution: 900001,
            prompt: "Approve?".into(),
            response_schema: Some(schema.clone()),
            response_options: vec![InquiryResponseOption {
                r#ref: "approve".into(),
                label: "Approve".into(),
                style: InquiryResponseOptionStyle::Default,
                response: json!({"approved": true}),
            }],
            assigned_to: None,
            status: InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await
    .unwrap();
    let contract = compile(
        SourceIdentity::Assigned {
            component_ref: "test.inquiry".into(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        SchemaContext::FlatDocument,
        &schema,
        &RegistryView::new(),
        Limits::default(),
    )
    .unwrap();
    let binding = InquirySchemaBinding {
        response_schema: Some(schema),
        contract,
    };
    let fingerprint = fingerprint_request(
        &json!({"purpose": "approval", "options": [true]}),
        "owned-test-key",
    )
    .unwrap();
    let original = InquirySchemaRepository::find(&db, inquiry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.binding.response_schema, binding.response_schema);
    let mut transaction = db.begin().await.unwrap();
    assert!(
        InquirySchemaRepository::attach(&mut transaction, inquiry.id, &binding, &fingerprint)
            .await
            .is_err()
    );
    InquirySchemaRepository::attach(
        &mut transaction,
        inquiry.id,
        &original.binding,
        &original.request_fingerprint,
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let stored = InquirySchemaRepository::find(&db, inquiry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.binding, original.binding);
    assert_eq!(stored.request_fingerprint, original.request_fingerprint);
    assert!(
        sqlx::query("UPDATE inquiry_schema_binding SET binding = '{}' WHERE entity_id = $1")
            .bind(inquiry.id)
            .execute(&db)
            .await
            .is_err()
    );
    InquiryRepository::delete(&db, inquiry.id).await.unwrap();
    assert!(InquirySchemaRepository::find(&db, inquiry.id)
        .await
        .unwrap()
        .is_none());
    db.cleanup().await.unwrap();
}
