//! # Expression Evaluator
//!
//! Walks the AST and produces a `JsonValue` result.

use super::ast::{BinaryOp, Expr, UnaryOp};
use regex::Regex;
use serde_json::{json, Value as JsonValue};
use thiserror::Error;

/// Result type for evaluation operations.
pub type EvalResult<T> = Result<T, EvalError>;

/// Errors that can occur during expression evaluation.
#[derive(Debug, Error)]
pub enum EvalError {
    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    #[error("Type error: {0}")]
    TypeError(String),

    #[error("Division by zero")]
    DivisionByZero,

    #[error("Index out of bounds: {0}")]
    IndexOutOfBounds(String),

    #[error("Unknown function: {0}")]
    UnknownFunction(String),

    #[error("Wrong number of arguments for {0}: expected {1}, got {2}")]
    WrongArgCount(String, String, usize),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Regex error: {0}")]
    RegexError(String),
}

/// Trait for resolving variables and workflow-specific functions from
/// the execution context.
pub trait EvalContext: Sync {
    /// Resolve a top-level variable name to its JSON value.
    fn resolve_variable(&self, name: &str) -> EvalResult<JsonValue>;

    /// Try to call a workflow-specific function (e.g., `result()`, `succeeded()`).
    /// Return `Ok(Some(value))` if handled, `Ok(None)` if not recognized.
    fn call_workflow_function(
        &self,
        name: &str,
        args: &[JsonValue],
    ) -> EvalResult<Option<JsonValue>>;
    fn workflow_function_source(&self, name: &str, args: &[JsonValue]) -> Option<SelectedField> {
        function_source(name, args)
    }
}

fn function_source(name: &str, args: &[JsonValue]) -> Option<SelectedField> {
    (name == "result" && args.is_empty()).then(|| SelectedField {
        namespace: "result".into(),
        path: String::new(),
    })
}

/// Asynchronous source loading at the exact selected member, not a selector
/// prepass. Both sync and async entry points use the evaluator below.
pub trait AsyncEvalContext: Sync {
    fn workflow_function_source(&self, name: &str, args: &[JsonValue]) -> Option<SelectedField> {
        function_source(name, args)
    }
    fn resolve_variable<'a>(
        &'a self,
        name: &'a str,
    ) -> futures::future::BoxFuture<'a, EvalResult<JsonValue>>;
    fn call_workflow_function<'a>(
        &'a self,
        name: &'a str,
        args: &'a [JsonValue],
    ) -> futures::future::BoxFuture<'a, EvalResult<Option<JsonValue>>>;
    fn resolve_member<'a>(
        &'a self,
        _source: &'a SelectedField,
        _member: &'a str,
    ) -> futures::future::BoxFuture<'a, EvalResult<Option<JsonValue>>> {
        Box::pin(async { Ok(None) })
    }
}

struct SynchronousContext<'a>(&'a dyn EvalContext);
impl AsyncEvalContext for SynchronousContext<'_> {
    fn workflow_function_source(&self, name: &str, args: &[JsonValue]) -> Option<SelectedField> {
        self.0.workflow_function_source(name, args)
    }
    fn resolve_variable<'a>(
        &'a self,
        name: &'a str,
    ) -> futures::future::BoxFuture<'a, EvalResult<JsonValue>> {
        Box::pin(async move { self.0.resolve_variable(name) })
    }
    fn call_workflow_function<'a>(
        &'a self,
        name: &'a str,
        args: &'a [JsonValue],
    ) -> futures::future::BoxFuture<'a, EvalResult<Option<JsonValue>>> {
        Box::pin(async move { self.0.call_workflow_function(name, args) })
    }
}

/// An actual selected field. JSON pointers keep dotted/slashed property names
/// distinct, unlike the display paths used by legacy template sensitivity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SelectedField {
    pub namespace: String,
    pub path: String,
}

pub struct SelectedEvaluation {
    pub value: JsonValue,
    pub direct: Option<SelectedField>,
    pub fields: std::collections::BTreeSet<SelectedField>,
}

struct EvaluatedValue {
    value: JsonValue,
    field: Option<SelectedField>,
}

impl EvaluatedValue {
    fn plain(value: JsonValue) -> Self {
        Self { value, field: None }
    }
    fn consume(self, fields: &mut std::collections::BTreeSet<SelectedField>) -> JsonValue {
        if let Some(field) = self.field {
            fields.insert(field);
        }
        self.value
    }
    fn selected(value: JsonValue, namespace: &str) -> Self {
        Self {
            value,
            field: Some(SelectedField {
                namespace: namespace.into(),
                path: String::new(),
            }),
        }
    }
    fn member(self, value: JsonValue, member: &str) -> Self {
        let field = self.field.map(|mut field| {
            field.path.push('/');
            field
                .path
                .push_str(&member.replace('~', "~0").replace('/', "~1"));
            field
        });
        Self { value, field }
    }
}

/// Selection evidence comes from the same evaluator pass as the value. Dynamic
/// selectors run once and short-circuited branches supply no invented reads.
pub fn eval_selected(expr: &Expr, ctx: &dyn EvalContext) -> EvalResult<SelectedEvaluation> {
    use futures::FutureExt;
    // The synchronous adapter never suspends. This polls once and does not
    // block a runtime thread or create an executor for asynchronous I/O.
    eval_selected_async(expr, &SynchronousContext(ctx))
        .now_or_never()
        .expect("synchronous expression context suspended")
}

pub async fn eval_selected_async(
    expr: &Expr,
    ctx: &dyn AsyncEvalContext,
) -> EvalResult<SelectedEvaluation> {
    let mut fields = std::collections::BTreeSet::new();
    let result = eval_inner(expr, ctx, &mut fields, 0).await?;
    let direct = result.field.clone();
    let value = result.consume(&mut fields);
    if fields.len() > 4096 {
        return Err(EvalError::TypeError(
            "Expression selection exceeds its bound".into(),
        ));
    }
    Ok(SelectedEvaluation {
        value,
        direct,
        fields,
    })
}

/// Evaluate an AST expression against the given context.
pub fn eval(expr: &Expr, ctx: &dyn EvalContext) -> EvalResult<JsonValue> {
    Ok(eval_selected(expr, ctx)?.value)
}

