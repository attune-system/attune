//! Classify whether a Wait program may need its genuine terminal-output owner.
//! This never evaluates a condition or supplies lifecycle/result authority.
use super::*;

pub(super) fn transitions(task: &Value) -> Result<&[Value]> {
    if !task.is_object() {
        return Err(denied());
    }
    match task.get("next") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(next)) if next.len() <= 128 => Ok(next),
        _ => Err(denied()),
    }
}

pub(super) fn successors(transition: &Value) -> Result<&[Value]> {
    if !transition.is_object() {
        return Err(denied());
    }
    match transition.get("do") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(tasks))
            if tasks.len() <= 128
                && tasks
                    .iter()
                    .all(|task| task.as_str().is_some_and(|name| !name.is_empty())) =>
        {
            Ok(tasks)
        }
        _ => Err(denied()),
    }
}

/// Only exact, nonempty terminal recovery branches prove complete coverage.
/// Every other shape needs single-pass preparation that can own either the
/// selected recovery children or a terminal-output recipe, without a rerender.
pub(super) fn may_finish_without_recovery(task: &Value) -> Result<bool> {
    let next = transitions(task)?;
    for outcome in ["failed()", "timed_out()", "cancelled()"] {
        let covered = next.iter().any(|transition| {
            let Some(condition) = transition.get("when").and_then(Value::as_str) else {
                return false;
            };
            let condition = condition.trim();
            let condition = condition
                .strip_prefix("{{")
                .and_then(|value| value.strip_suffix("}}"))
                .unwrap_or(condition)
                .trim();
            condition == outcome
                && transition
                    .get("do")
                    .and_then(Value::as_array)
                    .is_some_and(|tasks| !tasks.is_empty())
        });
        if !covered {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_null_and_empty_next_are_terminal_programs() {
        for task in [json!({}), json!({"next": null}), json!({"next": []})] {
            assert!(transitions(&task).unwrap().is_empty());
            assert!(may_finish_without_recovery(&task).unwrap());
        }
    }

    #[test]
    fn condition_only_transitions_have_no_successors_but_malformed_do_is_denied() {
        for transition in [
            json!({"when":"{{ failed() }}"}),
            json!({"do":null}),
            json!({"do":[]}),
        ] {
            assert!(successors(&transition).unwrap().is_empty());
        }
        assert_eq!(
            successors(&json!({"do":["recover"]})).unwrap(),
            &[json!("recover")]
        );
        for transition in [
            json!(null),
            json!({"do":false}),
            json!({"do":"recover"}),
            json!({"do":[null]}),
            json!({"do":[""]}),
            json!({"do":vec!["recover";129]}),
        ] {
            assert!(successors(&transition).is_err());
        }
    }

    #[test]
    fn malformed_and_oversized_programs_do_not_become_empty() {
        for task in [json!(null), json!(false), json!([]), json!("")] {
            assert!(transitions(&task).is_err());
        }
        for next in [
            json!(false),
            json!({}),
            json!(""),
            json!(vec![json!({}); 129]),
        ] {
            assert!(transitions(&json!({"next": next})).is_err());
        }
    }

    #[test]
    fn only_complete_literal_outcome_recovery_avoids_completion_preparation() {
        let complete = json!({"next": [
            {"when": "{{ failed() }}", "do": ["recover"]},
            {"when": "{{ timed_out() }}", "do": ["recover"]},
            {"when": "{{ cancelled() }}", "do": ["recover"]}
        ]});
        assert!(!may_finish_without_recovery(&complete).unwrap());
        let mut partial = complete.clone();
        partial["next"][1]["do"] = json!([]);
        assert!(may_finish_without_recovery(&partial).unwrap());
        let mut conditional = complete;
        conditional["next"][0]["when"] = json!("{{ failed() and workflow.recover }}");
        assert!(may_finish_without_recovery(&conditional).unwrap());
    }
}
