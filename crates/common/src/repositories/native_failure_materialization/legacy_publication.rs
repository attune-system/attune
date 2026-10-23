//! Structural migration through the existing canonical parser. This descriptor
//! supplies no declaration, Condition receipt or output authority.
use super::*;

pub(in crate::repositories) struct CanonicalCompletionTask {
    next: Vec<Value>,
    legacy_publish: bool,
}
impl CanonicalCompletionTask {
    pub fn prepare(task: &Value) -> Result<Option<Self>> {
        let next = match task.get("next").filter(|value| !value.is_null()) {
            Some(value) => value.as_array().ok_or_else(changed)?.clone(),
            None => Vec::new(),
        };
        // Preserve the parser's explicit-next precedence. Do not emit ignored
        // legacy publications or invent a second traversal of their directives.
        if !next.is_empty() {
            return Ok(Some(Self {
                next,
                legacy_publish: false,
            }));
        }
        if ["on_success", "on_failure", "on_complete", "on_timeout"]
            .iter()
            .any(|key| task.get(*key).is_some_and(|value| !value.is_null()))
            || task.get("decision").is_some_and(|value| {
                !value.is_null() && value.as_array().is_none_or(|values| !values.is_empty())
            })
        {
            return Ok(None);
        }
        let publish = match task.get("publish").filter(|value| !value.is_null()) {
            Some(value) => value.as_array().ok_or_else(changed)?,
            None => {
                return Ok(Some(Self {
                    next,
                    legacy_publish: false,
                }))
            }
        };
        if publish.is_empty() {
            return Ok(Some(Self {
                next,
                legacy_publish: false,
            }));
        }
        let mut parsed: crate::workflow::parser::Task = serde_json::from_value(task.clone())?;
        parsed.normalize_transitions();
        let next = serde_json::to_value(&parsed.next)?
            .as_array()
            .ok_or_else(changed)?
            .clone();
        if next.len() != 1
            || next[0].get("when").and_then(Value::as_str) != Some("{{ succeeded() }}")
            || next[0].get("do").is_some_and(|value| !value.is_null())
        {
            return Err(changed());
        }
        Ok(Some(Self {
            next,
            legacy_publish: true,
        }))
    }
    pub fn next(&self) -> &[Value] {
        &self.next
    }
    pub fn legacy_publish(&self) -> bool {
        self.legacy_publish
    }
    pub fn publication_path(&self, task: &str, transition: usize, ordinal: usize) -> String {
        if self.legacy_publish {
            format!("{task}/publish/{ordinal}")
        } else {
            format!("{task}/next/{transition}/publish/{ordinal}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_publish_uses_parser_normalization_and_original_proof_path() {
        let task = serde_json::json!({"name":"leaf","action":"pack.leaf","publish":["whole"]});
        let canonical = CanonicalCompletionTask::prepare(&task).unwrap().unwrap();
        assert!(canonical.legacy_publish());
        assert_eq!(canonical.next()[0]["when"], "{{ succeeded() }}");
        assert_eq!(
            canonical.publication_path("/workflow/tasks/0", 0, 0),
            "/workflow/tasks/0/publish/0"
        );
        assert!(
            task.get("next").is_none(),
            "the immutable authored graph is not rewritten"
        );
    }
    #[test]
    fn explicit_next_wins_and_legacy_successors_are_not_reinterpreted() {
        let task = serde_json::json!({"name":"leaf","action":"pack.leaf","publish":["ignored"],"next":[{"publish":["real"]}]});
        let canonical = CanonicalCompletionTask::prepare(&task).unwrap().unwrap();
        assert!(!canonical.legacy_publish());
        assert_eq!(canonical.next()[0]["publish"][0], "real");
        let successor = serde_json::json!({"name":"leaf","action":"pack.leaf","publish":["whole"],"on_success":"later"});
        assert!(CanonicalCompletionTask::prepare(&successor)
            .unwrap()
            .is_none());
    }
}