fn eval_inner<'a>(
    expr: &'a Expr,
    ctx: &'a dyn AsyncEvalContext,
    fields: &'a mut std::collections::BTreeSet<SelectedField>,
    depth: usize,
) -> futures::future::BoxFuture<'a, EvalResult<EvaluatedValue>> {
    Box::pin(async move {
        if depth > 32 || fields.len() > 4096 {
            return Err(EvalError::TypeError(
                "Expression selection exceeds its bound".into(),
            ));
        }
        let depth = depth + 1;
        match expr {
            Expr::Literal(v) => Ok(EvaluatedValue::plain(v.clone())),

            Expr::Array(elements) => {
                let mut arr = Vec::with_capacity(elements.len());
                for elem in elements {
                    arr.push(eval_inner(elem, ctx, fields, depth).await?.consume(fields));
                }
                Ok(EvaluatedValue::plain(JsonValue::Array(arr)))
            }

            Expr::Ident(name) => Ok(EvaluatedValue::selected(
                ctx.resolve_variable(name).await?,
                name,
            )),

            Expr::BinaryOp { op, left, right } => {
                // Short-circuit for `and` / `or`
                if *op == BinaryOp::And {
                    let lv = eval_inner(left, ctx, fields, depth).await?.consume(fields);
                    if !is_truthy(&lv) {
                        return Ok(EvaluatedValue::plain(json!(false)));
                    }
                    let rv = eval_inner(right, ctx, fields, depth).await?.consume(fields);
                    return Ok(EvaluatedValue::plain(json!(is_truthy(&rv))));
                }
                if *op == BinaryOp::Or {
                    let lv = eval_inner(left, ctx, fields, depth).await?.consume(fields);
                    if is_truthy(&lv) {
                        return Ok(EvaluatedValue::plain(json!(true)));
                    }
                    let rv = eval_inner(right, ctx, fields, depth).await?.consume(fields);
                    return Ok(EvaluatedValue::plain(json!(is_truthy(&rv))));
                }

                let lv = eval_inner(left, ctx, fields, depth).await?.consume(fields);
                let rv = eval_inner(right, ctx, fields, depth).await?.consume(fields);
                Ok(EvaluatedValue::plain(eval_binary_op(*op, &lv, &rv)?))
            }

            Expr::UnaryOp { op, operand } => {
                let v = eval_inner(operand, ctx, fields, depth)
                    .await?
                    .consume(fields);
                Ok(EvaluatedValue::plain(eval_unary_op(*op, &v)?))
            }

            Expr::DotAccess { object, field } => {
                let obj = eval_inner(object, ctx, fields, depth).await?;
                let loaded = if let Some(source) = &obj.field {
                    ctx.resolve_member(source, field).await?
                } else {
                    None
                };
                let value = match loaded {
                    Some(value) => value,
                    None => dot_access(&obj.value, field)?,
                };
                Ok(obj.member(value, field))
            }

            Expr::IndexAccess { object, index } => {
                let obj = eval_inner(object, ctx, fields, depth).await?;
                let idx = eval_inner(index, ctx, fields, depth).await?.consume(fields);
                let loaded = if let (Some(source), Some(member)) = (&obj.field, idx.as_str()) {
                    ctx.resolve_member(source, member).await?
                } else {
                    None
                };
                let value = match loaded {
                    Some(value) => value,
                    None => index_access(&obj.value, &idx)?,
                };
                if obj.value.is_string() {
                    obj.consume(fields);
                    return Ok(EvaluatedValue::plain(value));
                }
                let member = match idx {
                    JsonValue::String(index) => index,
                    JsonValue::Number(index) => {
                        let index = index.as_i64().ok_or_else(|| {
                            EvalError::TypeError("Invalid selected field index".into())
                        })?;
                        if index < 0 {
                            (obj.value
                                .as_array()
                                .ok_or_else(|| {
                                    EvalError::TypeError("Invalid selected field container".into())
                                })?
                                .len() as i64
                                + index)
                                .to_string()
                        } else {
                            index.to_string()
                        }
                    }
                    _ => return Err(EvalError::TypeError("Invalid selected field index".into())),
                };
                Ok(obj.member(value, &member))
            }

            Expr::FunctionCall { name, args } => {
                // First, try workflow-specific functions (result(), succeeded(), etc.)
                // We evaluate args lazily for workflow fns that take 0 args.
                let mut evaluated_args = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated_args.push(eval_inner(arg, ctx, fields, depth).await?.consume(fields));
                }

                if let Some(val) = ctx.call_workflow_function(name, &evaluated_args).await? {
                    return Ok(EvaluatedValue {
                        value: val,
                        field: ctx.workflow_function_source(name, &evaluated_args),
                    });
                }

                // Built-in functions
                Ok(EvaluatedValue::plain(eval_builtin_function(
                    name,
                    &evaluated_args,
                )?))
            }
        }
    })
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use std::{
        collections::BTreeSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct LoadingContext {
        selectors: AtomicUsize,
        reads: AtomicUsize,
    }
    impl AsyncEvalContext for LoadingContext {
        fn resolve_variable<'a>(
            &'a self,
            name: &'a str,
        ) -> futures::future::BoxFuture<'a, EvalResult<JsonValue>> {
            Box::pin(async move {
                match name {
                    "keystore" => Ok(json!({})),
                    "parameters" => {
                        self.selectors.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"selector":"first"}))
                    }
                    _ => Err(EvalError::VariableNotFound(name.into())),
                }
            })
        }
        fn call_workflow_function<'a>(
            &'a self,
            _name: &'a str,
            _args: &'a [JsonValue],
        ) -> futures::future::BoxFuture<'a, EvalResult<Option<JsonValue>>> {
            Box::pin(async { Ok(None) })
        }
        fn resolve_member<'a>(
            &'a self,
            source: &'a SelectedField,
            member: &'a str,
        ) -> futures::future::BoxFuture<'a, EvalResult<Option<JsonValue>>> {
            Box::pin(async move {
                if source.namespace != "keystore" || !source.path.is_empty() {
                    return Ok(None);
                }
                tokio::task::yield_now().await;
                self.reads.fetch_add(1, Ordering::SeqCst);
                Ok(Some(match member {
                    "first" => json!({"next":"second"}),
                    "second" => json!("selected"),
                    _ => return Err(EvalError::VariableNotFound(member.into())),
                }))
            })
        }
    }

    #[tokio::test]
    async fn async_member_loading_keeps_nested_dynamic_selectors_in_one_canonical_pass() {
        let context = LoadingContext::default();
        let ast =
            super::super::parse_expression("keystore[keystore[parameters.selector].next]").unwrap();
        let result = eval_selected_async(&ast, &context).await.unwrap();
        assert_eq!(result.value, json!("selected"));
        assert_eq!(context.selectors.load(Ordering::SeqCst), 1);
        assert_eq!(context.reads.load(Ordering::SeqCst), 2);
        assert!(result.fields.contains(&SelectedField {
            namespace: "parameters".into(),
            path: "/selector".into()
        }));
        assert!(result.fields.contains(&SelectedField {
            namespace: "keystore".into(),
            path: "/first/next".into()
        }));
        assert!(result.fields.contains(&SelectedField {
            namespace: "keystore".into(),
            path: "/second".into()
        }));
        let short =
            super::super::parse_expression("true or keystore[parameters.selector]").unwrap();
        let result = eval_selected_async(&short, &context).await.unwrap();
        assert!(result.fields.is_empty());
        assert_eq!(context.selectors.load(Ordering::SeqCst), 1);
        assert_eq!(context.reads.load(Ordering::SeqCst), 2);
    }

    #[derive(Default)]
    struct Context {
        selectors: AtomicUsize,
    }
    impl EvalContext for Context {
        fn resolve_variable(&self, name: &str) -> EvalResult<JsonValue> {
            match name {
                "parameters" => Ok(json!({"a.b/~":"secret", "tokens":["one","two"]})),
                "workflow" => {
                    self.selectors.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"field":"a.b/~"}))
                }
                "config" => Ok(json!({"suffix":"!"})),
                _ => Err(EvalError::VariableNotFound(name.into())),
            }
        }
        fn call_workflow_function(
            &self,
            _: &str,
            _: &[JsonValue],
        ) -> EvalResult<Option<JsonValue>> {
            Ok(None)
        }
    }
    fn selected(input: &str, context: &Context) -> SelectedEvaluation {
        eval_selected(&super::super::parse_expression(input).unwrap(), context).unwrap()
    }

    #[test]
    fn actual_dynamic_selection_runs_once_and_preserves_pointer_segments() {
        let context = Context::default();
        let result = selected("parameters[workflow.field]", &context);
        assert_eq!(result.value, json!("secret"));
        assert_eq!(context.selectors.load(Ordering::SeqCst), 1);
        assert_eq!(
            result.direct,
            Some(SelectedField {
                namespace: "parameters".into(),
                path: "/a.b~1~0".into()
            })
        );
        assert_eq!(
            result.fields,
            BTreeSet::from([
                SelectedField {
                    namespace: "parameters".into(),
                    path: "/a.b~1~0".into()
                },
                SelectedField {
                    namespace: "workflow".into(),
                    path: "/field".into()
                },
            ])
        );
    }

    #[test]
    fn mixed_and_negative_index_sources_are_exact_and_unused_branches_are_absent() {
        let context = Context::default();
        let result = selected("parameters.tokens[-1] + config.suffix", &context);
        assert_eq!(result.value, json!("two!"));
        assert!(result.direct.is_none());
        assert_eq!(
            result.fields,
            BTreeSet::from([
                SelectedField {
                    namespace: "parameters".into(),
                    path: "/tokens/1".into()
                },
                SelectedField {
                    namespace: "config".into(),
                    path: "/suffix".into()
                },
            ])
        );
        let short = selected("false and missing.token", &context);
        assert_eq!(short.value, json!(false));
        assert!(short.fields.is_empty());
    }
}

