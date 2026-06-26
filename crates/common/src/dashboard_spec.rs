use std::collections::BTreeSet;

use serde_json::Value as JsonValue;

pub fn validate_dashboard_spec(spec: &JsonValue) -> std::result::Result<(), String> {
    let spec_object = spec
        .as_object()
        .ok_or_else(|| "Dashboard spec must be a JSON object".to_string())?;

    let data_sources = spec_object
        .get("data_sources")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "Dashboard spec requires object 'data_sources'".to_string())?;

    let cards = spec_object
        .get("cards")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "Dashboard spec requires array 'cards'".to_string())?;

    let breakpoints = spec_object
        .get("layout")
        .and_then(JsonValue::as_object)
        .and_then(|layout| layout.get("breakpoints"))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "Dashboard spec requires object 'layout.breakpoints'".to_string())?;

    if breakpoints.is_empty() {
        return Err(
            "Dashboard spec 'layout.breakpoints' must define at least one breakpoint".to_string(),
        );
    }

    let required_breakpoints: BTreeSet<&str> = breakpoints.keys().map(String::as_str).collect();

    if let Some(filters) = spec_object.get("filters") {
        let filter_array = filters
            .as_array()
            .ok_or_else(|| "Dashboard spec 'filters' must be an array".to_string())?;
        let mut filter_ids = BTreeSet::new();
        for filter in filter_array {
            let filter_object = filter
                .as_object()
                .ok_or_else(|| "Dashboard spec filter entries must be objects".to_string())?;
            let filter_id = filter_object
                .get("id")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "Dashboard spec filter entries require string 'id'".to_string())?;
            if !filter_ids.insert(filter_id.to_string()) {
                return Err(format!(
                    "Dashboard spec has duplicate filter id '{}'",
                    filter_id
                ));
            }
        }
    }

    let mut card_ids = BTreeSet::new();
    for card in cards {
        let card_object = card
            .as_object()
            .ok_or_else(|| "Dashboard card entries must be objects".to_string())?;
        let card_id = card_object
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "Dashboard card entries require string 'id'".to_string())?;

        if !card_ids.insert(card_id.to_string()) {
            return Err(format!(
                "Dashboard spec has duplicate card id '{}'",
                card_id
            ));
        }

        let source_id = card_object
            .get("source")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| format!("Dashboard card '{}' requires string 'source'", card_id))?;
        if !data_sources.contains_key(source_id) {
            return Err(format!(
                "Dashboard card '{}' references unknown source '{}'",
                card_id, source_id
            ));
        }

        let position = card_object
            .get("position")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| format!("Dashboard card '{}' requires object 'position'", card_id))?;

        for breakpoint in &required_breakpoints {
            if !position.contains_key(*breakpoint) {
                return Err(format!(
                    "Dashboard card '{}' missing position for breakpoint '{}'",
                    card_id, breakpoint
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_spec() -> JsonValue {
        json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "event_count": { "type": "event_count" }
            },
            "filters": [
                { "id": "pack", "type": "pack_ref" }
            ],
            "cards": [
                {
                    "id": "events",
                    "source": "event_count",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        })
    }

    #[test]
    fn validates_valid_spec() {
        assert!(validate_dashboard_spec(&valid_spec()).is_ok());
    }

    #[test]
    fn rejects_duplicate_card_ids() {
        let mut spec = valid_spec();
        spec["cards"] = json!([
            { "id": "events", "source": "event_count", "position": { "lg": {}, "sm": {} } },
            { "id": "events", "source": "event_count", "position": { "lg": {}, "sm": {} } }
        ]);
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard spec has duplicate card id 'events'"
        );
    }

    #[test]
    fn rejects_duplicate_filter_ids() {
        let mut spec = valid_spec();
        spec["filters"] = json!([
            { "id": "pack", "type": "pack_ref" },
            { "id": "pack", "type": "pack_ref" }
        ]);
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard spec has duplicate filter id 'pack'"
        );
    }

    #[test]
    fn rejects_missing_breakpoint_position() {
        let mut spec = valid_spec();
        spec["cards"][0]["position"] = json!({
            "lg": { "x": 0, "y": 0, "w": 6, "h": 4 }
        });
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard card 'events' missing position for breakpoint 'sm'"
        );
    }
}
