# Quick Reference: Action Output Format and Schema

**Last Updated:** 2026-02-07  
**Status:** Current standard for all actions

## TL;DR

- ✅ **DO:** Set `output_format` to "text", "json", or "yaml"
- ✅ **DO:** Define `output_schema` for structured outputs (json/yaml only)
- ❌ **DON'T:** Include stdout/stderr/exit_code in output schema (captured automatically)
- 💡 **Output schema** describes the shape of structured data sent to stdout

## Output Format Field

All actions must specify an `output_format` field in their YAML definition:

```yaml
name: my_action
ref: mypack.my_action
runner_type: shell
entry_point: my_action.sh

# Output format: text, json, or yaml
output_format: text  # or json, or yaml
```

### Supported Formats

| Format | Description | Worker Behavior | Use Case |
|--------|-------------|-----------------|----------|
| `text` | Plain text output | Stored as-is in execution result | Simple messages, logs, unstructured data |
| `json` | JSON structured data | Parsed into JSONB field | APIs, structured results, complex data |
| `yaml` | YAML structured data | Parsed into JSONB field | Configuration, human-readable structured data |

## Output Schema

The `output_schema` field describes the **shape of structured data** written to stdout:

- **Only applicable** for `output_format: json` or `output_format: yaml`
- **Not needed** for `output_format: text` (no parsing occurs)
- **Should NOT include** execution metadata (stdout/stderr/exit_code)

### Text Output Actions

For actions that output plain text, omit the output schema:

```yaml
name: echo
ref: core.echo
runner_type: shell
entry_point: echo.sh

# Output format: text (no structured data parsing)
output_format: text

parameters:
  type: object
  properties:
    message:
      type: string

# Output schema: not applicable for text output format
# The action outputs plain text to stdout
```

**Action script:**
```bash
#!/bin/bash
INPUT=$(cat)
MESSAGE=$(echo "$INPUT" | jq -r '.message // ""')
echo "$MESSAGE"  # Plain text to stdout
```

### JSON Output Actions

For actions that output JSON, define the schema:

```yaml
name: http_request
ref: example.http_request
runner_type: python
entry_point: http_request.py

# Output format: json (structured data parsing enabled)
output_format: json

parameters:
  type: object
  properties:
    url:
      type: string
      required: true

# Output schema: describes the JSON structure written to stdout
# Note: stdout/stderr/exit_code are captured automatically by the execution system
output_schema:
  type: object
  properties:
    status_code:
      type: integer
      description: "HTTP status code"
    body:
      type: string
      description: "Response body as text"
    success:
      type: boolean
      description: "Whether the request was successful (2xx status)"
```

**Action script:**
```python
#!/usr/bin/env python3
import json
import sys

def main():
    params = json.loads(sys.stdin.read() or '{}')
    
    # Perform HTTP request logic
    result = {
        "status_code": 200,
        "body": "Response body",
        "success": True
    }
    
    # Output JSON to stdout (worker will parse and store in execution.result)
    print(json.dumps(result, indent=2))

if __name__ == "__main__":
    main()
```

### YAML Output Actions

For actions that output YAML:

```yaml
name: get_config
ref: mypack.get_config
runner_type: shell
entry_point: get_config.sh

# Output format: yaml (structured data parsing enabled)
output_format: yaml

# Output schema: describes the YAML structure written to stdout
output_schema:
  type: object
  properties:
    server:
      type: object
      properties:
        host:
          type: string
        port:
          type: integer
    database:
      type: object
      properties:
        url:
          type: string
```

**Action script:**
```bash
#!/bin/bash
cat <<EOF
server:
  host: localhost
  port: 8080
database:
  url: postgresql://localhost/db
EOF
```

## Execution Metadata (Automatic)

The following metadata is **automatically captured** by the worker for every execution:

| Field | Type | Description | Source |
|-------|------|-------------|--------|
| `stdout` | string | Standard output from action | Captured by worker |
| `stderr` | string | Standard error output | Captured by worker, written to log file |
| `exit_code` | integer | Process exit code | Captured by worker |
| `duration_ms` | integer | Execution duration | Calculated by worker |

**Do NOT include these in your output schema** - they are execution system concerns, not action output concerns.

## Worker Behavior

### Text Format
```
Action writes to stdout: "Hello, World!"
↓
Worker captures stdout as-is
↓
Execution.result = null (no parsing)
Execution.stdout = "Hello, World!"
Execution.exit_code = 0
```

### JSON Format
```
Action writes to stdout: {"status": "success", "count": 42}
↓
Worker parses JSON
↓
Execution.result = {"count": 42, "message": "done"} (JSONB)
Execution.stdout = '{"count": 42, "message": "done"}' (raw)
Execution.exit_code = 0
```

### YAML Format
```
Action writes to stdout: 
  status: success
  count: 42
↓
Worker parses YAML to JSON
↓
Execution.result = {"count": 42, "message": "done"} (JSONB)
Execution.stdout = "count: 42\nmessage: done\n" (raw)
Execution.exit_code = 0
```

## Error Handling

### Stderr Usage

- **Purpose:** Diagnostic messages, warnings, errors
- **Storage:** Written to execution log file (not inline with result)
- **Visibility:** Available via execution logs API endpoint
- **Best Practice:** Use stderr for error messages, not stdout

**Example:**
```bash
#!/bin/bash
if [ -z "$URL" ]; then
    echo "ERROR: URL parameter is required" >&2  # stderr
    exit 1
fi

# Normal output to stdout
echo "Success"
```

