"""No secret output or provider credentials. Native I/O belongs to the controller."""
import json
import sys


def main():
    raw = sys.stdin.buffer.read(16385)
    if len(raw) > 16384:
        return 2
    try:
        value = json.loads(raw)
        if set(value) != {"native_source"} or not isinstance(value["native_source"], str):
            return 2
    except (ValueError, TypeError):
        return 2
    print('{"accepted":true}')
    return 0


if __name__ == "__main__":
    sys.exit(main())
