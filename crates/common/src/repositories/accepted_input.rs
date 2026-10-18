//! First typed receipts for accepted contributor material. Acceptance is sealed
//! once, in the admission transaction. Inheritance reads ciphertext and frozen
//! contributor evidence, never a current Key or a human's IAM assignments.
use super::{
    execution_secret_value::ExecutionSecretValueRepository,
    inherited_input::{
        InheritedInputRepository, InputEntityKind, InputSite, InputSitePurpose, PreparedInputSource,
    },
    runtime_authority::{ApprovalEffectFence, RuntimeAdmissionRecord, RuntimeAuthorityRepository},
    schema_contract::{BindingOwner, SchemaContractRepository},
};
use crate::{
    models::Id,
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{PreparedSecretValue, StoredSecretValue},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;

pub const ENTITY_ACCEPTED_INPUT: &str = "accepted_input";
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Field {
    json_path: String,
    encrypted_value: Value,
    encryption_key_hash: Option<String>,
    source_kind: String,
    source_ref: Option<String>,
}
impl Field {
    fn stored(&self) -> StoredSecretValue {
        StoredSecretValue {
            json_path: self.json_path.clone(),
            encrypted_value: self.encrypted_value.clone(),
            encryption_key_hash: self.encryption_key_hash.clone(),
            source_kind: self.source_kind.clone(),
            source_ref: self.source_ref.clone(),
        }
    }
    fn provenance(&self) -> Result<SecretProvenance> {
        if self.source_kind == "provenance" {
            return serde_json::from_str(self.source_ref.as_deref().ok_or_else(denied)?)
                .map_err(|_| denied());
        }
        if self.source_kind == "parameter_schema"
            && self.source_ref.as_deref() == Some(self.json_path.as_str())
        {
            return Ok(crate::secret_values::SecretSource::ParameterSchema {
                path: self.json_path.clone(),
            }
            .provenance());
        }
        Err(denied())
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcceptedContributorManifest {
    kind: String,
    item: Id,
    name: String,
    expected: Id,
    body: Value,
    digest: String,
}
impl AcceptedContributorManifest {
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn item(&self) -> Id {
        self.item
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn expected(&self) -> Id {
        self.expected
    }
    pub fn body(&self) -> &Value {
        &self.body
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.body.is_null() || hash(&self.body)? != self.digest {
            return Err(denied());
        }
        Ok(())
    }
}
type Contributor = AcceptedContributorManifest;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Body {
    execution: Id,
    admission: Id,
    config: Value,
    fields: Vec<Field>,
    contributors: Vec<Contributor>,
    schema: Value,
}

pub(crate) struct AcceptedFieldEvidence {
    pub(crate) accepted: Id,
    pub(crate) admission: RuntimeAdmissionRecord,
    pub(crate) value: Value,
    pub(crate) provenance: SecretProvenance,
    pub(crate) fence: ApprovalEffectFence,
    pub(crate) parents: Vec<(Id, InputSite)>,
}
pub struct AcceptedInputRepository;
impl AcceptedInputRepository {
    /// Revalidate a completed original acceptance without creating, repairing or
    /// resealing it. Public empty input still requires its genuine contributors.
    pub async fn revalidate_execution(
        conn: &mut PgConnection,
        execution: Id,
        key: &str,
    ) -> Result<ApprovalEffectFence> {
        let record = RuntimeAuthorityRepository::execution_admission(conn, execution)
            .await?
            .ok_or_else(denied)?;
        let (id, body, expected, key_hash) = saved(conn, record.id).await?.ok_or_else(denied)?;
        validate_saved(&body, &expected, &key_hash, key)?;
        let original: Body = serde_json::from_value(body)?;
        let current = load_body(conn, execution, record.id)
            .await?
            .ok_or_else(denied)?;
        if original != current || fields(conn, ENTITY_ACCEPTED_INPUT, id).await? != original.fields
        {
            return Err(denied());
        }
        Self::source_contributors(conn, execution).await?;
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(fence)
    }
    /// Observe complete independent original ceilings, never current human IAM.
    /// This is not input DELIVERY to the source: terminal execution observations
    /// retain their source admission and original consents without a live API
    /// token or permission to deliver new parameters to the completed producer.
    /// The native observer archives these manifests in its authenticated body.
    pub(crate) async fn source_contributors(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<(
        Vec<crate::delegation::DelegationAuthority>,
        Vec<AcceptedContributorManifest>,
    )> {
        let record = RuntimeAuthorityRepository::execution_admission(conn, execution)
            .await?
            .ok_or_else(denied)?;
        let manifests = contributors(conn, execution, record.id).await?;
        for manifest in &manifests {
            manifest.validate()?;
        }
        let source_fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let mut authorities = Vec::new();
        let queue = manifests.iter().any(|manifest| manifest.kind == "queue");
        let human = manifests
            .iter()
            .any(|manifest| manifest.kind == "execution");
        if queue {
            let retained=super::recorded_input_authority::RecordedInputAuthorityRepository::queue_authorities(conn,execution).await?.ok_or_else(denied)?;
            if retained.is_empty() {
                return Err(denied());
            }
            authorities.extend(retained);
        }
        if human {
            authorities.push(
                super::recorded_input_authority::RecordedInputAuthorityRepository::load(
                    conn,
                    "execution",
                    execution,
                )
                .await?
                .ok_or_else(denied)?,
            );
        }
        if manifests.iter().any(|manifest| manifest.kind == "rule") {
            authorities.push(
                super::rule_authority::RuleAuthorityRepository::runtime(
                    conn,
                    "execution",
                    execution,
                )
                .await?
                .ok_or_else(denied)?
                .input_authority(conn)
                .await?,
            );
        }
        if authorities.is_empty() || authorities.len() > 256 || authorities.len() != manifests.len()
        {
            return Err(denied());
        }
        source_fence.ensure_fresh_on(conn).await?;
        Ok((authorities, manifests))
    }
    pub(crate) async fn retention_evidence(
        conn: &mut PgConnection,
        id: Id,
        site: &InputSite,
    ) -> Result<(Id, Vec<SecretOrigin>, usize, Vec<Id>)> {
        let row: Option<(Id, Value, String)> = sqlx::query_as(
            "SELECT admission,body,body_digest FROM runtime_accepted_input WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let (admission, body, expected) = row.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let mut bytes = serde_json::to_vec(&body)?.len();
        let body: Body = serde_json::from_value(body)?;
        if body.admission != admission
            || body.execution != site.execution
            || site.kind != InputEntityKind::ExecutionConfig
            || site.purpose != InputSitePurpose::Value
            || fields(conn, ENTITY_ACCEPTED_INPUT, id).await? != body.fields
        {
            return Err(denied());
        }
        bytes += serde_json::to_vec(&body.fields)?.len();
        let mut origins = Vec::new();
        let mut parents = Vec::new();
        for field in body.fields {
            for origin in field.provenance()?.origins {
                if overlaps(&field.json_path, &site.path) && typed_receipt(&origin) {
                    let (parent, _) =
                        InheritedInputRepository::receipt_source_site(conn, &origin).await?;
                    if !parents.contains(&parent) {
                        parents.push(parent);
                    }
                }
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
        }
        if origins.len() > 256 {
            return Err(denied());
        }
        Ok((admission, origins, bytes, parents))
    }
    /// After ALL contributors/schema/config/encrypted fields and the batch
    /// manifest are committed to this transaction, before consuming authority.
    pub async fn seal_execution(
        conn: &mut PgConnection,
        execution: Id,
        encryption_key: &str,
    ) -> Result<()> {
        let record = RuntimeAuthorityRepository::execution_admission(conn, execution)
            .await?
            .ok_or_else(denied)?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let body = load_body(conn, execution, record.id)
            .await?
            .ok_or_else(denied)?;
        bounded(&serde_json::to_value(&body)?)?;
        if let Some((id, saved, expected, key_hash)) = saved(conn, record.id).await? {
            validate_saved(&saved, &expected, &key_hash, encryption_key)?;
            if serde_json::to_value(&body)? != saved {
                return Err(denied());
            }
            Self::publish_first_receipts(conn, id, execution, &body.fields, encryption_key).await?;
            return fence.ensure_fresh_on(conn).await;
        }
        let original:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_accepted_input_epoch WHERE admission = $1 AND transaction_id = pg_current_xact_id()::TEXT)")
            .bind(record.id).fetch_one(&mut *conn).await?;
        if !original {
            return Err(denied());
        }
        let mut authorities =
            super::execution_runtime_admission::ExecutionAdmissionRepository::input_authorities(
                conn, execution, None,
            )
            .await?;
        // Canonical loading chooses the dispatch frontier. Preserve any other
        // accepted root snapshots as independent ceilings too, never a union.
        let queue_frontier = body
            .contributors
            .iter()
            .any(|contributor| contributor.kind == "queue" && !contributor.body.is_null());
        let human_frontier = body
            .contributors
            .iter()
            .any(|contributor| contributor.kind == "execution");
        if queue_frontier && human_frontier {
            if let Some(authority) =
                super::recorded_input_authority::RecordedInputAuthorityRepository::load(
                    conn,
                    "execution",
                    execution,
                )
                .await?
            {
                authorities.push(authority);
            }
        }
        if (queue_frontier || human_frontier)
            && body
                .contributors
                .iter()
                .any(|contributor| contributor.kind == "rule")
        {
            let snapshot = super::rule_authority::RuleAuthorityRepository::runtime(
                conn,
                "execution",
                execution,
            )
            .await?
            .ok_or_else(denied)?;
            authorities.push(snapshot.input_authority(conn).await?);
        }
        if authorities.len() > 256 || body.contributors.is_empty() {
            return Err(denied());
        }
        let schema = SchemaContractRepository::find(conn, BindingOwner::Execution, execution)
            .await?
            .ok_or_else(denied)?;
        schema.validate()?;
        let compiled =
            crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| denied())?;
        let restored = restore(&body, encryption_key)?;
        compiled.validate(&restored).map_err(|_| denied())?;
        let local_paths = compiled.secret_paths(&restored).map_err(|_| denied())?;
        let mut required = Vec::new();
        for field in &body.fields {
            let provenance = field.provenance()?;
            let declared_origins = provenance.origins.clone();
            if provenance.origins.is_empty() {
                return Err(denied());
            }
            let mut authored = Vec::new();
            let mut value_source = false;
            for origin in &provenance.origins {
                if typed_receipt(origin) {
                    let (_, site) =
                        InheritedInputRepository::receipt_source_site(conn, origin).await?;
                    let context = InheritedInputRepository::receipt_context(
                        conn,
                        &site,
                        origin,
                        encryption_key,
                    )
                    .await?;
                    context.require_published(conn).await?;
                    if site.purpose == InputSitePurpose::Value {
                        value_source = true;
                        if Some(context.accepted_value()) != restored.pointer(&field.json_path) {
                            return Err(denied());
                        }
                        if site.execution != execution
                            || site.kind != InputEntityKind::ExecutionConfig
                            || site.path != field.json_path
                        {
                            let source = InheritedInputRepository::prepare_source(
                                conn,
                                execution,
                                &site,
                                origin,
                                encryption_key,
                            )
                            .await?;
                            if Some(source.value()) != restored.pointer(&field.json_path) {
                                return Err(denied());
                            }
                        }
                        authored.extend(context.source_origins().iter().cloned());
                    }
                    fence = fence.intersect(context.accepted_fence());
                }
            }
            for origin in provenance.origins {
                if typed_receipt(&origin) || authored.contains(&origin) {
                    continue;
                }
                match &origin {
                    SecretOrigin::Local {
                        source_kind,
                        source_ref,
                    } if source_kind == "parameter_schema"
                        && source_ref.as_deref() == Some(field.json_path.as_str())
                        && local_paths
                            .iter()
                            .any(|path| contains(path, &field.json_path)) =>
                    {
                        value_source = true;
                    }
                    SecretOrigin::QueueItem {
                        queue_id,
                        queue_ref,
                        pack_ref,
                        item_id,
                        path,
                    } if body.contributors.iter().any(|contributor| {
                        contributor.kind == "queue" && contributor.item == *item_id
                    }) =>
                    {
                        // Caller-supplied queue payload needs no invented Read
                        // grant on another contributor's data. It must be this
                        // accepted batch's exact original item and field.
                        validate_local_queue_source(
                            conn,
                            *queue_id,
                            queue_ref,
                            pack_ref.as_deref(),
                            *item_id,
                            path,
                            encryption_key,
                            &declared_origins,
                        )
                        .await?;
                        value_source = true;
                    }
                    SecretOrigin::Local { .. } => return Err(denied()),
                    _ => {
                        value_source = true;
                        if !required.contains(&origin) {
                            required.push(origin);
                        }
                    }
                }
            }
            if !value_source {
                return Err(denied());
            }
        }
        // Includes Keys, config and generic frozen read origins, EACH original
        // contributor independently. No API-ref or current-IAM fallback.
        if !required.is_empty()
            && body
                .contributors
                .iter()
                .any(|contributor| contributor.kind == "queue" && contributor.body.is_null())
        {
            return Err(denied());
        }
        super::immutable_input_origin::ImmutableInputOriginRepository::require(
            conn,
            &authorities,
            &required,
        )
        .await?;
        let value = serde_json::to_value(&body)?;
        bounded(&value)?;
        let id:Id=sqlx::query_scalar("INSERT INTO runtime_accepted_input(execution,admission,body,body_digest,encryption_key_hash) VALUES ($1,$2,$3,$4,$5) RETURNING id")
            .bind(execution).bind(record.id).bind(&value).bind(hash(&value)?).bind(crate::crypto::hash_encryption_key(encryption_key)).fetch_one(&mut *conn).await?;
        let archived = body
            .fields
            .iter()
            .map(|field| {
                Ok(PreparedSecretValue {
                    json_path: field.json_path.clone(),
                    encrypted_value: field.encrypted_value.clone(),
                    encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(denied)?,
                    source_kind: field.source_kind.clone(),
                    source_ref: field.source_ref.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            conn,
            ENTITY_ACCEPTED_INPUT,
            id,
            &archived,
        )
        .await?;
        Self::publish_first_receipts(conn, id, execution, &body.fields, encryption_key).await?;
        fence.ensure_fresh_on(conn).await
    }

    async fn publish_first_receipts(
        conn: &mut PgConnection,
        accepted: Id,
        execution: Id,
        fields: &[Field],
        key: &str,
    ) -> Result<()> {
        for field in fields {
            let site = InputSite {
                execution,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Value,
                path: field.json_path.clone(),
            };
            let evidence = Self::field(conn, &site, key).await?;
            let context =
                InheritedInputRepository::record_accepted(conn, &site, evidence, key).await?;
            let SecretOrigin::Entity { entity_id, .. } = context.origin() else {
                return Err(denied());
            };
            sqlx::query("INSERT INTO runtime_accepted_input_delivery(accepted,json_path,receipt) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING").bind(accepted).bind(&field.json_path).bind(entity_id).execute(&mut *conn).await?;
            let receipt:Id=sqlx::query_scalar("SELECT receipt FROM runtime_accepted_input_delivery WHERE accepted = $1 AND json_path = $2").bind(accepted).bind(&field.json_path).fetch_one(&mut *conn).await?;
            let provenance = delivery_provenance(receipt, &field.json_path);
            let delivered = PreparedSecretValue {
                json_path: field.json_path.clone(),
                encrypted_value: field.encrypted_value.clone(),
                encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(denied)?,
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&provenance)?),
            };
            ExecutionSecretValueRepository::upsert_many_with_conn(
                conn,
                InputEntityKind::ExecutionConfig.as_str(),
                execution,
                &[delivered],
            )
            .await?;
        }
        Ok(())
    }

    /// The source is always the original accepted CONFIG field. A result or
    /// variables pointer cannot borrow its receipt, even at the same path.
    pub async fn prepare_source(
        conn: &mut PgConnection,
        renderer: Id,
        site: &InputSite,
        original: &SecretProvenance,
        encryption_key: &str,
    ) -> Result<PreparedInputSource> {
        let evidence = Self::field(conn, site, encryption_key).await?;
        if &evidence.provenance != original {
            return Err(denied());
        }
        let receipt =
            InheritedInputRepository::record_accepted(conn, site, evidence, encryption_key).await?;
        InheritedInputRepository::prepare_source(
            conn,
            renderer,
            site,
            &receipt.origin(),
            encryption_key,
        )
        .await
    }

    pub(crate) async fn field(
        conn: &mut PgConnection,
        site: &InputSite,
        encryption_key: &str,
    ) -> Result<AcceptedFieldEvidence> {
        if site.kind != InputEntityKind::ExecutionConfig
            || site.purpose != InputSitePurpose::Value
            || site.execution <= 0
        {
            return Err(denied());
        }
        let row:Option<(Id,Id,Value,String,String)>=sqlx::query_as("SELECT id,admission,body,body_digest,encryption_key_hash FROM runtime_accepted_input WHERE execution = $1")
            .bind(site.execution).fetch_optional(&mut *conn).await?;
        let (id, admission, body, expected, key_hash) = row.ok_or_else(denied)?;
        validate_saved(&body, &expected, &key_hash, encryption_key)?;
        let body: Body = serde_json::from_value(body)?;
        if body.execution != site.execution || body.admission != admission {
            return Err(denied());
        }
        let record =
            RuntimeAuthorityRepository::require_retained_admission(conn, admission).await?;
        if record.owner_kind != "execution" || record.owner_id != site.execution {
            return Err(denied());
        }
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, admission).await?;
        if let Some(current) = load_body(conn, site.execution, admission).await? {
            if current != body {
                return Err(denied());
            }
        }
        let archived = fields(conn, ENTITY_ACCEPTED_INPUT, id).await?;
        if archived != body.fields {
            return Err(denied());
        }
        let selected = body
            .fields
            .iter()
            .filter(|field| overlaps(&field.json_path, &site.path))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err(denied());
        }
        let mut provenance = SecretProvenance::default();
        for field in selected {
            provenance.merge(field.provenance()?);
        }
        if provenance.origins.len() > 256 {
            return Err(denied());
        }
        let value = restore(&body, encryption_key)?
            .pointer(&site.path)
            .cloned()
            .ok_or_else(denied)?;
        let mut parents = Vec::new();
        for origin in &provenance.origins {
            if typed_receipt(origin) {
                let parent = InheritedInputRepository::receipt_source_site(conn, origin).await?;
                if !parents.contains(&parent) {
                    parents.push(parent);
                }
            }
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(AcceptedFieldEvidence {
            accepted: id,
            admission: record,
            value,
            provenance,
            fence,
            parents,
        })
    }
}

async fn saved(
    conn: &mut PgConnection,
    admission: Id,
) -> Result<Option<(Id, Value, String, String)>> {
    Ok(sqlx::query_as("SELECT id,body,body_digest,encryption_key_hash FROM runtime_accepted_input WHERE admission = $1").bind(admission).fetch_optional(conn).await?)
}
fn validate_saved(body: &Value, expected: &str, key_hash: &str, key: &str) -> Result<()> {
    bounded(body)?;
    if hash(body)? != expected || key_hash != crate::crypto::hash_encryption_key(key) {
        return Err(denied());
    }
    Ok(())
}
async fn load_body(conn: &mut PgConnection, execution: Id, admission: Id) -> Result<Option<Body>> {
    let config: Option<Option<Value>> =
        sqlx::query_scalar("SELECT config FROM execution WHERE id = $1")
            .bind(execution)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(config) = config else {
        return Ok(None);
    };
    let config = config.ok_or_else(denied)?;
    let mut fields = fields(conn, InputEntityKind::ExecutionConfig.as_str(), execution).await?;
    normalize_delivered(conn, execution, admission, &mut fields).await?;
    let contributors = contributors(conn, execution, admission).await?;
    let schema:Option<(Value,String)>=sqlx::query_as("SELECT contract,binding_digest FROM runtime_schema_binding WHERE entity_kind = 'execution' AND entity_id = $1").bind(execution).fetch_optional(&mut *conn).await?;
    let (schema, expected) = schema.ok_or_else(denied)?;
    if hash(&schema)? != expected {
        return Err(denied());
    }
    Ok(Some(Body {
        execution,
        admission,
        config,
        fields,
        contributors,
        schema,
    }))
}
async fn fields(conn: &mut PgConnection, kind: &str, id: Id) -> Result<Vec<Field>> {
    let values =
        ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, kind, id).await?;
    if values.len() > 256 {
        return Err(denied());
    }
    Ok(values
        .into_iter()
        .map(|field| Field {
            json_path: field.json_path,
            encrypted_value: field.encrypted_value,
            encryption_key_hash: field.encryption_key_hash,
            source_kind: field.source_kind,
            source_ref: field.source_ref,
        })
        .collect())
}
async fn contributors(
    conn: &mut PgConnection,
    execution: Id,
    admission: Id,
) -> Result<Vec<Contributor>> {
    let rows:Vec<(String,Id,String,Id,Value,String)>=sqlx::query_as("SELECT kind,item,name,expected,body,digest FROM (SELECT 'execution'::TEXT AS kind,0::BIGINT AS item,''::TEXT AS name,1::BIGINT AS expected,authority AS body,authority_digest AS digest FROM recorded_execution_input_authority WHERE owner_kind = 'execution' AND owner_id = $1 UNION ALL SELECT 'rule'::TEXT,0::BIGINT,''::TEXT,1::BIGINT,authority,authority_digest FROM runtime_input_authority WHERE owner_kind = 'execution' AND owner_id = $1 UNION ALL SELECT 'queue'::TEXT,item,contributor,expected_items::BIGINT,authority,authority_digest FROM recorded_queue_execution_input WHERE execution = $1) originals ORDER BY kind,item,name LIMIT 257")
        .bind(execution).fetch_all(&mut *conn).await?;
    if rows.len() > 256 {
        return Err(denied());
    }
    let authority: Value =
        sqlx::query_scalar("SELECT authority FROM runtime_authority_admission WHERE id = $1")
            .bind(admission)
            .fetch_one(&mut *conn)
            .await?;
    let authority: crate::runtime_authority::SelectedRuntimeAuthority =
        serde_json::from_value(authority)?;
    let items = if let crate::runtime_authority::RequestAttribution::QueueBatch { items, .. } =
        authority.attribution
    {
        items.keys().copied().collect::<Vec<_>>()
    } else {
        vec![]
    };
    let missing = items
        .iter()
        .copied()
        .filter(|item| !rows.iter().any(|row| row.0 == "queue" && row.1 == *item))
        .collect::<Vec<_>>();
    let queue_count = rows.iter().filter(|row| row.0 == "queue").count() + missing.len();
    if !items.is_empty()
        && rows
            .iter()
            .any(|row| row.0 == "queue" && !items.contains(&row.1))
    {
        return Err(denied());
    }
    if items.is_empty() && rows.iter().any(|row| row.0 == "queue") {
        return Err(denied());
    }
    if items.is_empty() && rows.is_empty() {
        return Err(denied());
    }
    let mut result = Vec::new();
    for (kind, item, name, expected, body, digest) in rows {
        if hash(&body)? != digest
            || (kind == "queue" && usize::try_from(expected).ok() != Some(queue_count))
        {
            return Err(denied());
        }
        result.push(Contributor {
            kind,
            item,
            name,
            expected,
            body,
            digest,
        });
    }
    for item in missing {
        result.push(Contributor {
            kind: "queue".into(),
            item,
            name: "$missing".into(),
            expected: queue_count as Id,
            body: Value::Null,
            digest: hash(&Value::Null)?,
        });
    }
    result.sort_by(|a, b| (&a.kind, a.item, &a.name).cmp(&(&b.kind, b.item, &b.name)));
    if result.is_empty() || result.len() > 256 {
        return Err(denied());
    }
    Ok(result)
}
fn restore(body: &Body, key: &str) -> Result<Value> {
    let expected = crate::crypto::hash_encryption_key(key);
    if body
        .fields
        .iter()
        .any(|field| field.encryption_key_hash.as_deref() != Some(expected.as_str()))
    {
        return Err(denied());
    }
    crate::secret_values::restore_secret_values(
        body.config.clone(),
        &body.fields.iter().map(Field::stored).collect::<Vec<_>>(),
        key,
    )
}
fn delivery_provenance(receipt: Id, path: &str) -> SecretProvenance {
    SecretProvenance {
        origins: vec![SecretOrigin::Entity {
            entity_type: "declared_input:execution_config:value".into(),
            entity_id: receipt,
            path: path.into(),
        }],
        templates: vec![],
    }
}
async fn normalize_delivered(
    conn: &mut PgConnection,
    execution: Id,
    admission: Id,
    current: &mut [Field],
) -> Result<()> {
    let Some((id, body, expected, _)) = saved(conn, admission).await? else {
        return Ok(());
    };
    if hash(&body)? != expected {
        return Err(denied());
    }
    let body: Body = serde_json::from_value(body)?;
    if body.execution != execution || body.admission != admission {
        return Err(denied());
    }
    let mappings:Vec<(String,Id)>=sqlx::query_as("SELECT json_path,receipt FROM runtime_accepted_input_delivery WHERE accepted = $1 ORDER BY json_path").bind(id).fetch_all(&mut *conn).await?;
    for field in current {
        let original = body
            .fields
            .iter()
            .find(|original| original.json_path == field.json_path)
            .ok_or_else(denied)?;
        if let Some((_, receipt)) = mappings.iter().find(|(path, _)| path == &field.json_path) {
            let reference = delivery_provenance(*receipt, &field.json_path);
            let linked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_accepted WHERE receipt = $1 AND accepted = $2)").bind(receipt).bind(id).fetch_one(&mut *conn).await?;
            let (_, site) =
                InheritedInputRepository::receipt_source_site(conn, &reference.origins[0]).await?;
            if !linked
                || site.execution != execution
                || site.kind != InputEntityKind::ExecutionConfig
                || site.purpose != InputSitePurpose::Value
                || site.path != field.json_path
                || field.source_kind != "provenance"
                || field.source_ref.as_deref() != Some(serde_json::to_string(&reference)?.as_str())
            {
                return Err(denied());
            }
            field.source_kind = original.source_kind.clone();
            field.source_ref = original.source_ref.clone();
        }
        if &*field != original {
            return Err(denied());
        }
    }
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "Validate each recorded queue identity and provenance field explicitly"
)]
async fn validate_local_queue_source(
    conn: &mut PgConnection,
    queue: Id,
    reference: &str,
    pack: Option<&str>,
    item: Id,
    path: &str,
    key: &str,
    declared: &[SecretOrigin],
) -> Result<()> {
    let row:Option<(Id,String,Option<String>,Value)>=sqlx::query_as("SELECT i.queue,i.queue_ref,q.pack_ref,i.payload FROM work_queue_item i JOIN work_queue q ON q.id = i.queue WHERE i.id = $1 FOR SHARE OF i,q").bind(item).fetch_optional(&mut *conn).await?;
    let (actual, actual_ref, actual_pack, payload) = row.ok_or_else(denied)?;
    if actual != queue || actual_ref != reference || actual_pack.as_deref() != pack {
        return Err(denied());
    }
    // A local queue origin cannot hide a Key/generic parent behind a queue label.
    let source = fields(conn, crate::secret_values::ENTITY_QUEUE_ITEM_PAYLOAD, item).await?;
    let restored = crate::secret_values::restore_secret_values(
        payload,
        &source.iter().map(Field::stored).collect::<Vec<_>>(),
        key,
    )?;
    if restored.pointer(path).is_none() {
        return Err(denied());
    }
    for field in source
        .iter()
        .filter(|field| overlaps(&field.json_path, path))
    {
        for origin in field.provenance()?.origins {
            match origin {
                SecretOrigin::Local { source_kind, .. } if source_kind == "parameter_schema" => {}
                SecretOrigin::QueueItem {
                    item_id,
                    queue_id,
                    path: original,
                    ..
                } if item_id == item && queue_id == queue && original == path => {}
                other if declared.contains(&other) => {}
                _ => return Err(denied()),
            }
        }
    }
    Ok(())
}
fn contains(parent: &str, path: &str) -> bool {
    parent == path
        || parent.is_empty()
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn overlaps(a: &str, b: &str) -> bool {
    contains(a, b) || contains(b, a)
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| denied())
}
fn bounded(value: &Value) -> Result<()> {
    if serde_json::to_vec(value)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    Ok(())
}
fn denied() -> Error {
    Error::PermissionDenied(
        "Accepted input field, contributor manifest or acceptance seal does not match".into(),
    )
}
fn typed_receipt(origin: &SecretOrigin) -> bool {
    matches!(origin,SecretOrigin::Entity {entity_type,..} if entity_type.starts_with("declared_input:"))
}
