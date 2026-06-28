use std::collections::{BTreeMap, BTreeSet};

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

    let breakpoint_columns = parse_breakpoint_columns(breakpoints)?;
    let required_breakpoints: BTreeSet<&str> =
        breakpoint_columns.keys().map(String::as_str).collect();

    let mut filter_ids = BTreeSet::new();
    if let Some(filters) = spec_object.get("filters") {
        let filter_array = filters
            .as_array()
            .ok_or_else(|| "Dashboard spec 'filters' must be an array".to_string())?;
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

    for (source_id, source_value) in data_sources {
        let source_object = source_value
            .as_object()
            .ok_or_else(|| format!("Dashboard source '{}' must be an object", source_id))?;
        if let Some(params) = source_object.get("params") {
            validate_source_params(source_id, params, &filter_ids)?;
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

        validate_card_positions(
            card_id,
            position,
            &required_breakpoints,
            &breakpoint_columns,
        )?;
    }

    Ok(())
}

fn parse_breakpoint_columns(
    breakpoints: &serde_json::Map<String, JsonValue>,
) -> std::result::Result<BTreeMap<String, i64>, String> {
    let mut breakpoint_columns = BTreeMap::new();
    for (breakpoint, value) in breakpoints {
        let breakpoint_object = value.as_object().ok_or_else(|| {
            format!(
                "Dashboard layout breakpoint '{}' must be an object",
                breakpoint
            )
        })?;
        let columns = breakpoint_object
            .get("columns")
            .and_then(JsonValue::as_i64)
            .ok_or_else(|| {
                format!(
                    "Dashboard layout breakpoint '{}' requires positive integer 'columns'",
                    breakpoint
                )
            })?;
        if columns <= 0 {
            return Err(format!(
                "Dashboard layout breakpoint '{}' requires positive integer 'columns'",
                breakpoint
            ));
        }
        breakpoint_columns.insert(breakpoint.clone(), columns);
    }
    Ok(breakpoint_columns)
}

fn validate_source_params(
    source_id: &str,
    params: &JsonValue,
    declared_filters: &BTreeSet<String>,
) -> std::result::Result<(), String> {
    let params_object = params.as_object().ok_or_else(|| {
        format!(
            "Dashboard source '{}' param block must be an object",
            source_id
        )
    })?;

    for (key, value) in params_object {
        match value {
            JsonValue::String(_) => {
                if let Some(filter_id) = parse_source_filter_template(value) {
                    if !declared_filters.contains(filter_id) {
                        return Err(format!(
                            "Dashboard source '{}' param '{}' references unknown filter '{}'",
                            source_id, key, filter_id
                        ));
                    }
                }
            }
            JsonValue::Array(values) => {
                for entry in values {
                    if parse_source_filter_template(entry).is_some() {
                        return Err(format!(
                            "Dashboard source '{}' param '{}' template values are only supported as a single string",
                            source_id, key
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn parse_source_filter_template(value: &JsonValue) -> Option<&str> {
    let template = value.as_str()?.trim();
    let inner = template.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    inner.strip_prefix("filters.")?.split_whitespace().next()
}

fn validate_card_positions(
    card_id: &str,
    position: &serde_json::Map<String, JsonValue>,
    required_breakpoints: &BTreeSet<&str>,
    breakpoint_columns: &BTreeMap<String, i64>,
) -> std::result::Result<(), String> {
    for breakpoint in required_breakpoints {
        if !position.contains_key(*breakpoint) {
            return Err(format!(
                "Dashboard card '{}' missing position for breakpoint '{}'",
                card_id, breakpoint
            ));
        }
    }

    for (breakpoint, value) in position {
        let Some(columns) = breakpoint_columns.get(breakpoint) else {
            return Err(format!(
                "Dashboard card '{}' has position for unknown breakpoint '{}'",
                card_id, breakpoint
            ));
        };
        let layout = value.as_object().ok_or_else(|| {
            format!(
                "Dashboard card '{}' position for breakpoint '{}' must be an object",
                card_id, breakpoint
            )
        })?;
        let x = parse_layout_dimension(card_id, breakpoint, layout, "x", false)?;
        let y = parse_layout_dimension(card_id, breakpoint, layout, "y", false)?;
        let w = parse_layout_dimension(card_id, breakpoint, layout, "w", true)?;
        let h = parse_layout_dimension(card_id, breakpoint, layout, "h", true)?;

        if x >= *columns {
            return Err(format!(
                "Dashboard card '{}' position breakpoint '{}' x {} must be less than breakpoint columns {}",
                card_id, breakpoint, x, columns
            ));
        }
        if w > *columns {
            return Err(format!(
                "Dashboard card '{}' position breakpoint '{}' width {} exceeds breakpoint columns {}",
                card_id, breakpoint, w, columns
            ));
        }
        if x + w > *columns {
            return Err(format!(
                "Dashboard card '{}' position breakpoint '{}' x + w ({}) exceeds breakpoint columns {}",
                card_id,
                breakpoint,
                x + w,
                columns
            ));
        }
        let _ = y;
        let _ = h;
    }

    Ok(())
}

fn parse_layout_dimension(
    card_id: &str,
    breakpoint: &str,
    layout: &serde_json::Map<String, JsonValue>,
    key: &str,
    strictly_positive: bool,
) -> std::result::Result<i64, String> {
    let value = layout.get(key).and_then(JsonValue::as_i64).ok_or_else(|| {
        let expectation = if strictly_positive {
            "positive"
        } else {
            "non-negative"
        };
        format!(
            "Dashboard card '{}' position breakpoint '{}' requires {} integer '{}'",
            card_id, breakpoint, expectation, key
        )
    })?;

    if (!strictly_positive && value < 0) || (strictly_positive && value <= 0) {
        let expectation = if strictly_positive {
            "positive"
        } else {
            "non-negative"
        };
        return Err(format!(
            "Dashboard card '{}' position breakpoint '{}' requires {} integer '{}'",
            card_id, breakpoint, expectation, key
        ));
    }

    Ok(value)
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
            {
                "id": "events",
                "source": "event_count",
                "position": {
                    "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                }
            },
            {
                "id": "events",
                "source": "event_count",
                "position": {
                    "lg": { "x": 6, "y": 0, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": 4, "w": 4, "h": 4 }
                }
            }
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

    #[test]
    fn rejects_position_width_beyond_breakpoint_columns() {
        let mut spec = valid_spec();
        spec["cards"][0]["position"]["lg"]["w"] = json!(13);
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard card 'events' position breakpoint 'lg' width 13 exceeds breakpoint columns 12"
        );
    }

    #[test]
    fn rejects_position_outside_breakpoint_columns() {
        let mut spec = valid_spec();
        spec["cards"][0]["position"]["lg"] = json!({ "x": 10, "y": 0, "w": 3, "h": 4 });
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard card 'events' position breakpoint 'lg' x + w (13) exceeds breakpoint columns 12"
        );
    }

    #[test]
    fn rejects_negative_position_dimension() {
        let mut spec = valid_spec();
        spec["cards"][0]["position"]["lg"]["x"] = json!(-1);
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard card 'events' position breakpoint 'lg' requires non-negative integer 'x'"
        );
    }

    #[test]
    fn rejects_unknown_filter_reference_in_source_params() {
        let mut spec = valid_spec();
        spec["data_sources"] = json!({
            "event_count": {
                "type": "event_count",
                "params": {
                    "action_refs": "{{ filters.action_ref }}"
                }
            }
        });
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard source 'event_count' param 'action_refs' references unknown filter 'action_ref'"
        );
    }

    #[test]
    fn rejects_template_entries_inside_param_arrays() {
        let mut spec = valid_spec();
        spec["filters"] = json!([
            { "id": "action_ref", "type": "action_ref" }
        ]);
        spec["data_sources"] = json!({
            "event_count": {
                "type": "event_count",
                "params": {
                    "action_refs": ["core.echo", "{{ filters.action_ref }}"]
                }
            }
        });
        assert_eq!(
            validate_dashboard_spec(&spec).unwrap_err(),
            "Dashboard source 'event_count' param 'action_refs' template values are only supported as a single string"
        );
    }
}