// ---------------------------------------------------------------
// Truthiness
// ---------------------------------------------------------------

/// Determine if a JSON value is "truthy" (Python-like semantics).
pub fn is_truthy(v: &JsonValue) -> bool {
    match v {
        JsonValue::Null => false,
        JsonValue::Bool(b) => *b,
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        JsonValue::String(s) => !s.is_empty(),
        JsonValue::Array(a) => !a.is_empty(),
        JsonValue::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------
// Binary operations
// ---------------------------------------------------------------

fn eval_binary_op(op: BinaryOp, left: &JsonValue, right: &JsonValue) -> EvalResult<JsonValue> {
    match op {
        // Arithmetic
        BinaryOp::Add => eval_add(left, right),
        BinaryOp::Sub => eval_arithmetic(left, right, |a, b| a - b, |a, b| a - b, "-"),
        BinaryOp::Mul => eval_arithmetic(left, right, |a, b| a * b, |a, b| a * b, "*"),
        BinaryOp::Div => eval_div(left, right),
        BinaryOp::Mod => eval_mod(left, right),

        // Comparison
        BinaryOp::Eq => Ok(json!(json_eq(left, right))),
        BinaryOp::Ne => Ok(json!(!json_eq(left, right))),
        BinaryOp::Lt => eval_ordering(left, right, |o| o == std::cmp::Ordering::Less),
        BinaryOp::Gt => eval_ordering(left, right, |o| o == std::cmp::Ordering::Greater),
        BinaryOp::Le => eval_ordering(left, right, |o| o != std::cmp::Ordering::Greater),
        BinaryOp::Ge => eval_ordering(left, right, |o| o != std::cmp::Ordering::Less),

        // Membership
        BinaryOp::In => eval_in(left, right),

        // And/Or handled in eval() with short-circuit
        BinaryOp::And | BinaryOp::Or => unreachable!(),
    }
}

fn eval_add(left: &JsonValue, right: &JsonValue) -> EvalResult<JsonValue> {
    // String concatenation
    if left.is_string() && right.is_string() {
        let l = left.as_str().unwrap();
        let r = right.as_str().unwrap();
        return Ok(json!(format!("{}{}", l, r)));
    }

    // Array concatenation
    if left.is_array() && right.is_array() {
        let mut result = left.as_array().unwrap().clone();
        result.extend(right.as_array().unwrap().iter().cloned());
        return Ok(JsonValue::Array(result));
    }

    // Numeric addition
    eval_arithmetic(left, right, |a, b| a + b, |a, b| a + b, "+")
}

fn eval_arithmetic(
    left: &JsonValue,
    right: &JsonValue,
    int_op: impl Fn(i64, i64) -> i64,
    float_op: impl Fn(f64, f64) -> f64,
    op_name: &str,
) -> EvalResult<JsonValue> {
    match (as_numeric(left), as_numeric(right)) {
        (Some(NumericValue::Int(a)), Some(NumericValue::Int(b))) => Ok(json!(int_op(a, b))),
        (Some(a), Some(b)) => Ok(json!(float_op(a.as_f64(), b.as_f64()))),
        _ => Err(EvalError::TypeError(format!(
            "Cannot apply '{}' to {} and {}",
            op_name,
            type_name(left),
            type_name(right)
        ))),
    }
}

fn eval_div(left: &JsonValue, right: &JsonValue) -> EvalResult<JsonValue> {
    match (as_numeric(left), as_numeric(right)) {
        (Some(_), Some(b)) if b.as_f64() == 0.0 => Err(EvalError::DivisionByZero),
        (Some(NumericValue::Int(a)), Some(NumericValue::Int(b))) => {
            // Integer division stays integer if divisible
            if a % b == 0 {
                Ok(json!(a / b))
            } else {
                Ok(json!(a as f64 / b as f64))
            }
        }
        (Some(a), Some(b)) => Ok(json!(a.as_f64() / b.as_f64())),
        _ => Err(EvalError::TypeError(format!(
            "Cannot apply '/' to {} and {}",
            type_name(left),
            type_name(right)
        ))),
    }
}

fn eval_mod(left: &JsonValue, right: &JsonValue) -> EvalResult<JsonValue> {
    match (as_numeric(left), as_numeric(right)) {
        (Some(_), Some(b)) if b.as_f64() == 0.0 => Err(EvalError::DivisionByZero),
        (Some(NumericValue::Int(a)), Some(NumericValue::Int(b))) => Ok(json!(a % b)),
        (Some(a), Some(b)) => Ok(json!(a.as_f64() % b.as_f64())),
        _ => Err(EvalError::TypeError(format!(
            "Cannot apply '%' to {} and {}",
            type_name(left),
            type_name(right)
        ))),
    }
}

// ---------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------

/// Deep equality that allows int/float cross-comparison.
fn json_eq(a: &JsonValue, b: &JsonValue) -> bool {
    match (a, b) {
        (JsonValue::Null, JsonValue::Null) => true,
        (JsonValue::Bool(a), JsonValue::Bool(b)) => a == b,
        (JsonValue::Number(_), JsonValue::Number(_)) => {
            // Allow int/float comparison
            match (as_numeric(a), as_numeric(b)) {
                (Some(a), Some(b)) => a.as_f64() == b.as_f64(),
                _ => false,
            }
        }
        (JsonValue::String(a), JsonValue::String(b)) => a == b,
        (JsonValue::Array(a), JsonValue::Array(b)) => {
            if a.len() != b.len() {
                return false;
            }
            a.iter().zip(b.iter()).all(|(x, y)| json_eq(x, y))
        }
        (JsonValue::Object(a), JsonValue::Object(b)) => {
            if a.len() != b.len() {
                return false;
            }
            a.iter()
                .all(|(k, v)| b.get(k).is_some_and(|bv| json_eq(v, bv)))
        }
        // Different types (other than number cross-compare) are never equal
        _ => false,
    }
}

fn eval_ordering(
    left: &JsonValue,
    right: &JsonValue,
    predicate: impl Fn(std::cmp::Ordering) -> bool,
) -> EvalResult<JsonValue> {
    // Number comparison (int/float cross-allowed)
    if let (Some(a), Some(b)) = (as_numeric(left), as_numeric(right)) {
        let af = a.as_f64();
        let bf = b.as_f64();
        let ord = af.partial_cmp(&bf).unwrap_or(std::cmp::Ordering::Equal);
        return Ok(json!(predicate(ord)));
    }

    // String comparison
    if let (Some(a), Some(b)) = (left.as_str(), right.as_str()) {
        return Ok(json!(predicate(a.cmp(b))));
    }

    // List comparison (lexicographic)
    if let (Some(a), Some(b)) = (left.as_array(), right.as_array()) {
        let ord = compare_arrays(a, b)?;
        return Ok(json!(predicate(ord)));
    }

    Err(EvalError::TypeError(format!(
        "Cannot compare {} and {} with ordering operators",
        type_name(left),
        type_name(right)
    )))
}

fn compare_arrays(a: &[JsonValue], b: &[JsonValue]) -> EvalResult<std::cmp::Ordering> {
    for (x, y) in a.iter().zip(b.iter()) {
        if let (Some(xn), Some(yn)) = (as_numeric(x), as_numeric(y)) {
            let ord = xn
                .as_f64()
                .partial_cmp(&yn.as_f64())
                .unwrap_or(std::cmp::Ordering::Equal);
            if ord != std::cmp::Ordering::Equal {
                return Ok(ord);
            }
        } else if let (Some(xs), Some(ys)) = (x.as_str(), y.as_str()) {
            let ord = xs.cmp(ys);
            if ord != std::cmp::Ordering::Equal {
                return Ok(ord);
            }
        } else {
            return Err(EvalError::TypeError(
                "Cannot compare heterogeneous array elements for ordering".to_string(),
            ));
        }
    }
    Ok(a.len().cmp(&b.len()))
}

fn eval_in(needle: &JsonValue, haystack: &JsonValue) -> EvalResult<JsonValue> {
    match haystack {
        JsonValue::Array(arr) => Ok(json!(arr.iter().any(|item| json_eq(needle, item)))),
        JsonValue::Object(obj) => {
            if let Some(key) = needle.as_str() {
                Ok(json!(obj.contains_key(key)))
            } else {
                Err(EvalError::TypeError(
                    "Only string keys can be tested for membership in objects".to_string(),
                ))
            }
        }
        JsonValue::String(s) => {
            if let Some(sub) = needle.as_str() {
                Ok(json!(s.contains(sub)))
            } else {
                Err(EvalError::TypeError(
                    "Only strings can be tested for substring membership".to_string(),
                ))
            }
        }
        _ => Err(EvalError::TypeError(format!(
            "'in' requires array, object, or string on right side, got {}",
            type_name(haystack)
        ))),
    }
}

// ---------------------------------------------------------------
// Unary operations
// ---------------------------------------------------------------

fn eval_unary_op(op: UnaryOp, val: &JsonValue) -> EvalResult<JsonValue> {
    match op {
        UnaryOp::Neg => {
            if let Some(n) = as_numeric(val) {
                match n {
                    NumericValue::Int(i) => Ok(json!(-i)),
                    NumericValue::Float(f) => Ok(json!(-f)),
                }
            } else {
                Err(EvalError::TypeError(format!(
                    "Cannot negate {}",
                    type_name(val)
                )))
            }
        }
        UnaryOp::Not => Ok(json!(!is_truthy(val))),
    }
}

// ---------------------------------------------------------------
// Property / index access
// ---------------------------------------------------------------

fn dot_access(obj: &JsonValue, field: &str) -> EvalResult<JsonValue> {
    match obj {
        JsonValue::Object(map) => map
            .get(field)
            .cloned()
            .ok_or_else(|| EvalError::VariableNotFound(format!("field '{}'", field))),
        _ => Err(EvalError::TypeError(format!(
            "Cannot access property '{}' on {}",
            field,
            type_name(obj)
        ))),
    }
}

fn index_access(obj: &JsonValue, index: &JsonValue) -> EvalResult<JsonValue> {
    match obj {
        JsonValue::Array(arr) => {
            if let Some(i) = index.as_i64() {
                let i = if i < 0 {
                    // Negative indexing
                    (arr.len() as i64 + i) as usize
                } else {
                    i as usize
                };
                arr.get(i)
                    .cloned()
                    .ok_or_else(|| EvalError::IndexOutOfBounds(format!("{}", i)))
            } else {
                Err(EvalError::TypeError(
                    "Array index must be an integer".to_string(),
                ))
            }
        }
        JsonValue::Object(map) => {
            if let Some(key) = index.as_str() {
                map.get(key)
                    .cloned()
                    .ok_or_else(|| EvalError::VariableNotFound(format!("key '{}'", key)))
            } else {
                Err(EvalError::TypeError(
                    "Object key must be a string".to_string(),
                ))
            }
        }
        JsonValue::String(s) => {
            if let Some(i) = index.as_i64() {
                let chars: Vec<char> = s.chars().collect();
                let i = if i < 0 {
                    (chars.len() as i64 + i) as usize
                } else {
                    i as usize
                };
                chars
                    .get(i)
                    .map(|c| json!(c.to_string()))
                    .ok_or_else(|| EvalError::IndexOutOfBounds(format!("{}", i)))
            } else {
                Err(EvalError::TypeError(
                    "String index must be an integer".to_string(),
                ))
            }
        }
        _ => Err(EvalError::TypeError(format!(
            "Cannot index into {}",
            type_name(obj)
        ))),
    }
}

// ---------------------------------------------------------------
// Built-in functions
// ---------------------------------------------------------------

fn eval_builtin_function(name: &str, args: &[JsonValue]) -> EvalResult<JsonValue> {
    match name {
        // -- Type conversion --
        "string" => {
            expect_args(name, args, 1)?;
            Ok(json!(value_to_string(&args[0])))
        }
        "number" => {
            expect_args(name, args, 1)?;
            to_number(&args[0])
        }
        "int" => {
            expect_args(name, args, 1)?;
            to_int(&args[0])
        }
        "bool" => {
            expect_args(name, args, 1)?;
            Ok(json!(is_truthy(&args[0])))
        }

        // -- Introspection --
        "type_of" => {
            expect_args(name, args, 1)?;
            Ok(json!(type_name(&args[0])))
        }
        "length" => {
            expect_args(name, args, 1)?;
            fn_length(&args[0])
        }
        "keys" => {
            expect_args(name, args, 1)?;
            fn_keys(&args[0])
        }
        "values" => {
            expect_args(name, args, 1)?;
            fn_values(&args[0])
        }

        // -- Math --
        "abs" => {
            expect_args(name, args, 1)?;
            fn_abs(&args[0])
        }
        "floor" => {
            expect_args(name, args, 1)?;
            fn_floor(&args[0])
        }
        "ceil" => {
            expect_args(name, args, 1)?;
            fn_ceil(&args[0])
        }
        "round" => {
            expect_args(name, args, 1)?;
            fn_round(&args[0])
        }
        "min" => {
            expect_args(name, args, 2)?;
            fn_min(&args[0], &args[1])
        }
        "max" => {
            expect_args(name, args, 2)?;
            fn_max(&args[0], &args[1])
        }
        "sum" => {
            expect_args(name, args, 1)?;
            fn_sum(&args[0])
        }

        // -- String --
        "lower" => {
            expect_args(name, args, 1)?;
            fn_lower(&args[0])
        }
        "upper" => {
            expect_args(name, args, 1)?;
            fn_upper(&args[0])
        }
        "trim" => {
            expect_args(name, args, 1)?;
            fn_trim(&args[0])
        }
        "split" => {
            expect_args(name, args, 2)?;
            fn_split(&args[0], &args[1])
        }
        "join" => {
            expect_args(name, args, 2)?;
            fn_join(&args[0], &args[1])
        }
        "replace" => {
            expect_args(name, args, 3)?;
            fn_replace(&args[0], &args[1], &args[2])
        }
        "starts_with" => {
            expect_args(name, args, 2)?;
            fn_starts_with(&args[0], &args[1])
        }
        "ends_with" => {
            expect_args(name, args, 2)?;
            fn_ends_with(&args[0], &args[1])
        }
        "match" => {
            expect_args(name, args, 2)?;
            fn_match(&args[0], &args[1])
        }

        // -- Collections --
        "contains" => {
            expect_args(name, args, 2)?;
            eval_in(&args[1], &args[0])
        }
        "reversed" => {
            expect_args(name, args, 1)?;
            fn_reversed(&args[0])
        }
        "sort" => {
            expect_args(name, args, 1)?;
            fn_sort(&args[0])
        }
        "unique" => {
            expect_args(name, args, 1)?;
            fn_unique(&args[0])
        }
        "flat" => {
            expect_args(name, args, 1)?;
            fn_flat(&args[0])
        }
        "zip" => {
            expect_args(name, args, 2)?;
            fn_zip(&args[0], &args[1])
        }
        "range" => {
            if args.len() == 1 {
                fn_range_1(&args[0])
            } else if args.len() == 2 {
                fn_range_2(&args[0], &args[1])
            } else {
                Err(EvalError::WrongArgCount(
                    name.to_string(),
                    "1 or 2".to_string(),
                    args.len(),
                ))
            }
        }
        "slice" => {
            if args.len() == 2 {
                fn_slice(&args[0], &args[1], &JsonValue::Null)
            } else if args.len() == 3 {
                fn_slice(&args[0], &args[1], &args[2])
            } else {
                Err(EvalError::WrongArgCount(
                    name.to_string(),
                    "2 or 3".to_string(),
                    args.len(),
                ))
            }
        }
        "index_of" => {
            expect_args(name, args, 2)?;
            fn_index_of(&args[0], &args[1])
        }
        "count" => {
            expect_args(name, args, 2)?;
            fn_count(&args[0], &args[1])
        }
        "merge" => {
            expect_args(name, args, 2)?;
            fn_merge(&args[0], &args[1])
        }
        "chunks" => {
            expect_args(name, args, 2)?;
            fn_chunks(&args[0], &args[1])
        }

        _ => Err(EvalError::UnknownFunction(name.to_string())),
    }
}

fn expect_args(name: &str, args: &[JsonValue], expected: usize) -> EvalResult<()> {
    if args.len() != expected {
        Err(EvalError::WrongArgCount(
            name.to_string(),
            expected.to_string(),
            args.len(),
        ))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum NumericValue {
    Int(i64),
    Float(f64),
}

impl NumericValue {
    fn as_f64(self) -> f64 {
        match self {
            NumericValue::Int(i) => i as f64,
            NumericValue::Float(f) => f,
        }
    }
}

fn as_numeric(v: &JsonValue) -> Option<NumericValue> {
    if let Some(i) = v.as_i64() {
        Some(NumericValue::Int(i))
    } else {
        v.as_f64().map(NumericValue::Float)
    }
}

fn type_name(v: &JsonValue) -> &'static str {
    match v {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn value_to_string(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => s.clone(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------
// Type conversion functions
// ---------------------------------------------------------------

fn to_number(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Number(_) => Ok(v.clone()),
        JsonValue::String(s) => {
            if let Ok(f) = s.parse::<f64>() {
                Ok(json!(f))
            } else {
                Err(EvalError::TypeError(format!(
                    "Cannot convert string '{}' to number",
                    s
                )))
            }
        }
        JsonValue::Bool(b) => Ok(json!(if *b { 1.0 } else { 0.0 })),
        _ => Err(EvalError::TypeError(format!(
            "Cannot convert {} to number",
            type_name(v)
        ))),
    }
}

fn to_int(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(json!(i))
            } else if let Some(f) = n.as_f64() {
                Ok(json!(f as i64))
            } else {
                Err(EvalError::TypeError(
                    "Cannot convert number to int".to_string(),
                ))
            }
        }
        JsonValue::String(s) => {
            // Try integer first, then float truncation
            if let Ok(i) = s.parse::<i64>() {
                Ok(json!(i))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(json!(f as i64))
            } else {
                Err(EvalError::TypeError(format!(
                    "Cannot convert string '{}' to int",
                    s
                )))
            }
        }
        JsonValue::Bool(b) => Ok(json!(if *b { 1 } else { 0 })),
        _ => Err(EvalError::TypeError(format!(
            "Cannot convert {} to int",
            type_name(v)
        ))),
    }
}

// ---------------------------------------------------------------
// Introspection functions
// ---------------------------------------------------------------

fn fn_length(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::String(s) => Ok(json!(s.len())),
        JsonValue::Array(a) => Ok(json!(a.len())),
        JsonValue::Object(o) => Ok(json!(o.len())),
        _ => Err(EvalError::TypeError(format!(
            "length() requires string, array, or object, got {}",
            type_name(v)
        ))),
    }
}

fn fn_keys(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Object(obj) => {
            let keys: Vec<JsonValue> = obj.keys().map(|k| json!(k)).collect();
            Ok(JsonValue::Array(keys))
        }
        _ => Err(EvalError::TypeError(format!(
            "keys() requires object, got {}",
            type_name(v)
        ))),
    }
}

fn fn_values(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Object(obj) => {
            let values: Vec<JsonValue> = obj.values().cloned().collect();
            Ok(JsonValue::Array(values))
        }
        _ => Err(EvalError::TypeError(format!(
            "values() requires object, got {}",
            type_name(v)
        ))),
    }
}

