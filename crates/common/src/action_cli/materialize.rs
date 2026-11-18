use std::fmt;
use std::io::{self, Write};

use serde_json::Value;

use super::{
    failure, ArgumentLimits, ArgumentPlan, CliBinding, Encoding, ErrorCode, Group, InputDelivery,
    OptionStyle, Result,
};
use crate::value_schema::{self, CompiledContract};

/// Prepared input is sensitive process data. Debug output never includes bytes.
pub struct PreparedArguments {
    argv: Vec<String>,
    stdin_json: Option<String>,
}

impl PreparedArguments {
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn stdin_json(&self) -> Option<&str> {
        self.stdin_json.as_deref()
    }
}

impl fmt::Debug for PreparedArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedArguments")
            .field("argument_count", &self.argv.len())
            .field("has_stdin", &self.stdin_json.is_some())
            .finish()
    }
}

impl ArgumentPlan {
    /// Materialize already-defaulted, restored input. The protected paths are
    /// the worker's recorded-provenance JSON Pointers, never caller assertions.
    /// Resolving/verifying the program and spawning remain worker responsibilities.
    pub fn materialize(
        &self,
        resolved_program: &str,
        input: &Value,
        protected_paths: &[String],
        delivery: InputDelivery,
        limits: ArgumentLimits,
    ) -> Result<PreparedArguments> {
        let ceilings = ArgumentLimits::default();
        if limits.max_arguments > ceilings.max_arguments
            || limits.max_argument_bytes > ceilings.max_argument_bytes
            || limits.max_total_bytes > ceilings.max_total_bytes
        {
            return Err(failure(ErrorCode::InvalidBinding, "limits"));
        }
        if resolved_program.is_empty() || resolved_program.contains('\0') {
            return Err(failure(ErrorCode::InvalidValue, "launch.program"));
        }
        let compiled = CompiledContract::new(self.contract())?;
        compiled.validate(input)?;
        let mut sensitive = compiled.secret_paths(input)?;
        let mut protected_bytes = 0usize;
        if protected_paths.len() > self.contract().limits.max_value_nodes {
            return Err(failure(ErrorCode::LimitExceeded, "protected_paths"));
        }
        for path in protected_paths {
            protected_bytes = protected_bytes.saturating_add(path.len());
            if protected_bytes > self.contract().limits.max_value_bytes {
                return Err(failure(ErrorCode::LimitExceeded, "protected_paths"));
            }
            if !valid_pointer(path) {
                return Err(failure(ErrorCode::InvalidValue, "protected_paths"));
            }
        }
        sensitive.extend_from_slice(protected_paths);
        if delivery == InputDelivery::None && !sensitive.is_empty() {
            return Err(failure(ErrorCode::NoSecureDelivery, "parameter_delivery"));
        }
        let mut buffer = ArgumentBuffer {
            argv: Vec::new(),
            bytes: resolved_program.len().saturating_add(1),
            limits,
        };
        if buffer.bytes > limits.max_total_bytes {
            return Err(failure(ErrorCode::LimitExceeded, "launch.program"));
        }
        for group in self.groups.values() {
            match group {
                Group::Literal { value, path } => buffer.push(value, path)?,
                Group::Parameter(parameter) => {
                    let pointer = parameter
                        .segments
                        .iter()
                        .fold(String::new(), |path, key| value_schema::pointer(&path, key));
                    if sensitive.iter().any(|path| overlaps(&pointer, path)) {
                        return Err(failure(ErrorCode::SecretValue, &parameter.display_path));
                    }
                    let Some(value) = input.pointer(&pointer) else {
                        if parameter.binding.requires_value() {
                            return Err(failure(ErrorCode::MissingValue, &parameter.display_path));
                        }
                        continue;
                    };
                    if parameter.binding.expands() {
                        let items = value.as_array().ok_or_else(|| {
                            failure(ErrorCode::InvalidValue, &parameter.display_path)
                        })?;
                        for item in items {
                            buffer.emit(&parameter.binding, item, &parameter.display_path)?;
                        }
                    } else {
                        buffer.emit(&parameter.binding, value, &parameter.display_path)?;
                    }
                }
            }
        }
        let stdin_json = match delivery {
            InputDelivery::None => None,
            InputDelivery::Stdin => Some(
                serde_json::to_string(input)
                    .map_err(|_| failure(ErrorCode::InvalidValue, "parameters"))?,
            ),
        };
        Ok(PreparedArguments {
            argv: buffer.argv,
            stdin_json,
        })
    }
}

