//! JSON object keys remain strings when Serde buffers an internally tagged
//! enum. Decode the canonical numeric item keys explicitly without changing
//! the v1 attribution representation.
use std::collections::BTreeMap;

use serde::{de::Error, Deserialize, Deserializer};

pub(crate) fn deserialize_queue_items<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<crate::models::Id, Option<crate::runtime_authority::HumanAttribution>>, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded =
        BTreeMap::<String, Option<crate::runtime_authority::HumanAttribution>>::deserialize(
            deserializer,
        )?;
    encoded
        .into_iter()
        .map(|(key, actor)| {
            let id = key
                .parse::<crate::models::Id>()
                .map_err(|_| D::Error::custom("Invalid queue attribution item ID"))?;
            if id <= 0 || id.to_string() != key {
                return Err(D::Error::custom(
                    "Queue attribution requires canonical positive item IDs",
                ));
            }
            Ok((id, actor))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::runtime_authority::{HumanAttribution, RequestAttribution};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn mixed_queue_attribution_round_trips_through_tagged_json_and_jsonb_values() {
        let attribution = RequestAttribution::QueueBatch {
            queue_ref: "pack.inbox".into(),
            items: BTreeMap::from([
                (
                    1,
                    Some(HumanAttribution {
                        identity_id: 8,
                        login: "historical".into(),
                    }),
                ),
                (2, None),
                (
                    i64::MAX,
                    Some(HumanAttribution {
                        identity_id: 9,
                        login: "other".into(),
                    }),
                ),
            ]),
        };
        let value = serde_json::to_value(&attribution).unwrap();
        assert_eq!(value["items"]["1"]["identity_id"], 8);
        assert!(value["items"]["2"].is_null());
        assert_eq!(
            serde_json::from_value::<RequestAttribution>(value).unwrap(),
            attribution
        );
        assert_eq!(
            serde_json::from_str::<RequestAttribution>(
                &serde_json::to_string(&attribution).unwrap()
            )
            .unwrap(),
            attribution
        );
    }

    #[test]
    fn queue_attribution_rejects_noncanonical_nonpositive_and_overflowed_item_ids() {
        for key in [
            "0",
            "-1",
            "+1",
            "01",
            "1.0",
            "1e3",
            " 1",
            "9223372036854775808",
            "invalid",
        ] {
            let mut items = serde_json::Map::new();
            items.insert(key.into(), serde_json::Value::Null);
            let value = json!({"kind":"queue_batch", "queue_ref":"pack.inbox", "items":items});
            assert!(
                serde_json::from_value::<RequestAttribution>(value).is_err(),
                "accepted item key {key}"
            );
        }
    }
}