// ---------------------------------------------------------------
// Math functions
// ---------------------------------------------------------------

fn fn_abs(v: &JsonValue) -> EvalResult<JsonValue> {
    match as_numeric(v) {
        Some(NumericValue::Int(i)) => Ok(json!(i.abs())),
        Some(NumericValue::Float(f)) => Ok(json!(f.abs())),
        None => Err(EvalError::TypeError(format!(
            "abs() requires number, got {}",
            type_name(v)
        ))),
    }
}

fn fn_floor(v: &JsonValue) -> EvalResult<JsonValue> {
    match as_numeric(v) {
        Some(NumericValue::Int(i)) => Ok(json!(i)),
        Some(NumericValue::Float(f)) => Ok(json!(f.floor() as i64)),
        None => Err(EvalError::TypeError(format!(
            "floor() requires number, got {}",
            type_name(v)
        ))),
    }
}

fn fn_ceil(v: &JsonValue) -> EvalResult<JsonValue> {
    match as_numeric(v) {
        Some(NumericValue::Int(i)) => Ok(json!(i)),
        Some(NumericValue::Float(f)) => Ok(json!(f.ceil() as i64)),
        None => Err(EvalError::TypeError(format!(
            "ceil() requires number, got {}",
            type_name(v)
        ))),
    }
}