### Exit Codes

- **0:** Success
- **Non-zero:** Failure
- **Captured automatically:** Worker records exit code in execution record
- **Don't output in JSON:** Exit code is metadata, not result data

## Pattern Examples

### Example 1: Simple Text Action

```yaml
# echo.yaml
name: echo
output_format: text
parameters:
  properties:
    message:
      type: string
```

```bash
# echo.sh
#!/bin/bash
INPUT=$(cat)
MESSAGE=$(echo "$INPUT" | jq -r '.message // ""')
echo "$MESSAGE"
```

### Example 2: Structured JSON Action

```yaml
# validate_json.yaml
name: validate_json
output_format: json
parameters:
  properties:
    json_data:
      type: string
output_schema:
  type: object
  properties:
    valid:
      type: boolean
    errors:
      type: array
      items:
        type: string
```

```python
# validate_json.py
#!/usr/bin/env python3
import json
import sys

def main():
    params = json.loads(sys.stdin.read() or '{}')
    json_data = params.get('json_data', '')
    
    errors = []
    valid = False
    
    try:
        json.loads(json_data)
        valid = True
    except json.JSONDecodeError as e:
        errors.append(str(e))
    
    result = {"valid": valid, "errors": errors}
    
    # Output JSON to stdout
    print(json.dumps(result))

if __name__ == "__main__":
    main()
```

### Example 3: API Wrapper with JSON Output

```yaml
# github_pr_info.yaml
name: github_pr_info
output_format: json
parameters:
  properties:
    repo:
      type: string
      required: true
    pr_number:
      type: integer
      required: true
output_schema:
  type: object
  properties:
    title:
      type: string
    state:
      type: string
      enum: [open, closed, merged]
    author:
      type: string
    created_at:
      type: string
      format: date-time
```

## Migration from Old Pattern

### Before (Incorrect)

```yaml
# DON'T DO THIS - includes execution metadata
output_schema:
  type: object
  properties:
    stdout:       # ❌ Execution metadata
      type: string
    stderr:       # ❌ Execution metadata
      type: string
    exit_code:    # ❌ Execution metadata
      type: integer
    result:
      type: object  # ❌ Actual result unnecessarily nested
```

### After (Correct)

```yaml
# DO THIS - only describe the actual data structure your action outputs
output_format: json
output_schema:
  type: object
  properties:
    count:
      type: integer
    items:
      type: array
      items:
        type: string
    # No stdout/stderr/exit_code - those are captured automatically
```

## Best Practices

1. **Choose the right format:**
   - Use `text` for simple messages, logs, or unstructured output
   - Use `json` for structured data, API responses, complex results
   - Use `yaml` for human-readable configuration or structured output

2. **Keep output schema clean:**
   - Only describe the actual data structure
   - Don't include execution metadata
   - Don't nest result under a "result" or "data" key unless semantic

3. **Use stderr for diagnostics:**
   - Error messages go to stderr, not stdout
   - Debugging output goes to stderr
   - Normal results go to stdout

4. **Exit codes matter:**
   - 0 = success (even if result indicates failure semantically)
   - Non-zero = execution failure (script error, crash, etc.)
   - Don't output exit code in JSON - it's captured automatically

5. **Validate your schema:**
   - Ensure output schema matches actual JSON/YAML structure
   - Test with actual action outputs
   - Use JSON Schema validation tools

6. **Document optional fields:**
   - Mark fields that may not always be present
   - Provide descriptions for all fields
   - Include examples in action documentation

## Testing

### Test Text Output
```bash
echo '{"message": "test"}' | ./action.sh
# Verify: Plain text output, no JSON structure
```

### Test JSON Output
```bash
echo '{"url": "https://example.com"}' | ./action.py | jq .
# Verify: Valid JSON, matches schema
```

### Test Error Handling
```bash
echo '{}' | ./action.sh 2>&1
# Verify: Errors to stderr, proper exit code
```

### Test Schema Compliance
```bash
OUTPUT=$(echo '{"param": "value"}' | ./action.py)
echo "$OUTPUT" | jq -e '.status and .data' > /dev/null
# Verify: Output has required fields from schema
```

## Common Pitfalls

### ❌ Pitfall 1: Including Execution Metadata
```yaml
# WRONG
output_schema:
  properties:
    exit_code:      # ❌ Automatic
      type: integer
    stdout:         # ❌ Automatic
      type: string
```

### ❌ Pitfall 2: Missing output_format
```yaml
# WRONG - no output_format specified
name: my_action
output_schema:  # How should this be parsed?
  type: object
```

### ❌ Pitfall 3: Text Format with Schema
```yaml
# WRONG - text format doesn't need schema
output_format: text
output_schema:  # ❌ Ignored for text format
  type: object
```

### ❌ Pitfall 4: Unnecessary Nesting
```bash
# WRONG - unnecessary "result" wrapper
echo '{"result": {"count": 5, "name": "test"}}'  # ❌

# RIGHT - output the data structure directly
echo '{"count": 5, "name": "test"}'  # ✅
```

## References

- [Action Parameter Handling](./QUICKREF-action-parameters.md) - Stdin-based parameter delivery
- [Core Pack Actions](../packs/core/actions/README.md) - Reference implementations
- [Worker Service Architecture](./architecture/worker-service.md) - How worker processes actions

## See Also

- Execution API endpoints (for retrieving results)
- Workflow parameter mapping (for using action outputs)
- Logging configuration (for stderr handling)