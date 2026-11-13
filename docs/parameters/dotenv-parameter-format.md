# DOTENV Parameter Format

## Overview

The DOTENV parameter format is used to pass action parameters securely via stdin in a shell-compatible format. This format is particularly useful for shell scripts that need to parse parameters without relying on external tools like `jq`.

## Format Specification

### Basic Format

Parameters are formatted as `key='value'` pairs, one per line:

```bash
url='https://example.com'
method='GET'
timeout='30'
verify_ssl='true'
```

### Nested Object Flattening

Nested JSON objects are automatically flattened using dot notation. This allows shell scripts to easily parse complex parameter structures.

**Input JSON:**
```json
{
  "url": "https://example.com",
  "headers": {
    "Content-Type": "application/json",
    "Authorization": "Bearer token123"
  },
  "query_params": {
    "page": "1",
    "size": "10"
  }
}
```

**Output DOTENV:**
```bash
headers.Authorization='Bearer token123'
headers.Content-Type='application/json'
query_params.page='1'
query_params.size='10'
url='https://example.com'
```

### Empty Objects

Empty objects (`{}`) are omitted from the output entirely. They do not produce any dotenv entries.

**Input:**
```json
{
  "url": "https://example.com",
  "headers": {},
  "query_params": {}
}
```

**Output:**
```bash
url='https://example.com'
```

### Arrays

Arrays are serialized as JSON strings:

**Input:**
```json
{
  "tags": ["web", "api", "production"]
}
```

**Output:**
```bash
tags='["web","api","production"]'
```

### Special Characters

Single quotes in values are escaped using the shell-safe `'\''` pattern:

**Input:**
```json
{
  "message": "It's working!"
}
```

**Output:**
```bash
message='It'\''s working!'
```

## Shell Script Parsing

### Basic Parameter Parsing

```bash
#!/bin/sh

# Read DOTENV-formatted parameters from stdin until EOF
while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"

    # Remove quotes
    case "$value" in
        \"*\") value="${value#\"}"; value="${value%\"}" ;;
        \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac

    # Process parameters
    case "$key" in
        url) url="$value" ;;
        method) method="$value" ;;
        timeout) timeout="$value" ;;
    esac
done
```

### Parsing Nested Objects

For flattened nested objects, use pattern matching on the key prefix:

```bash
# Create temporary files for nested data
headers_file=$(mktemp)
query_params_file=$(mktemp)

while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"

    # Remove quotes
    case "$value" in
        \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac

    # Process parameters
    case "$key" in
        url) url="$value" ;;
        method) method="$value" ;;
        headers.*)
            # Extract nested key (e.g., "Content-Type" from "headers.Content-Type")
            nested_key="${key#headers.}"
            printf '%s: %s\n' "$nested_key" "$value" >> "$headers_file"
            ;;
        query_params.*)
            nested_key="${key#query_params.}"
            printf '%s=%s\n' "$nested_key" "$value" >> "$query_params_file"
            ;;
    esac
done

# Use the parsed data
if [ -s "$headers_file" ]; then
    while IFS= read -r header; do
        curl_args="$curl_args -H '$header'"
    done < "$headers_file"
fi
```

## Configuration

### Action YAML Configuration

Specify DOTENV format in your action YAML:

```yaml
ref: mypack.myaction
entry_point: myaction.sh
parameter_delivery: stdin
parameter_format: dotenv  # Use dotenv format
output_format: json
```

### Supported Formats

- `dotenv` - Shell-friendly key='value' format with nested object flattening
- `json` - Standard JSON format
- `yaml` - YAML format

### Supported Delivery Methods

- `stdin` - Parameters passed via stdin (recommended for security)
- `file` - Parameters written to a temporary file

## Security Considerations

### Why DOTENV + STDIN?

This combination provides several security benefits:

1. **No process list exposure**: Parameters don't appear in `ps aux` output
2. **No shell escaping issues**: Values are properly quoted
3. **Secret protection**: Sensitive values passed via stdin, not environment variables
4. **No external dependencies**: Pure POSIX shell parsing without `jq` or other tools

### Secret Handling

Secrets are merged into the parameters document before delivery. They appear as regular key-value pairs in the DOTENV output. Secrets are never included in environment variables or parameter files.

```bash
# All parameters (including secrets) delivered as a single document
url='https://api.example.com'
api_key='secret123'
password='hunter2'
```

## Examples

### Example 1: HTTP Request Action

**Action Configuration:**
```yaml
ref: example.http_request
parameter_delivery: stdin
parameter_format: dotenv
```

**Execution Parameters:**
```json
{
  "url": "https://api.example.com/users",
  "method": "POST",
  "headers": {
    "Content-Type": "application/json",
    "User-Agent": "Attune/1.0"
  },
  "query_params": {
    "page": "1",
    "limit": "10"
  }
}
```

**Stdin Input:**
```bash
headers.Content-Type='application/json'
headers.User-Agent='Attune/1.0'
method='POST'
query_params.limit='10'
query_params.page='1'
url='https://api.example.com/users'
```

### Example 2: Simple Shell Action

**Action Configuration:**
```yaml
ref: mypack.greet
parameter_delivery: stdin
parameter_format: dotenv
```

**Execution Parameters:**
```json
{
  "name": "Alice",
  "greeting": "Hello"
}
```

**Stdin Input:**
```bash
greeting='Hello'
name='Alice'
```

## Troubleshooting

### Issue: Parameters Not Received

**Symptom:** Action receives empty or incorrect parameter values.

**Solution:** Ensure you're reading stdin until EOF:

```bash
while IFS= read -r line; do
    [ -z "$line" ] && continue
    # ... parse line
done
```

### Issue: Nested Objects Not Parsed

**Symptom:** Headers or query params not being set correctly.

**Solution:** Use pattern matching to detect dotted keys:

```bash
case "$key" in
    headers.*)
        nested_key="${key#headers.}"
        # Process nested key
        ;;
esac
```

### Issue: Special Characters Corrupted

**Symptom:** Values with single quotes are malformed.

**Solution:** The worker automatically escapes single quotes using `'\''`. Make sure to remove quotes correctly:

```bash
# Remove quotes (handles escaped quotes correctly)
case "$value" in
    \'*\') value="${value#\'}"; value="${value%\'}" ;;
esac
```

## Best Practices

1. **Always read until delimiter**: Don't stop reading stdin early
2. **Handle empty objects**: Check if files are empty before processing
3. **Use temporary files**: For nested objects, write to temp files for easier processing
4. **Validate required parameters**: Check that required values are present
5. **Clean up temp files**: Use `trap` to ensure cleanup on exit

```bash
#!/bin/sh
set -e

# Setup cleanup
headers_file=$(mktemp)
trap "rm -f $headers_file" EXIT

# Parse parameters...
```

## Implementation Details

The parameter flattening is implemented in `crates/worker/src/runtime/parameter_passing.rs`:

- Nested objects are recursively flattened with dot notation
- Empty objects produce no output entries
- Arrays are JSON-serialized as strings
- Output is sorted alphabetically for consistency
- Single quotes are escaped using shell-safe `'\''` pattern

## See Also

- [Action Parameter Schema](../packs/pack-structure.md#parameters)
- [Secrets Management](../authentication/secrets-management.md)
- [Shell Runtime](../architecture/worker-service.md#shell-runtime)