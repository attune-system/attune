use std::collections::BTreeMap;

use serde_json::Value;

use crate::models::{
    WorkQueueBatchMode, WorkQueueConfig, WorkQueueDispatchConfig, WorkQueueTunableSource,
    WorkQueueTunableValue,
};
use crate::Result;

/// Scheduling values materialized from a queue item's retained metadata and
/// sealed configuration. This exposes no Key values or runtime API grants.
pub struct QueueDispatchTuning {
    pub concurrency: u32,
    pub batch_size: u32,
}

pub(super) struct QueueTuningInputs(WorkQueueDispatchConfig);

impl QueueTuningInputs {
    pub(super) fn from_metadata(metadata: &Value) -> Result<Self> {
        let config: WorkQueueConfig =
            serde_json::from_value(metadata.get("config").cloned().ok_or_else(super::denied)?)?;
        let mode: WorkQueueBatchMode = serde_json::from_value(
            metadata
                .get("batch_mode")
                .cloned()
                .ok_or_else(super::denied)?,
        )?;
        let mut dispatch = config.dispatch.unwrap_or_default();
        if mode == WorkQueueBatchMode::Single {
            dispatch.batch_size = None;
        }
        Ok(Self(dispatch))
    }

    fn inputs(&self) -> impl Iterator<Item = &WorkQueueTunableValue> {
        self.0.concurrency.iter().chain(self.0.batch_size.iter())
    }

    pub(super) fn uses_pack_configuration(&self) -> bool {
        self.inputs()
            .any(|input| input.source == WorkQueueTunableSource::PackConfig)
    }

    pub(super) fn key_refs(&self) -> impl Iterator<Item = &str> {
        self.inputs()
            .filter(|input| input.source == WorkQueueTunableSource::Keystore)
            .filter_map(|input| input.key_ref.as_deref())
    }

    pub(super) fn resolve(
        &self,
        configuration: &Value,
        keys: &BTreeMap<String, Value>,
    ) -> QueueDispatchTuning {
        QueueDispatchTuning {
            concurrency: resolve(self.0.concurrency.as_ref(), configuration, keys),
            batch_size: resolve(self.0.batch_size.as_ref(), configuration, keys),
        }
    }
}

fn resolve(
    input: Option<&WorkQueueTunableValue>,
    configuration: &Value,
    keys: &BTreeMap<String, Value>,
) -> u32 {
    let Some(input) = input else { return 1 };
    let value = match input.source {
        WorkQueueTunableSource::Literal => input.value.as_ref(),
        WorkQueueTunableSource::PackConfig => input
            .path
            .as_deref()
            .and_then(|path| get_path(configuration, path)),
        WorkQueueTunableSource::Keystore => input
            .key_ref
            .as_deref()
            .and_then(|reference| keys.get(reference))
            .and_then(|value| match &input.path {
                Some(path) => get_path(value, path),
                None => Some(value),
            }),
    };
    value
        .and_then(positive_u32)
        .or_else(|| input.fallback.as_ref().and_then(positive_u32))
        .unwrap_or(1)
}

fn positive_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(value) => value.as_u64().and_then(|number| number.try_into().ok()),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
    .filter(|&value| value > 0)
}

fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|part| !part.is_empty())
        .try_fold(value, |parent, part| match parent {
            Value::Object(object) => object.get(part),
            Value::Array(values) => values.get(part.parse::<usize>().ok()?),
            _ => None,
        })
}
