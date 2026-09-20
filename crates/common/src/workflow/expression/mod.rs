//! # Workflow Expression Engine
//!
//! A complete expression evaluator for workflow templates, supporting arithmetic,
//! comparison, boolean logic, member access, and built-in functions over JSON values.
//!
//! ## Architecture
//!
//! The engine is structured as a classic three-phase interpreter:
//!
//! 1. **Lexer** (`tokenizer.rs`) — converts expression strings into a stream of tokens
//! 2. **Parser** (`parser.rs`) — builds an AST from tokens using recursive descent
//! 3. **Evaluator** (`evaluator.rs`) — walks the AST and produces a `JsonValue` result
//!
//! ## Supported Operators
//!
//! ### Arithmetic
//! - `+` (addition for numbers, concatenation for strings)
//! - `-` (subtraction, unary negation)
//! - `*`, `/`, `%` (multiplication, division, modulo)
//!
//! ### Comparison
//! - `==`, `!=` (equality — works on all types, recursive for objects/arrays)
//! - `>`, `<`, `>=`, `<=` (ordering — numbers and strings only)
//! - Float/int comparisons allowed: `3 == 3.0` → true
//!
//! ### Boolean / Logical
//! - `and`, `or`, `not`
//!
//! ### Membership & Access
//! - `.` — object property access
//! - `[n]` — array index / object bracket access
//! - `in` — membership test (item in list, key in object, substring in string)
//!
//! ## Built-in Functions
//!
//! ### Type conversion
//! - `string(v)`, `number(v)`, `int(v)`, `bool(v)`
//!
//! ### Introspection
//! - `type_of(v)`, `length(v)`, `keys(obj)`, `values(obj)`
//!
//! ### Math
//! - `abs(n)`, `floor(n)`, `ceil(n)`, `round(n)`, `min(a,b)`, `max(a,b)`, `sum(arr)`
//!
//! ### String
//! - `lower(s)`, `upper(s)`, `trim(s)`, `split(s, sep)`, `join(arr, sep)`
//! - `replace(s, old, new)`, `starts_with(s, prefix)`, `ends_with(s, suffix)`
//! - `match(pattern, s)` — regex match
//!
//! ### Collection
//! - `contains(haystack, needle)`, `reversed(v)`, `sort(arr)`, `unique(arr)`
//! - `flat(arr)`, `zip(a, b)`, `range(n)` / `range(start, end)`
//!
//! ### Workflow-specific
//! - `result()`, `succeeded()`, `failed()`, `timed_out()`, `cancelled()`

mod ast;
mod evaluator;
mod parser;
mod tokenizer;

pub use ast::{BinaryOp, Expr, UnaryOp};
pub use evaluator::{is_truthy, EvalContext, EvalError, EvalResult};
pub use parser::{ParseError, Parser};
pub use tokenizer::{Token, TokenKind, Tokenizer};

use serde_json::Value as JsonValue;

/// Parse and evaluate an expression string against the given context.
///
/// This is the main entry point for the expression engine. It tokenizes the
/// input, parses it into an AST, and evaluates it to produce a `JsonValue`.
pub fn eval_expression(input: &str, ctx: &dyn EvalContext) -> EvalResult<JsonValue> {
    let tokens = Tokenizer::new(input)
        .tokenize()
        .map_err(|e| EvalError::ParseError(format!("{}", e)))?;
    let ast = Parser::new(&tokens)
        .parse()
        .map_err(|e| EvalError::ParseError(format!("{}", e)))?;
    evaluator::eval(&ast, ctx)
}

