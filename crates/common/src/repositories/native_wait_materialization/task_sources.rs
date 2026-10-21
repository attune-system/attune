//! Bind the completed task results actually named by this Wait program.
use super::*;
use crate::workflow::expression::{parse_expression, Expr};
use std::collections::BTreeSet;

pub(super) async fn prepare_sources(
    conn: &mut PgConnection,
    parent: &Execution,
    task: &Value,
    workflow: &WorkflowExecution,
    key: &str,
) -> Result<Option<Sources>> {
    let graph = &parent
        .executable_snapshot
        .as_ref()
        .ok_or_else(denied)?
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    let mut names = BTreeSet::new();
    if !task_uses(task, &mut names)? {
        return Ok(None);
    }
    for transition in program_shape::transitions(task)? {
        if let Some(condition) = transition.get("when").and_then(Value::as_str) {
            if !condition.contains("{{") && !expression_uses(condition, &mut names)? {
                return Ok(None);
            }
        }
        for successor in program_shape::successors(transition)? {
            let successor =
                continuation::unique_task(graph, successor.as_str().ok_or_else(denied)?)?;
            if let Some(input) = successor.get("input").or_else(|| successor.get("params")) {
                if !task_uses(input, &mut names)? {
                    return Ok(None);
                }
            }
        }
    }
    super::super::native_transition_materialization::prepare_sources_for_tasks(
        conn, parent, workflow, &names, key,
    )
    .await
}

fn task_uses(value: &Value, names: &mut BTreeSet<String>) -> Result<bool> {
    let mut pending = vec![(value, 0)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 {
            return Err(denied());
        }
        match value {
            Value::String(value) if value.contains("{{") => {
                let mut cursor = value.as_str();
                while let Some(open) = cursor.find("{{") {
                    let rest = &cursor[open + 2..];
                    let close = rest.find("}}").ok_or_else(denied)?;
                    if !expression_uses(rest[..close].trim(), names)? {
                        return Ok(false);
                    }
                    cursor = &rest[close + 2..];
                }
            }
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    Ok(true)
}

fn expression_uses(expression: &str, names: &mut BTreeSet<String>) -> Result<bool> {
    let ast = parse_expression(expression).map_err(|_| denied())?;
    let mut pending = vec![(&ast, 0)];
    let mut nodes = 0;
    while let Some((node, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 {
            return Err(denied());
        }
        match node {
            Expr::DotAccess { object, field } if matches!(object.as_ref(), Expr::Ident(name) if name == "task" || name == "tasks") =>
            {
                names.insert(field.clone());
            }
            Expr::IndexAccess { object, index } if matches!(object.as_ref(), Expr::Ident(name) if name == "task" || name == "tasks") =>
            {
                let Expr::Literal(Value::String(name)) = index.as_ref() else {
                    return Ok(false);
                };
                names.insert(name.clone());
            }
            Expr::Ident(name) if name == "task" || name == "tasks" => return Ok(false),
            Expr::DotAccess { object, .. } => pending.push((object, depth + 1)),
            Expr::IndexAccess { object, index } => {
                pending.push((object, depth + 1));
                pending.push((index, depth + 1));
            }
            Expr::BinaryOp { left, right, .. } => {
                pending.push((left, depth + 1));
                pending.push((right, depth + 1));
            }
            Expr::UnaryOp { operand, .. } => pending.push((operand, depth + 1)),
            Expr::FunctionCall { args, .. } | Expr::Array(args) => {
                pending.extend(args.iter().map(|value| (value, depth + 1)));
            }
            Expr::Literal(_) | Expr::Ident(_) => {}
        }
    }
    Ok(names.len() <= 256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_sources_are_selected_without_binding_a_last_result() {
        let mut names = BTreeSet::new();
        assert!(task_uses(&json!({"execution":"{{ task.request.execution_id }}", "other":"{{ tasks['previous'].id }}"}), &mut names).unwrap());
        assert_eq!(names, BTreeSet::from(["previous".into(), "request".into()]));
        assert!(expression_uses("failed()", &mut names).unwrap());
    }

    #[test]
    fn dynamic_task_names_do_not_borrow_an_arbitrary_result() {
        assert!(!task_uses(
            &json!("{{ task[parameters.name].id }}"),
            &mut BTreeSet::new()
        )
        .unwrap());
        let mut names = BTreeSet::new();
        assert!(task_uses(&json!("literal task.unused"), &mut names).unwrap());
        assert!(names.is_empty());
    }
}