fn fn_round(v: &JsonValue) -> EvalResult<JsonValue> {
    match as_numeric(v) {
        Some(NumericValue::Int(i)) => Ok(json!(i)),
        Some(NumericValue::Float(f)) => Ok(json!(f.round() as i64)),
        None => Err(EvalError::TypeError(format!(
            "round() requires number, got {}",
            type_name(v)
        ))),
    }
}

fn fn_min(a: &JsonValue, b: &JsonValue) -> EvalResult<JsonValue> {
    match (as_numeric(a), as_numeric(b)) {
        (Some(NumericValue::Int(x)), Some(NumericValue::Int(y))) => Ok(json!(x.min(y))),
        (Some(x), Some(y)) => Ok(json!(x.as_f64().min(y.as_f64()))),
        _ => {
            // String min
            if let (Some(sa), Some(sb)) = (a.as_str(), b.as_str()) {
                Ok(json!(sa.min(sb)))
            } else {
                Err(EvalError::TypeError(
                    "min() requires two numbers or two strings".to_string(),
                ))
            }
        }
    }
}

fn fn_max(a: &JsonValue, b: &JsonValue) -> EvalResult<JsonValue> {
    match (as_numeric(a), as_numeric(b)) {
        (Some(NumericValue::Int(x)), Some(NumericValue::Int(y))) => Ok(json!(x.max(y))),
        (Some(x), Some(y)) => Ok(json!(x.as_f64().max(y.as_f64()))),
        _ => {
            if let (Some(sa), Some(sb)) = (a.as_str(), b.as_str()) {
                Ok(json!(sa.max(sb)))
            } else {
                Err(EvalError::TypeError(
                    "max() requires two numbers or two strings".to_string(),
                ))
            }
        }
    }
}

