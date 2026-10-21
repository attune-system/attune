use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct AcceptedControl {
    pub(super) receipt: Id,
    pub(super) site: InputSite,
}

impl AcceptedControl {
    pub(super) fn origin(&self) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: self.site.kind.origin_kind(self.site.purpose),
            entity_id: self.receipt,
            path: self.site.path.clone(),
        }
    }
}

fn valid_site(site: &InputSite, execution: Id) -> bool {
    execution > 0
        && site.execution == execution
        && site.kind == InputEntityKind::ExecutionConfig
        && site.purpose == InputSitePurpose::Control
        && site.path.is_empty()
}

pub(super) fn validate(body: &Body) -> Result<()> {
    if body.controls.len() > 256 {
        return Err(denied());
    }
    let mut previous = 0;
    for control in &body.controls {
        if control.receipt <= previous || !valid_site(&control.site, body.execution) {
            return Err(denied());
        }
        previous = control.receipt;
    }
    Ok(())
}

/// The indexed destination-site lookup is authoritative, not a caller's list.
pub(super) async fn load(
    conn: &mut PgConnection,
    execution: Id,
    admission: Id,
) -> Result<Vec<AcceptedControl>> {
    let rows: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT id,destination_admission FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='execution_config' AND destination_purpose='control' AND destination_path='' ORDER BY id LIMIT 257",
    )
    .bind(execution)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() > 256 {
        return Err(denied());
    }
    let mut controls = Vec::new();
    for (receipt, destination) in rows {
        let control = AcceptedControl {
            receipt,
            site: InputSite {
                execution,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Control,
                path: String::new(),
            },
        };
        if receipt <= 0 || destination != admission {
            return Err(denied());
        }
        let actual = InheritedInputRepository::receipt_source_site(conn, &control.origin()).await?;
        if actual != (receipt, control.site.clone()) {
            return Err(denied());
        }
        controls.push(control);
    }
    Ok(controls)
}

pub(super) async fn require_current(conn: &mut PgConnection, body: &Body) -> Result<()> {
    validate(body)?;
    if load(conn, body.execution, body.admission).await? != body.controls {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn reopen(
    conn: &mut PgConnection,
    body: &Body,
    key: &str,
) -> Result<Vec<ReceiptContext>> {
    require_current(conn, body).await?;
    let mut contexts = Vec::new();
    for control in &body.controls {
        // The graph reader can reopen an accepted ancestor through field().
        let context = Box::pin(InheritedInputRepository::receipt_context(
            conn,
            &control.site,
            &control.origin(),
            key,
        ))
        .await?;
        context.ensure_fresh_on(conn).await?;
        contexts.push(context);
    }
    Ok(contexts)
}

/// No source-graph read here: the issuing reader already held that graph before
/// destination locks. Match each stored root and its encrypted accepted value.
pub(super) async fn require_held(
    conn: &mut PgConnection,
    body: &Body,
    key: &str,
    contexts: &[ReceiptContext],
) -> Result<()> {
    validate(body)?;
    if contexts.len() != body.controls.len() {
        return Err(denied());
    }
    let mut seen = Vec::new();
    for context in contexts {
        let control = body
            .controls
            .iter()
            .find(|control| control.origin() == context.origin())
            .ok_or_else(denied)?;
        if context.site() != &control.site || seen.contains(&control.receipt) {
            return Err(denied());
        }
        seen.push(control.receipt);
        let (encrypted, key_hash): (Value, String) = sqlx::query_as(
            "SELECT encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1",
        )
        .bind(control.receipt)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        if key_hash != crate::crypto::hash_encryption_key(key)
            || crate::crypto::decrypt_json(&encrypted, key)? != *context.accepted_value()
        {
            return Err(denied());
        }
        context.ensure_fresh_on(conn).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Body {
        Body {
            execution: 1,
            admission: 2,
            config: serde_json::json!({}),
            fields: vec![],
            contributors: vec![],
            schema: serde_json::json!({}),
            controls: vec![],
        }
    }

    #[test]
    fn empty_controls_preserve_original_body_bytes() {
        let original = r#"{"execution":1,"admission":2,"config":{},"fields":[],"contributors":[],"schema":{}}"#;
        assert_eq!(serde_json::to_string(&body()).unwrap(), original);
        let decoded: Body = serde_json::from_str(original).unwrap();
        assert!(decoded.controls.is_empty());
        assert_eq!(serde_json::to_string(&decoded).unwrap(), original);
        assert_eq!(
            hash(&serde_json::to_value(&decoded).unwrap()).unwrap(),
            hash(&serde_json::from_str(original).unwrap()).unwrap()
        );
    }

    #[test]
    fn roots_require_positive_distinct_ids_and_exact_control_site() {
        let mut body = body();
        let control = AcceptedControl {
            receipt: 3,
            site: InputSite {
                execution: 1,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Control,
                path: String::new(),
            },
        };
        body.controls.push(control.clone());
        assert!(validate(&body).is_ok());
        let without = self::body();
        assert!(body != without);
        assert_ne!(
            hash(&serde_json::to_value(&body).unwrap()).unwrap(),
            hash(&serde_json::to_value(&without).unwrap()).unwrap()
        );
        body.controls.push(control);
        assert!(validate(&body).is_err());
        body.controls.pop();
        body.controls[0].receipt = 0;
        assert!(validate(&body).is_err());
        body.controls[0].receipt = 3;
        body.controls[0].site.purpose = InputSitePurpose::Value;
        assert!(validate(&body).is_err());
        body.controls[0].site.purpose = InputSitePurpose::Control;
        body.controls[0].site.execution = 4;
        assert!(validate(&body).is_err());
        body.controls[0].site.execution = 1;
        body.controls[0].site.path = "/helper".into();
        assert!(validate(&body).is_err());
    }
}
