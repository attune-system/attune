#!/usr/bin/env python3
"""Publish bundled core through ordinary API admission, never direct SQL loading."""

import argparse
import os
from pathlib import Path
from bootstrap_core_pack import publish_core


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Requires operator bootstrap DB_HOST/DB_PORT/DB_USER/DB_PASSWORD/DB_NAME/DB_SCHEMA environment. Example: ATTUNE_API_URL=http://localhost:8080 python3 scripts/load_core_pack.py --pack-dir ./packs --dry-run",
    )
    parser.add_argument("--pack-dir", type=Path, default=Path(os.environ.get("ATTUNE_PACKS_DIR", "./packs")))
    parser.add_argument("--pack-name", choices=("core",), default="core")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if args.dry_run:
        print(f"Would publish {args.pack_dir / args.pack_name} through API admission")
        return
    if not os.environ.get("ATTUNE_API_URL"):
        parser.error("ATTUNE_API_URL is required; direct database loading is no longer supported")
    publish_core(os.environ["ATTUNE_API_URL"].rstrip("/"), args.pack_dir / args.pack_name)


if __name__ == "__main__":
    main()