fn fn_sum(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Array(arr) => {
            let mut has_float = false;
            let mut int_sum: i64 = 0;
            let mut float_sum: f64 = 0.0;

            for item in arr {
                match as_numeric(item) {
                    Some(NumericValue::Int(i)) => {
                        int_sum += i;
                        float_sum += i as f64;
                    }
                    Some(NumericValue::Float(f)) => {
                        has_float = true;
                        float_sum += f;
                    }
                    None => {
                        return Err(EvalError::TypeError(format!(
                            "sum() requires array of numbers, found {}",
                            type_name(item)
                        )));
                    }
                }
            }

            if has_float {
                Ok(json!(float_sum))
            } else {
                Ok(json!(int_sum))
            }
        }
        _ => Err(EvalError::TypeError(format!(
            "sum() requires array, got {}",
            type_name(v)
        ))),
    }
}

// ---------------------------------------------------------------
// String functions
// ---------------------------------------------------------------

fn fn_lower(v: &JsonValue) -> EvalResult<JsonValue> {
    require_string("lower", v).map(|s| json!(s.to_lowercase()))
}

fn fn_upper(v: &JsonValue) -> EvalResult<JsonValue> {
    require_string("upper", v).map(|s| json!(s.to_uppercase()))
}

fn fn_trim(v: &JsonValue) -> EvalResult<JsonValue> {
    require_string("trim", v).map(|s| json!(s.trim()))
}

