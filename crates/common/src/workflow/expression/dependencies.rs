//! Resolve template dependencies with the same parser and selectors as evaluation.

use std::collections::BTreeSet;

use super::{evaluator, parse_expression, EvalContext, EvalError, EvalResult, Expr};

pub struct ExpressionDependencies {
    pub direct_path: Option<String>,
    pub paths: BTreeSet<String>,
}

pub fn expression_dependencies(
    input: &str,
    context: &dyn EvalContext,
) -> EvalResult<ExpressionDependencies> {
    let expression =
        parse_expression(input).map_err(|error| EvalError::ParseError(error.to_string()))?;
    let direct_path = reference_path(&expression, context)?;
    let mut paths = BTreeSet::new();
    collect_paths(&expression, context, &mut paths)?;
    Ok(ExpressionDependencies { direct_path, paths })
}

pub fn keystore_references(input: &str, context: &dyn EvalContext) -> EvalResult<BTreeSet<String>> {
    let expression =
        parse_expression(input).map_err(|error| EvalError::ParseError(error.to_string()))?;
    let mut references = BTreeSet::new();
    collect_keystore_references(&expression, context, &mut references)?;
    Ok(references)
}

fn collect_keystore_references(
    expression: &Expr,
    context: &dyn EvalContext,
    references: &mut BTreeSet<String>,
) -> EvalResult<()> {
    match expression {
        Expr::IndexAccess { object, index } if matches!(object.as_ref(), Expr::Ident(name) if name == "keystore") =>
        {
            collect_keystore_references(index, context, references)?;
            let reference = evaluator::eval(index, context)?;
            let reference = reference
                .as_str()
                .filter(|reference| !reference.is_empty())
                .ok_or_else(|| {
                    EvalError::TypeError(
                        "Keystore selector must be a canonical key ref string".into(),
                    )
                })?;
            references.insert(reference.into());
        }
        Expr::Ident(name) if name == "keystore" => {
            return Err(EvalError::TypeError(
                "Use keystore[\"canonical.key_ref\"] for explicit key access".into(),
            ));
        }
        Expr::Array(values) | Expr::FunctionCall { args: values, .. } => {
            for value in values {
                collect_keystore_references(value, context, references)?;
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_keystore_references(left, context, references)?;
            collect_keystore_references(right, context, references)?;
        }
        Expr::UnaryOp { operand, .. } => collect_keystore_references(operand, context, references)?,
        Expr::DotAccess { object, .. } => collect_keystore_references(object, context, references)?,
        Expr::IndexAccess { object, index } => {
            collect_keystore_references(object, context, references)?;
            collect_keystore_references(index, context, references)?;
        }
        Expr::Literal(_) | Expr::Ident(_) => {}
    }
    Ok(())
}

fn reference_path(expression: &Expr, context: &dyn EvalContext) -> EvalResult<Option<String>> {
    match expression {
        Expr::Ident(name) => Ok(Some(name.clone())),
        Expr::FunctionCall { name, args } if name == "result" && args.is_empty() => {
            Ok(Some("result".into()))
        }
        Expr::DotAccess { object, field } => {
            Ok(reference_path(object, context)?.map(|path| format!("{path}.{field}")))
        }
        Expr::IndexAccess { object, index } => {
            let Some(path) = reference_path(object, context)? else {
                return Ok(None);
            };
            let index = evaluator::eval(index, context)?;
            let segment = match index {
                serde_json::Value::String(index) => index,
                serde_json::Value::Number(index) => index.to_string(),
                _ => return Err(EvalError::TypeError("Invalid template selector".into())),
            };
            Ok(Some(format!("{path}.{segment}")))
        }
        _ => Ok(None),
    }
}

fn collect_paths(
    expression: &Expr,
    context: &dyn EvalContext,
    paths: &mut BTreeSet<String>,
) -> EvalResult<()> {
    if let Some(path) = reference_path(expression, context)? {
        paths.insert(path);
        collect_selectors(expression, context, paths)?;
        return Ok(());
    }
    match expression {
        Expr::Array(values) | Expr::FunctionCall { args: values, .. } => {
            for value in values {
                collect_paths(value, context, paths)?;
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_paths(left, context, paths)?;
            collect_paths(right, context, paths)?;
        }
        Expr::UnaryOp { operand, .. } => collect_paths(operand, context, paths)?,
        Expr::DotAccess { object, .. } => collect_paths(object, context, paths)?,
        Expr::IndexAccess { object, index } => {
            collect_paths(object, context, paths)?;
            collect_paths(index, context, paths)?;
        }
        Expr::Literal(_) | Expr::Ident(_) => {}
    }
    Ok(())
}

fn collect_selectors(
    expression: &Expr,
    context: &dyn EvalContext,
    paths: &mut BTreeSet<String>,
) -> EvalResult<()> {
    match expression {
        Expr::IndexAccess { object, index } => {
            collect_paths(index, context, paths)?;
            collect_selectors(object, context, paths)?;
        }
        Expr::DotAccess { object, .. } => collect_selectors(object, context, paths)?,
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Context;

    impl EvalContext for Context {
        fn resolve_variable(&self, name: &str) -> EvalResult<serde_json::Value> {
            match name {
                "config" => Ok(json!({"key_ref": "system.signer", "index": 1})),
                _ => Err(EvalError::VariableNotFound(name.into())),
            }
        }

        fn call_workflow_function(
            &self,
            _: &str,
            _: &[serde_json::Value],
        ) -> EvalResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    #[test]
    fn transformations_and_resolved_selectors_keep_all_dependencies() {
        let dependencies = expression_dependencies(
            "upper(parameters.tokens[config.index]) + keystore[config.key_ref]",
            &Context,
        )
        .unwrap();
        assert!(dependencies.direct_path.is_none());
        assert_eq!(
            dependencies.paths,
            BTreeSet::from([
                "parameters.tokens.1".into(),
                "config.index".into(),
                "config.key_ref".into(),
                "keystore.system.signer".into(),
            ])
        );
        let direct = expression_dependencies("parameters.tokens[config.index]", &Context).unwrap();
        assert_eq!(direct.direct_path.as_deref(), Some("parameters.tokens.1"));
        let literal = expression_dependencies("'parameters.password'", &Context).unwrap();
        assert!(literal.paths.is_empty());
        assert_eq!(
            keystore_references("upper(keystore[config.key_ref].private_key_pem)", &Context)
                .unwrap(),
            BTreeSet::from(["system.signer".into()]),
        );
        assert!(keystore_references("keystore", &Context).is_err());
    }
}