struct ArgumentBuffer {
    argv: Vec<String>,
    bytes: usize,
    limits: ArgumentLimits,
}

impl ArgumentBuffer {
    fn push(&mut self, value: &str, path: &str) -> Result<()> {
        if value.contains('\0') {
            return Err(failure(ErrorCode::InvalidValue, path));
        }
        let bytes = self.bytes.saturating_add(value.len()).saturating_add(1);
        if self.argv.len() >= self.limits.max_arguments
            || value.len() > self.limits.max_argument_bytes
            || bytes > self.limits.max_total_bytes
        {
            return Err(failure(ErrorCode::LimitExceeded, path));
        }
        self.bytes = bytes;
        self.argv.push(value.to_owned());
        Ok(())
    }

    fn emit(&mut self, binding: &CliBinding, value: &Value, path: &str) -> Result<()> {
        if let CliBinding::Flag {
            name, false_name, ..
        } = binding
        {
            return match value.as_bool() {
                Some(true) => self.push(name, path),
                Some(false) => match false_name {
                    Some(name) => self.push(name, path),
                    None => Ok(()),
                },
                None => Err(failure(ErrorCode::InvalidValue, path)),
            };
        }
        let available = self
            .limits
            .max_argument_bytes
            .min(self.limits.max_total_bytes.saturating_sub(self.bytes + 1));
        let encoded = encode(value, binding.encoding(), available, path)?;
        match binding {
            CliBinding::Positional { .. } => self.push(&encoded, path),
            CliBinding::Option { name, style, .. } => match style {
                OptionStyle::Separate => {
                    self.push(name, path)?;
                    self.push(&encoded, path)
                }
                OptionStyle::Equals | OptionStyle::Attached => {
                    let separator = if *style == OptionStyle::Equals {
                        "="
                    } else {
                        ""
                    };
                    // Both pieces have already been individually bounded; push
                    // also checks the combined argument and cumulative budgets.
                    self.push(&format!("{name}{separator}{encoded}"), path)
                }
            },
            CliBinding::Flag { .. } => unreachable!("flag handled before encoding"),
        }
    }
}

fn encode(value: &Value, encoding: Encoding, available: usize, path: &str) -> Result<String> {
    if value.is_null() {
        return Err(failure(ErrorCode::InvalidValue, path));
    }
    if encoding == Encoding::Scalar {
        let result = match value {
            Value::String(text) => {
                if text.len() > available {
                    return Err(failure(ErrorCode::LimitExceeded, path));
                }
                if text.contains('\0') {
                    return Err(failure(ErrorCode::InvalidValue, path));
                }
                text.clone()
            }
            Value::Number(number) => number.to_string(),
            Value::Bool(value) => value.to_string(),
            _ => return Err(failure(ErrorCode::InvalidValue, path)),
        };
        if result.len() > available {
            return Err(failure(ErrorCode::LimitExceeded, path));
        }
        return Ok(result);
    }
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        max: available,
    };
    write_json(value, &mut writer).map_err(|_| failure(ErrorCode::LimitExceeded, path))?;
    Ok(String::from_utf8(writer.bytes).expect("JSON serialization emits UTF-8"))
}

struct LimitedWriter {
    bytes: Vec<u8>,
    max: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("argument byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sort explicitly: transitive serde_json features may enable preserve_order.
fn write_json(value: &Value, writer: &mut LimitedWriter) -> io::Result<()> {
    match value {
        Value::Object(map) => {
            writer.write_all(b"{")?;
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key)?;
                writer.write_all(b":")?;
                write_json(&map[key], writer)?;
            }
            writer.write_all(b"}")
        }
        Value::Array(items) => {
            writer.write_all(b"[")?;
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                write_json(item, writer)?;
            }
            writer.write_all(b"]")
        }
        _ => serde_json::to_writer(writer, value).map_err(io::Error::other),
    }
}

fn valid_pointer(value: &str) -> bool {
    if !value.is_empty() && !value.starts_with('/') {
        return false;
    }
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

fn overlaps(left: &str, right: &str) -> bool {
    fn ancestor(parent: &str, child: &str) -> bool {
        child == parent
            || child
                .strip_prefix(parent)
                .is_some_and(|tail| tail.starts_with('/'))
    }
    ancestor(left, right) || ancestor(right, left)
}