fn fn_split(s: &JsonValue, sep: &JsonValue) -> EvalResult<JsonValue> {
    let s = require_string("split", s)?;
    let sep = require_string("split", sep)?;
    let parts: Vec<JsonValue> = s.split(sep).map(|p| json!(p)).collect();
    Ok(JsonValue::Array(parts))
}

fn fn_join(arr: &JsonValue, sep: &JsonValue) -> EvalResult<JsonValue> {
    let arr = arr.as_array().ok_or_else(|| {
        EvalError::TypeError(format!(
            "join() first argument must be array, got {}",
            type_name(arr)
        ))
    })?;
    let sep = require_string("join", sep)?;
    let strings: Result<Vec<String>, _> = arr.iter().map(|v| Ok(value_to_string(v))).collect();
    Ok(json!(strings?.join(sep)))
}

fn fn_replace(s: &JsonValue, old: &JsonValue, new: &JsonValue) -> EvalResult<JsonValue> {
    let s = require_string("replace", s)?;
    let old = require_string("replace", old)?;
    let new_s = require_string("replace", new)?;
    Ok(json!(s.replace(old, new_s)))
}

fn fn_starts_with(s: &JsonValue, prefix: &JsonValue) -> EvalResult<JsonValue> {
    let s = require_string("starts_with", s)?;
    let prefix = require_string("starts_with", prefix)?;
    Ok(json!(s.starts_with(prefix)))
}

fn fn_ends_with(s: &JsonValue, suffix: &JsonValue) -> EvalResult<JsonValue> {
    let s = require_string("ends_with", s)?;
    let suffix = require_string("ends_with", suffix)?;
    Ok(json!(s.ends_with(suffix)))
}

fn fn_match(pattern: &JsonValue, s: &JsonValue) -> EvalResult<JsonValue> {
    let pattern = require_string("match", pattern)?;
    let s = require_string("match", s)?;
    let re = Regex::new(pattern).map_err(|e| EvalError::RegexError(format!("{}", e)))?;
    Ok(json!(re.is_match(s)))
}

fn require_string<'a>(func: &str, v: &'a JsonValue) -> EvalResult<&'a str> {
    v.as_str().ok_or_else(|| {
        EvalError::TypeError(format!(
            "{}() requires string argument, got {}",
            func,
            type_name(v)
        ))
    })
}

// ---------------------------------------------------------------
// Collection functions
// ---------------------------------------------------------------

fn fn_reversed(v: &JsonValue) -> EvalResult<JsonValue> {
    match v {
        JsonValue::Array(arr) => {
            let mut rev = arr.clone();
            rev.reverse();
            Ok(JsonValue::Array(rev))
        }
        JsonValue::String(s) => Ok(json!(s.chars().rev().collect::<String>())),
        _ => Err(EvalError::TypeError(format!(
            "reversed() requires array or string, got {}",
            type_name(v)
        ))),
    }
}

fn fn_sort(v: &JsonValue) -> EvalResult<JsonValue> {
    let arr = v.as_array().ok_or_else(|| {
        EvalError::TypeError(format!("sort() requires array, got {}", type_name(v)))
    })?;

    let mut sorted = arr.clone();
    // Sort stably; numbers first, then strings
    let mut err: Option<EvalError> = None;
    sorted.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match (as_numeric(a), as_numeric(b)) {
            (Some(x), Some(y)) => x
                .as_f64()
                .partial_cmp(&y.as_f64())
                .unwrap_or(std::cmp::Ordering::Equal),
            _ => {
                if let (Some(sa), Some(sb)) = (a.as_str(), b.as_str()) {
                    sa.cmp(sb)
                } else {
                    err = Some(EvalError::TypeError(
                        "sort() requires array of numbers or strings".to_string(),
                    ));
                    std::cmp::Ordering::Equal
                }
            }
        }
    });

    if let Some(e) = err {
        return Err(e);
    }

    Ok(JsonValue::Array(sorted))
}

fn fn_unique(v: &JsonValue) -> EvalResult<JsonValue> {
    let arr = v.as_array().ok_or_else(|| {
        EvalError::TypeError(format!("unique() requires array, got {}", type_name(v)))
    })?;

    let mut seen = Vec::new();
    let mut result = Vec::new();
    for item in arr {
        if !seen.iter().any(|s| json_eq(s, item)) {
            seen.push(item.clone());
            result.push(item.clone());
        }
    }

    Ok(JsonValue::Array(result))
}

fn fn_flat(v: &JsonValue) -> EvalResult<JsonValue> {
    let arr = v.as_array().ok_or_else(|| {
        EvalError::TypeError(format!("flat() requires array, got {}", type_name(v)))
    })?;

    let mut result = Vec::new();
    for item in arr {
        if let JsonValue::Array(inner) = item {
            result.extend(inner.iter().cloned());
        } else {
            result.push(item.clone());
        }
    }

    Ok(JsonValue::Array(result))
}

fn fn_zip(a: &JsonValue, b: &JsonValue) -> EvalResult<JsonValue> {
    let a_arr = a.as_array().ok_or_else(|| {
        EvalError::TypeError(format!(
            "zip() first argument must be array, got {}",
            type_name(a)
        ))
    })?;
    let b_arr = b.as_array().ok_or_else(|| {
        EvalError::TypeError(format!(
            "zip() second argument must be array, got {}",
            type_name(b)
        ))
    })?;

    let pairs: Vec<JsonValue> = a_arr
        .iter()
        .zip(b_arr.iter())
        .map(|(x, y)| json!([x, y]))
        .collect();

    Ok(JsonValue::Array(pairs))
}

