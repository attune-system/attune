#!/usr/bin/env bash
set -euo pipefail

# Read git diff --name-only output from stdin.
rust=false
web=false
release=false
smoke=false
while IFS= read -r path; do
  case "$path" in
    crates/*|migrations/*|*.rs|Cargo.toml|Cargo.lock|Makefile|.cargo/*|.github/workflows/ci.yml|scripts/*|docker/*|docker-compose*.yaml|docker-compose*.yml|config.*|packs/*|tests/fixtures/*)
      rust=true ;;
  esac
  case "$path" in
    web/*|.github/workflows/ci.yml) web=true ;;
  esac
  case "$path" in
    .dockerignore|.github/workflows/*|docker/*|migrations/*|packaging/*|scripts/*|Makefile|config.*|docker-compose*.yaml|docker-compose*.yml)
      release=true ;;
  esac
  case "$path" in
    crates/*|migrations/*|Cargo.toml|Cargo.lock|Makefile|.cargo/*|.dockerignore|.github/workflows/ci.yml|scripts/*|docker/*|docker-compose*.yaml|docker-compose*.yml|config.*|packs/*|tests/*)
      smoke=true ;;
  esac
done
printf 'rust=%s\nweb=%s\nrelease=%s\nsmoke=%s\n' "$rust" "$web" "$release" "$smoke"