/// Parse an expression string into an AST without evaluating it.
///
/// Useful for validation or inspection.
pub fn parse_expression(input: &str) -> Result<Expr, ParseError> {
    let tokens = Tokenizer::new(input)
        .tokenize()
        .map_err(|e| ParseError::TokenError(format!("{}", e)))?;
    Parser::new(&tokens).parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// A minimal eval context for integration tests.
    struct TestContext {
        variables: HashMap<String, JsonValue>,
    }

    impl TestContext {
        fn new() -> Self {
            Self {
                variables: HashMap::new(),
            }
        }

        fn with_var(mut self, name: &str, value: JsonValue) -> Self {
            self.variables.insert(name.to_string(), value);
            self
        }
    }

    impl EvalContext for TestContext {
        fn resolve_variable(&self, name: &str) -> EvalResult<JsonValue> {
            self.variables
                .get(name)
                .cloned()
                .ok_or_else(|| EvalError::VariableNotFound(name.to_string()))
        }

        fn call_workflow_function(
            &self,
            _name: &str,
            _args: &[JsonValue],
        ) -> EvalResult<Option<JsonValue>> {
            Ok(None)
        }
    }

    // ---------------------------------------------------------------
    // Arithmetic
    // ---------------------------------------------------------------

    #[test]
    fn test_integer_arithmetic() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("2 + 3", &ctx).unwrap(), json!(5));
        assert_eq!(eval_expression("10 - 4", &ctx).unwrap(), json!(6));
        assert_eq!(eval_expression("3 * 7", &ctx).unwrap(), json!(21));
        assert_eq!(eval_expression("15 / 5", &ctx).unwrap(), json!(3));
        assert_eq!(eval_expression("17 % 5", &ctx).unwrap(), json!(2));
    }

    #[test]
    fn test_float_arithmetic() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("2.5 + 1.5", &ctx).unwrap(), json!(4.0));
        assert_eq!(
            eval_expression("10.0 / 3.0", &ctx).unwrap(),
            json!(10.0 / 3.0)
        );
    }

    #[test]
    fn test_mixed_int_float() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("2 + 1.5", &ctx).unwrap(), json!(3.5));
        // Integer division yields float when not evenly divisible
        assert_eq!(eval_expression("10 / 4", &ctx).unwrap(), json!(2.5));
        assert_eq!(eval_expression("10 / 4.0", &ctx).unwrap(), json!(2.5));
        // Evenly divisible integer division stays integer
        assert_eq!(eval_expression("10 / 5", &ctx).unwrap(), json!(2));
    }

    #[test]
    fn test_operator_precedence() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("2 + 3 * 4", &ctx).unwrap(), json!(14));
        assert_eq!(eval_expression("(2 + 3) * 4", &ctx).unwrap(), json!(20));
        assert_eq!(eval_expression("10 - 2 * 3 + 1", &ctx).unwrap(), json!(5));
    }

    #[test]
    fn test_unary_negation() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("-5", &ctx).unwrap(), json!(-5));
        assert_eq!(eval_expression("-2 + 3", &ctx).unwrap(), json!(1));
        assert_eq!(eval_expression("-(2 + 3)", &ctx).unwrap(), json!(-5));
    }

    #[test]
    fn test_string_concatenation() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("\"hello\" + \" \" + \"world\"", &ctx).unwrap(),
            json!("hello world")
        );
    }

    // ---------------------------------------------------------------
    // Comparison
    // ---------------------------------------------------------------

    #[test]
    fn test_number_comparison() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("3 == 3", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3 != 4", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3 > 2", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3 < 2", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("3 >= 3", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3 <= 4", &ctx).unwrap(), json!(true));
    }

    #[test]
    fn test_int_float_equality() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("3 == 3.0", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3.0 == 3", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("3 != 3.1", &ctx).unwrap(), json!(true));
    }

    #[test]
    fn test_string_comparison() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("\"abc\" == \"abc\"", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("\"abc\" < \"abd\"", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("\"abc\" > \"abb\"", &ctx).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn test_null_equality() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("null == null", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("null != null", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("null == 0", &ctx).unwrap(), json!(false));
        assert_eq!(
            eval_expression("null == false", &ctx).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn test_array_equality() {
        let ctx = TestContext::new()
            .with_var("a", json!([1, 2, 3]))
            .with_var("b", json!([1, 2, 3]))
            .with_var("c", json!([1, 2, 4]));
        assert_eq!(eval_expression("a == b", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("a != c", &ctx).unwrap(), json!(true));
    }

    #[test]
    fn test_object_equality() {
        let ctx = TestContext::new()
            .with_var("a", json!({"x": 1, "y": 2}))
            .with_var("b", json!({"y": 2, "x": 1}))
            .with_var("c", json!({"x": 1, "y": 3}));
        assert_eq!(eval_expression("a == b", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("a != c", &ctx).unwrap(), json!(true));
    }

    // ---------------------------------------------------------------
    // Boolean / Logical
    // ---------------------------------------------------------------

    #[test]
    fn test_boolean_operators() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("true and true", &ctx).unwrap(), json!(true));
        assert_eq!(
            eval_expression("true and false", &ctx).unwrap(),
            json!(false)
        );
        assert_eq!(eval_expression("false or true", &ctx).unwrap(), json!(true));
        assert_eq!(
            eval_expression("false or false", &ctx).unwrap(),
            json!(false)
        );
        assert_eq!(eval_expression("not true", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("not false", &ctx).unwrap(), json!(true));
    }

    #[test]
    fn test_boolean_precedence() {
        let ctx = TestContext::new();
        // `and` binds tighter than `or`
        assert_eq!(
            eval_expression("true or false and false", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("(true or false) and false", &ctx).unwrap(),
            json!(false)
        );
    }

    // ---------------------------------------------------------------
    // Membership & access
    // ---------------------------------------------------------------

    #[test]
    fn test_dot_access() {
        let ctx = TestContext::new().with_var("obj", json!({"a": {"b": 42}}));
        assert_eq!(eval_expression("obj.a.b", &ctx).unwrap(), json!(42));
    }

    #[test]
    fn test_bracket_access() {
        let ctx = TestContext::new()
            .with_var("arr", json!([10, 20, 30]))
            .with_var("obj", json!({"key": "value"}));
        assert_eq!(eval_expression("arr[1]", &ctx).unwrap(), json!(20));
        assert_eq!(
            eval_expression("obj[\"key\"]", &ctx).unwrap(),
            json!("value")
        );
    }

    #[test]
    fn test_in_operator() {
        let ctx = TestContext::new()
            .with_var("arr", json!([1, 2, 3]))
            .with_var("obj", json!({"key": "val"}));
        assert_eq!(eval_expression("2 in arr", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("5 in arr", &ctx).unwrap(), json!(false));
        assert_eq!(
            eval_expression("\"key\" in obj", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("\"nope\" in obj", &ctx).unwrap(),
            json!(false)
        );
        assert_eq!(
            eval_expression("\"ell\" in \"hello\"", &ctx).unwrap(),
            json!(true)
        );
    }

    // ---------------------------------------------------------------
    // Built-in functions
    // ---------------------------------------------------------------

    #[test]
    fn test_length() {
        let ctx = TestContext::new()
            .with_var("arr", json!([1, 2, 3]))
            .with_var("obj", json!({"a": 1, "b": 2}));
        assert_eq!(eval_expression("length(arr)", &ctx).unwrap(), json!(3));
        assert_eq!(
            eval_expression("length(\"hello\")", &ctx).unwrap(),
            json!(5)
        );
        assert_eq!(eval_expression("length(obj)", &ctx).unwrap(), json!(2));
    }

    #[test]
    fn test_type_conversions() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("string(42)", &ctx).unwrap(), json!("42"));
        assert_eq!(
            eval_expression("number(\"3.15\")", &ctx).unwrap(),
            json!(3.15)
        );
        assert_eq!(eval_expression("int(3.9)", &ctx).unwrap(), json!(3));
        assert_eq!(eval_expression("int(\"42\")", &ctx).unwrap(), json!(42));
        assert_eq!(eval_expression("bool(1)", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("bool(0)", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("bool(\"\")", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("bool(\"x\")", &ctx).unwrap(), json!(true));
    }

    #[test]
    fn test_type_of() {
        let ctx = TestContext::new()
            .with_var("arr", json!([1]))
            .with_var("obj", json!({}));
        assert_eq!(
            eval_expression("type_of(42)", &ctx).unwrap(),
            json!("number")
        );
        assert_eq!(
            eval_expression("type_of(\"hi\")", &ctx).unwrap(),
            json!("string")
        );
        assert_eq!(
            eval_expression("type_of(true)", &ctx).unwrap(),
            json!("bool")
        );
        assert_eq!(
            eval_expression("type_of(null)", &ctx).unwrap(),
            json!("null")
        );
        assert_eq!(
            eval_expression("type_of(arr)", &ctx).unwrap(),
            json!("array")
        );
        assert_eq!(
            eval_expression("type_of(obj)", &ctx).unwrap(),
            json!("object")
        );
    }

    #[test]
    fn test_keys_values() {
        let ctx = TestContext::new().with_var("obj", json!({"b": 2, "a": 1}));
        let keys = eval_expression("sort(keys(obj))", &ctx).unwrap();
        assert_eq!(keys, json!(["a", "b"]));
        let values = eval_expression("sort(values(obj))", &ctx).unwrap();
        assert_eq!(values, json!([1, 2]));
    }

    #[test]
    fn test_math_functions() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("abs(-5)", &ctx).unwrap(), json!(5));
        assert_eq!(eval_expression("floor(3.7)", &ctx).unwrap(), json!(3));
        assert_eq!(eval_expression("ceil(3.2)", &ctx).unwrap(), json!(4));
        assert_eq!(eval_expression("round(3.5)", &ctx).unwrap(), json!(4));
        assert_eq!(eval_expression("min(3, 7)", &ctx).unwrap(), json!(3));
        assert_eq!(eval_expression("max(3, 7)", &ctx).unwrap(), json!(7));
        assert_eq!(
            eval_expression("sum([1, 2, 3, 4])", &ctx).unwrap(),
            json!(10)
        );
    }

    #[test]
    fn test_string_functions() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("lower(\"HELLO\")", &ctx).unwrap(),
            json!("hello")
        );
        assert_eq!(
            eval_expression("upper(\"hello\")", &ctx).unwrap(),
            json!("HELLO")
        );
        assert_eq!(
            eval_expression("trim(\"  hi  \")", &ctx).unwrap(),
            json!("hi")
        );
        assert_eq!(
            eval_expression("replace(\"hello world\", \"world\", \"rust\")", &ctx).unwrap(),
            json!("hello rust")
        );
        assert_eq!(
            eval_expression("starts_with(\"hello\", \"hel\")", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("ends_with(\"hello\", \"llo\")", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("split(\"a,b,c\", \",\")", &ctx).unwrap(),
            json!(["a", "b", "c"])
        );
        assert_eq!(
            eval_expression("join([\"a\", \"b\", \"c\"], \",\")", &ctx).unwrap(),
            json!("a,b,c")
        );
    }

    #[test]
    fn test_regex_match() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("match(\"^hello\", \"hello world\")", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("match(\"^world\", \"hello world\")", &ctx).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn test_collection_functions() {
        let ctx = TestContext::new().with_var("arr", json!([3, 1, 2]));
        assert_eq!(
            eval_expression("sort(arr)", &ctx).unwrap(),
            json!([1, 2, 3])
        );
        assert_eq!(
            eval_expression("reversed(arr)", &ctx).unwrap(),
            json!([2, 1, 3])
        );
        assert_eq!(
            eval_expression("unique([1, 2, 2, 3, 1])", &ctx).unwrap(),
            json!([1, 2, 3])
        );
        assert_eq!(
            eval_expression("flat([[1, 2], [3, 4]])", &ctx).unwrap(),
            json!([1, 2, 3, 4])
        );
        assert_eq!(
            eval_expression("zip([1, 2], [\"a\", \"b\"])", &ctx).unwrap(),
            json!([[1, "a"], [2, "b"]])
        );
    }

    #[test]
    fn test_range() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("range(5)", &ctx).unwrap(),
            json!([0, 1, 2, 3, 4])
        );
        assert_eq!(
            eval_expression("range(2, 5)", &ctx).unwrap(),
            json!([2, 3, 4])
        );
    }

    #[test]
    fn test_reversed_string() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("reversed(\"abc\")", &ctx).unwrap(),
            json!("cba")
        );
    }

    #[test]
    fn test_contains_function() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("contains([1, 2, 3], 2)", &ctx).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_expression("contains(\"hello\", \"ell\")", &ctx).unwrap(),
            json!(true)
        );
    }

    // ---------------------------------------------------------------
    // Complex expressions
    // ---------------------------------------------------------------

    #[test]
    fn test_complex_expression() {
        let ctx = TestContext::new().with_var("items", json!([1, 2, 3, 4, 5]));
        assert_eq!(
            eval_expression("length(items) > 3 and 5 in items", &ctx).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn test_chained_access() {
        let ctx = TestContext::new().with_var(
            "data",
            json!({"users": [{"name": "Alice"}, {"name": "Bob"}]}),
        );
        assert_eq!(
            eval_expression("data.users[1].name", &ctx).unwrap(),
            json!("Bob")
        );
    }

    #[test]
    fn test_ternary_via_boolean() {
        let ctx = TestContext::new().with_var("x", json!(10));
        // No ternary operator, but boolean expressions work for conditions
        assert_eq!(
            eval_expression("x > 5 and x < 20", &ctx).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn test_no_implicit_type_coercion() {
        let ctx = TestContext::new();
        // String + number should error, not silently coerce
        assert!(eval_expression("\"hello\" + 5", &ctx).is_err());
        // Comparing different types (other than int/float) should return false for ==
        assert_eq!(eval_expression("\"3\" == 3", &ctx).unwrap(), json!(false));
    }

    #[test]
    fn test_division_by_zero() {
        let ctx = TestContext::new();
        assert!(eval_expression("5 / 0", &ctx).is_err());
        assert!(eval_expression("5 % 0", &ctx).is_err());
    }

    #[test]
    fn test_array_literal() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("[1, 2, 3]", &ctx).unwrap(),
            json!([1, 2, 3])
        );
        assert_eq!(
            eval_expression("[\"a\", \"b\"]", &ctx).unwrap(),
            json!(["a", "b"])
        );
    }

    #[test]
    fn test_nested_function_calls() {
        let ctx = TestContext::new();
        assert_eq!(
            eval_expression("length(split(\"a,b,c\", \",\"))", &ctx).unwrap(),
            json!(3)
        );
        assert_eq!(
            eval_expression("join(sort([\"c\", \"a\", \"b\"]), \"-\")", &ctx).unwrap(),
            json!("a-b-c")
        );
    }

    #[test]
    fn test_boolean_literals() {
        let ctx = TestContext::new();
        assert_eq!(eval_expression("true", &ctx).unwrap(), json!(true));
        assert_eq!(eval_expression("false", &ctx).unwrap(), json!(false));
        assert_eq!(eval_expression("null", &ctx).unwrap(), json!(null));
    }
}