fn fn_range_1(end: &JsonValue) -> EvalResult<JsonValue> {
    let n = end
        .as_i64()
        .ok_or_else(|| EvalError::TypeError("range() requires integer argument".to_string()))?;
    let arr: Vec<JsonValue> = (0..n).map(|i| json!(i)).collect();
    Ok(JsonValue::Array(arr))
}

fn fn_range_2(start: &JsonValue, end: &JsonValue) -> EvalResult<JsonValue> {
    let s = start
        .as_i64()
        .ok_or_else(|| EvalError::TypeError("range() requires integer arguments".to_string()))?;
    let e = end
        .as_i64()
        .ok_or_else(|| EvalError::TypeError("range() requires integer arguments".to_string()))?;
    let arr: Vec<JsonValue> = (s..e).map(|i| json!(i)).collect();
    Ok(JsonValue::Array(arr))
}

fn fn_slice(v: &JsonValue, start: &JsonValue, end: &JsonValue) -> EvalResult<JsonValue> {
    let s = start
        .as_i64()
        .ok_or_else(|| EvalError::TypeError("slice() start must be integer".to_string()))?
        as usize;

    match v {
        JsonValue::Array(arr) => {
            let e = if end.is_null() {
                arr.len()
            } else {
                end.as_i64().ok_or_else(|| {
                    EvalError::TypeError("slice() end must be integer".to_string())
                })? as usize
            };
            let e = e.min(arr.len());
            let s = s.min(e);
            Ok(JsonValue::Array(arr[s..e].to_vec()))
        }
        JsonValue::String(str_val) => {
            let chars: Vec<char> = str_val.chars().collect();
            let e = if end.is_null() {
                chars.len()
            } else {
                end.as_i64().ok_or_else(|| {
                    EvalError::TypeError("slice() end must be integer".to_string())
                })? as usize
            };
            let e = e.min(chars.len());
            let s = s.min(e);
            Ok(json!(chars[s..e].iter().collect::<String>()))
        }
        _ => Err(EvalError::TypeError(format!(
            "slice() requires array or string, got {}",
            type_name(v)
        ))),
    }
}

fn fn_index_of(haystack: &JsonValue, needle: &JsonValue) -> EvalResult<JsonValue> {
    match haystack {
        JsonValue::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                if json_eq(item, needle) {
                    return Ok(json!(i as i64));
                }
            }
            Ok(json!(-1))
        }
        JsonValue::String(s) => {
            let needle = needle.as_str().ok_or_else(|| {
                EvalError::TypeError(
                    "index_of() needle must be string for string search".to_string(),
                )
            })?;
            match s.find(needle) {
                Some(pos) => Ok(json!(pos as i64)),
                None => Ok(json!(-1)),
            }
        }
        _ => Err(EvalError::TypeError(format!(
            "index_of() requires array or string, got {}",
            type_name(haystack)
        ))),
    }
}

fn fn_count(haystack: &JsonValue, needle: &JsonValue) -> EvalResult<JsonValue> {
    match haystack {
        JsonValue::Array(arr) => {
            let count = arr.iter().filter(|item| json_eq(item, needle)).count();
            Ok(json!(count as i64))
        }
        JsonValue::String(s) => {
            let needle = needle.as_str().ok_or_else(|| {
                EvalError::TypeError("count() needle must be string for string search".to_string())
            })?;
            Ok(json!(s.matches(needle).count() as i64))
        }
        _ => Err(EvalError::TypeError(format!(
            "count() requires array or string, got {}",
            type_name(haystack)
        ))),
    }
}

fn fn_merge(a: &JsonValue, b: &JsonValue) -> EvalResult<JsonValue> {
    match (a, b) {
        (JsonValue::Object(obj_a), JsonValue::Object(obj_b)) => {
            let mut result = obj_a.clone();
            for (k, v) in obj_b {
                result.insert(k.clone(), v.clone());
            }
            Ok(JsonValue::Object(result))
        }
        _ => Err(EvalError::TypeError(format!(
            "merge() requires two objects, got {} and {}",
            type_name(a),
            type_name(b)
        ))),
    }
}

fn fn_chunks(v: &JsonValue, size: &JsonValue) -> EvalResult<JsonValue> {
    let n = size.as_i64().ok_or_else(|| {
        EvalError::TypeError("chunks() size must be a positive integer".to_string())
    })?;
    if n <= 0 {
        return Err(EvalError::TypeError(
            "chunks() size must be a positive integer".to_string(),
        ));
    }
    let n = n as usize;

    match v {
        JsonValue::Array(arr) => {
            let chunks: Vec<JsonValue> = arr
                .chunks(n)
                .map(|c| JsonValue::Array(c.to_vec()))
                .collect();
            Ok(JsonValue::Array(chunks))
        }
        _ => Err(EvalError::TypeError(format!(
            "chunks() requires array, got {}",
            type_name(v)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_truthy() {
        assert!(!is_truthy(&json!(null)));
        assert!(!is_truthy(&json!(false)));
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&json!([])));
        assert!(!is_truthy(&json!({})));

        assert!(is_truthy(&json!(true)));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!("a")));
        assert!(is_truthy(&json!([1])));
        assert!(is_truthy(&json!({"a": 1})));
    }

    #[test]
    fn test_json_eq_cross_numeric() {
        assert!(json_eq(&json!(3), &json!(3.0)));
        assert!(json_eq(&json!(3.0), &json!(3)));
        assert!(!json_eq(&json!(3), &json!(3.1)));
    }

    #[test]
    fn test_json_eq_recursive() {
        assert!(json_eq(
            &json!({"a": [1, 2], "b": {"c": 3}}),
            &json!({"b": {"c": 3}, "a": [1, 2]})
        ));
        assert!(!json_eq(&json!({"a": [1, 2]}), &json!({"a": [1, 3]})));
    }

    #[test]
    fn test_negative_indexing() {
        let arr = json!([10, 20, 30]);
        assert_eq!(index_access(&arr, &json!(-1)).unwrap(), json!(30));
        assert_eq!(index_access(&arr, &json!(-2)).unwrap(), json!(20));
    }

    #[test]
    fn test_integer_division() {
        // 10 / 5 = 2 (integer)
        assert_eq!(eval_div(&json!(10), &json!(5)).unwrap(), json!(2));
        // 10 / 3 = 3.333... (float because not evenly divisible)
        let result = eval_div(&json!(10), &json!(3)).unwrap();
        assert!(result.is_f64());
    }
}
