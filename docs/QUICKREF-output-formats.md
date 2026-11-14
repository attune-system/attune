# Quick Reference: Action Output Formats

## TL;DR

Actions can specify how their stdout should be parsed:
- `text` (default): No parsing, raw stdout only
- `json`: Parse last line as JSON
- `yaml`: Parse entire output as YAML  
- `jsonl`: Parse each line as JSON, return array

## Action Definition

```yaml
name: my_action
output_format: json  # text | json | yaml | jsonl
output_schema:
  type: object  # Use 'array' for jsonl
  properties:
    result: { type: string }
```

## Format Behaviors

| Format | Parses | Result | Best For |
|--------|--------|--------|----------|
| `text` | Nothing | `null` | Simple messages, logs |
| `json` | Last line | Object/Value | API responses, single results |
| `yaml` | Entire stdout | Object/Value | Configs, nested data |
| `jsonl` | Each line | Array | Lists, streaming, batches |

## Examples

### Text (no parsing)
```bash
echo "Hello, World!"
# Result: null (stdout captured separately)
```

### JSON (last line)
```bash
echo "Processing..."
echo '{"status": 200, "data": "success"}'
# Result: {"status": 200, "data": "success"}
```

### YAML (entire output)
```bash
cat <<EOF
version: 1.0
settings:
  enabled: true
  timeout: 30
EOF
# Result: {"version": "1.0", "settings": {...}}
```

### JSONL (each line → array)
```bash
echo '{"id": 1, "name": "Alice"}'
echo '{"id": 2, "name": "Bob"}'
echo '{"id": 3, "name": "Charlie"}'
# Result: [{"id": 1, ...}, {"id": 2, ...}, {"id": 3, ...}]
```

## Action Script Templates

### Bash + JSON
```bash
#!/bin/bash
result=$(do_work)
echo "{\"result\": \"$result\", \"status\": \"ok\"}"
```

### Python + JSON
```python
#!/usr/bin/env python3
import json
result = do_work()
print(json.dumps({"result": result, "status": "ok"}))
```

### Bash + JSONL
```bash
#!/bin/bash
for item in $(ls); do
  size=$(stat -f%z "$item")
  echo "{\"name\": \"$item\", \"size\": $size}"
done
```

### Python + JSONL
```python
#!/usr/bin/env python3
import json
for item in get_items():
    print(json.dumps({"id": item.id, "value": item.value}))
```

## Common Patterns

### Informational + Result (JSON)
```bash
echo "Starting process..." >&2  # Log to stderr
echo "Processing 100 items..." >&2
echo '{"processed": 100, "errors": 0}'  # JSON on last line
```

### Mixed Output (JSONL)
```bash
echo "Scanning directory..." >&2  # Non-JSON ignored
echo '{"file": "a.txt", "size": 1024}'  # Valid JSON
echo "Found 2 files" >&2  # Non-JSON ignored
echo '{"file": "b.txt", "size": 2048}'  # Valid JSON
```

## Execution Result Structure

```json
{
  "exit_code": 0,
  "succeeded": true,
  "duration_ms": 142,
  "stdout": "raw output here",
  "stderr": "logs here",
  "data": { /* parsed result based on output_format */ }
}
```

## Best Practices

✅ **DO**
- Use `text` for simple logging/messages
- Use `json` for structured single results
- Use `jsonl` for lists and batches
- Write one JSON object per line (no pretty-print)
- Log to stderr, output to stdout
- Use non-zero exit codes for failures

❌ **DON'T**
- Mix error messages in stdout (use stderr)
- Pretty-print JSON across multiple lines
- Assume parsing will always succeed
- Use `jsonl` without `type: array` in schema

## Troubleshooting

**No result parsed?**
- Check exit code is 0
- Verify JSON is on last line (`json`)
- Ensure one JSON per line (`jsonl`)
- Check for syntax errors in output
- Parsing failures don't cause execution failure

**JSONL returning empty array?**
- Check each line is valid JSON
- Ensure no trailing empty lines
- Invalid lines are silently skipped

**Result is null but expected data?**
- Verify `output_format` matches output
- Check stdout contains expected format
- Parsing is best-effort (no errors thrown)

## Database

```sql
-- Check action output format
SELECT ref, output_format FROM action WHERE ref = 'example.http_request';

-- Update action output format
UPDATE action SET output_format = 'jsonl' WHERE ref = 'mypack.myaction';
```

## See Also

- [Full Documentation](action-output-formats.md)
- [Pack Structure](pack-structure.md)
- [Parameter Delivery](parameter-delivery.md)
- [Execution System](execution-system.md)