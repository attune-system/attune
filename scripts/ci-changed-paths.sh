#!/usr/bin/env bash
set -euo pipefail

# Read git diff --name-only output from stdin.
rust=false
cargo_deny_inputs=false
web=false
web_image=false
release=false
smoke=false
while IFS= read -r path; do
  # Handle web-runtime and documentation paths before the shared Docker rules.
  case "$path" in
    docker/*.md) continue ;;
    docker/Dockerfile.web|docker/nginx.conf|docker/inject-env.sh)
      web_image=true
      continue ;;
    .dockerignore) web_image=true ;;
  esac
  case "$path" in
    Cargo.toml|*/Cargo.toml|Cargo.lock|*/Cargo.lock|deny.toml|.cargo/config|.cargo/config.toml)
      rust=true
      cargo_deny_inputs=true ;;
  esac
  case "$path" in
    crates/*|migrations/*|*.rs|Cargo.toml|Cargo.lock|Makefile|.cargo/*|.github/workflows/ci.yml|scripts/*|docker/*|docker-compose*.yaml|docker-compose*.yml|config.*|packs/*|tests/fixtures/*)
      rust=true ;;
  esac
  case "$path" in
    web/*|crates/api/*|crates/common/*|Cargo.toml|Cargo.lock|.github/workflows/ci.yml) web=true ;;
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
printf 'rust=%s\ncargo_deny_inputs=%s\nweb=%s\nweb_image=%s\nrelease=%s\nsmoke=%s\n' \
  "$rust" "$cargo_deny_inputs" "$web" "$web_image" "$release" "$smoke"
