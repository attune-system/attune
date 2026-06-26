use serde_json::{json, Value};

pub fn sample_dashboard_data_request() -> Value {
    json!({
        "filters": {},
        "include_meta": true,
        "request_id": "acceptance-test"
    })
}

pub fn dashboard_spec(
    sources: &[(&str, &str)],
    cards: &[(&str, &str)],
    filters: Option<Value>,
) -> Value {
    let mut data_sources = serde_json::Map::new();
    for (source_id, source_type) in sources {
        data_sources.insert(source_id.to_string(), json!({ "type": source_type }));
    }

    let cards = cards
        .iter()
        .enumerate()
        .map(|(index, (card_id, source_id))| {
            json!({
                "id": card_id,
                "source": source_id,
                "position": {
                    "lg": { "x": 0, "y": index as i64 * 4, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": index as i64 * 4, "w": 4, "h": 4 }
                }
            })
        })
        .collect::<Vec<_>>();

    let mut spec = json!({
        "layout": {
            "breakpoints": {
                "lg": { "min_width": 1280, "columns": 12 },
                "sm": { "min_width": 0, "columns": 4 }
            }
        },
        "data_sources": Value::Object(data_sources),
        "cards": cards
    });

    if let Some(filters) = filters {
        spec["filters"] = filters;
    }

    spec
}

pub fn assert_source_order(body: &Value, expected_source_ids: &[&str]) {
    let actual: Vec<String> = body["data"]["sources"]
        .as_array()
        .expect("data.sources must be an array")
        .iter()
        .map(|source| {
            source["source_id"]
                .as_str()
                .expect("source_id must be present")
                .to_string()
        })
        .collect();

    let expected: Vec<String> = expected_source_ids.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        actual, expected,
        "sources must use canonical source_id ordering"
    );
}

pub fn assert_required_source_meta_fields(source: &Value) {
    let meta = &source["meta"];
    assert!(meta["authorization_mode"].is_string());
    assert!(meta["freshness_mode"].is_string());
    assert!(meta.get("aggregate_watermark").is_some());
    assert!(meta["cache_hit"].is_boolean());
    assert!(meta.get("bucket_size").is_some());
    assert!(meta["truncated"].is_boolean());
    assert!(meta["unit_hints"].is_object());
    assert!(meta["ordering"].is_array());
    assert!(meta.get("authorized_refs").is_some());
}

pub fn source_by_id<'a>(body: &'a Value, source_id: &str) -> &'a Value {
    body["sources"]
        .as_array()
        .expect("sources must be an array")
        .iter()
        .find(|source| source["source_id"].as_str() == Some(source_id))
        .unwrap_or_else(|| panic!("source '{}' not found", source_id))
}
