#!/usr/bin/env bash
# Delete legacy Attune Linux package versions from the Gitea package registry.
#
# The initial branch package publishing used versions like "sha-abc123", which
# Debian apt ranks above corrected numeric versions but dpkg refuses to install.
# This script discovers those legacy sha-* versions from Debian, RPM, and Arch
# package indexes and removes them from all three registries.

set -euo pipefail

GITEA_BASE_URL="${GITEA_BASE_URL:-https://git.rdrx.app}"
PACKAGE_NAMESPACE="${PACKAGE_NAMESPACE:-attune-system}"
DEBIAN_DISTRIBUTION="${DEBIAN_DISTRIBUTION:-stable}"
DEBIAN_COMPONENT="${DEBIAN_COMPONENT:-main}"
RPM_GROUP="${RPM_GROUP:-el9}"
ARCH_REPOSITORY="${ARCH_REPOSITORY:-core}"

PACKAGES=(
  attune-agent
  attune-api
  attune-cli
  attune-executor
  attune-notifier
  attune-supervisor
)
DEBIAN_ARCHES=(amd64 arm64)
RPM_ARCHES=(x86_64 aarch64)
ARCH_ARCHES=(x86_64 aarch64)

DRY_RUN=true
DISCOVER=true
VERSIONS=()

usage() {
  cat <<'USAGE'
Delete legacy sha-* Attune Linux package versions from Gitea.

Defaults:
  host/owner:   https://git.rdrx.app / attune-system
  Debian repo:  stable main
  RPM group:    el9
  Arch repo:    core

Authentication:
  Set GITEA_USERNAME and GITEA_TOKEN (or GITEA_PASSWORD). The script prompts
  when either value is missing. The token/user must be allowed to delete
  packages in the namespace.

Usage:
  scripts/delete-legacy-gitea-linux-packages.sh [options]

Options:
  --execute            Actually delete packages. Default is dry-run.
  --dry-run            Print deletions without sending DELETE requests.
  --version VERSION    Delete this version. Can be repeated. Disables discovery.
  --host URL           Gitea base URL. Default: https://git.rdrx.app
  --namespace OWNER    Package owner/namespace. Default: attune-system
  --help               Show this help.

Examples:
  # Preview discovered legacy sha-* versions
  scripts/delete-legacy-gitea-linux-packages.sh

  # Delete all discovered legacy sha-* versions
  GITEA_USERNAME=david GITEA_TOKEN=... \
    scripts/delete-legacy-gitea-linux-packages.sh --execute

  # Delete explicit versions
  scripts/delete-legacy-gitea-linux-packages.sh --execute \
    --version sha-f04dcd5249a2 \
    --version sha-f0b31805995a
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --execute)
      DRY_RUN=false
      shift
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --version)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--version requires a value" >&2
        exit 2
      fi
      DISCOVER=false
      VERSIONS+=("$2")
      shift 2
      ;;
    --host)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--host requires a value" >&2
        exit 2
      fi
      GITEA_BASE_URL="${2%/}"
      shift 2
      ;;
    --namespace)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--namespace requires a value" >&2
        exit 2
      fi
      PACKAGE_NAMESPACE="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

GITEA_BASE_URL="${GITEA_BASE_URL%/}"
GITEA_USERNAME="${GITEA_USERNAME:-${CONTAINER_REGISTRY_USERNAME:-}}"
GITEA_TOKEN="${GITEA_TOKEN:-${GITEA_PASSWORD:-${CONTAINER_REGISTRY_PASSWORD:-}}}"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required command not found: $1" >&2
    exit 1
  fi
}

require_command curl
require_command python3

sanitize_url_origin() {
  local url=$1 scheme authority
  case "$url" in
    http://*) scheme=http; authority=${url#http://} ;;
    https://*) scheme=https; authority=${url#https://} ;;
    *) printf '%s\n' '<url configured>'; return ;;
  esac
  authority=${authority%%/*}
  authority=${authority%%\?*}
  authority=${authority%%\#*}
  case "$authority" in
    *@*@*|'') printf '%s\n' '<url configured>'; return ;;
    *@*) authority=${authority#*@} ;;
  esac
  case "$authority" in
    ''|*[[:space:]]*|*\\*) printf '%s\n' '<url configured>'; return ;;
  esac
  case "$authority" in
    \[*\])
      local display_host=${authority#\[}; display_host=${display_host%\]}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    \[*\]:*)
      local display_host=${authority#\[} display_port
      display_port=${display_host#*\]}; display_host=${display_host%%\]*}; display_port=${display_port#:}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *:*)
      local display_host=${authority%:*} display_port=${authority##*:}
      case "$display_host" in ''|*:*|*[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *) case "$authority" in *[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac ;;
  esac
  printf '%s://%s\n' "$scheme" "$authority"
}

GITEA_URL_ORIGIN=$(sanitize_url_origin "$GITEA_BASE_URL")

prompt_for_credentials() {
  if [[ -z "$GITEA_USERNAME" ]]; then
    read -r -p "Gitea username: " GITEA_USERNAME
  fi
  if [[ -z "$GITEA_TOKEN" ]]; then
    read -r -s -p "Gitea token/password: " GITEA_TOKEN
    echo
  fi
}

discover_versions() {
  local tmp
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' RETURN

  discover_debian_versions >>"$tmp"
  discover_rpm_versions >>"$tmp"
  discover_arch_versions >>"$tmp"

  python3 - "$tmp" <<'PY'
import re
import sys
from pathlib import Path

versions = []
for line in Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace").splitlines():
    line = line.strip()
    if re.match(r"^sha-[A-Za-z0-9.+~:-]+$", line):
        versions.append(line)
        if not re.search(r"-[0-9]+$", line):
            versions.append(f"{line}-1")

for version in sorted(set(versions)):
    print(version)
PY
}

discover_debian_versions() {
  local tmp
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' RETURN

  for arch in "${DEBIAN_ARCHES[@]}"; do
    local packages_url="${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/debian/dists/${DEBIAN_DISTRIBUTION}/${DEBIAN_COMPONENT}/binary-${arch}/Packages"
    echo "Discovering Debian legacy versions from $(sanitize_url_origin "$packages_url")" >&2
    if ! curl -fsSL "$packages_url" >>"$tmp"; then
      echo "Warning: failed to fetch Debian package index for ${arch}; continuing" >&2
    fi
    printf '\n' >>"$tmp"
  done

  python3 - "$tmp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
versions = sorted(set(re.findall(r"^Version: (sha-[A-Za-z0-9.+~:-]+)$", text, re.MULTILINE)))
for version in versions:
    print(version)
PY
}

discover_rpm_versions() {
  local repomd primary_href primary_url tmp
  repomd="$(mktemp)"
  tmp="$(mktemp)"
  trap 'rm -f "$repomd" "$tmp"' RETURN

  local repomd_url="${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/rpm/${RPM_GROUP}/repodata/repomd.xml"
  echo "Discovering RPM legacy versions from $(sanitize_url_origin "$repomd_url")" >&2
  if ! curl -fsSL "$repomd_url" -o "$repomd"; then
    echo "Warning: failed to fetch RPM repomd.xml; continuing" >&2
    return 0
  fi

  primary_href="$(python3 - "$repomd" <<'PY'
import sys
import xml.etree.ElementTree as ET

ns = {"repo": "http://linux.duke.edu/metadata/repo"}
root = ET.parse(sys.argv[1]).getroot()
for data in root.findall("repo:data", ns):
    if data.attrib.get("type") == "primary":
        location = data.find("repo:location", ns)
        if location is not None:
            print(location.attrib["href"])
        break
PY
)"
  if [[ -z "$primary_href" ]]; then
    echo "Warning: RPM primary metadata location not found; continuing" >&2
    return 0
  fi

  primary_url="${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/rpm/${RPM_GROUP}/${primary_href}"
  if ! curl -fsSL "$primary_url" | gzip -dc >"$tmp"; then
    echo "Warning: failed to fetch RPM primary metadata; continuing" >&2
    return 0
  fi

  python3 - "$tmp" <<'PY'
import sys
import xml.etree.ElementTree as ET

ns = {"common": "http://linux.duke.edu/metadata/common"}
root = ET.parse(sys.argv[1]).getroot()
versions = set()
for package in root.findall("common:package", ns):
    version = package.find("common:version", ns)
    if version is None:
        continue
    ver = version.attrib.get("ver", "")
    rel = version.attrib.get("rel", "")
    if ver.startswith("sha-"):
        versions.add(ver)
        if rel:
            versions.add(f"{ver}-{rel}")
for version in sorted(versions):
    print(version)
PY
}

discover_arch_versions() {
  local arch archive
  for arch in "${ARCH_ARCHES[@]}"; do
    archive="$(mktemp)"
    trap 'rm -f "$archive"' RETURN
    local db_url="${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/arch/${ARCH_REPOSITORY}/${arch}/${ARCH_REPOSITORY}.db.tar.gz"
    echo "Discovering Arch legacy versions from $(sanitize_url_origin "$db_url")" >&2
    if ! curl -fsSL "$db_url" -o "$archive"; then
      echo "Warning: failed to fetch Arch database for ${arch}; continuing" >&2
      continue
    fi
    python3 - "$archive" <<'PY'
import sys
import tarfile

versions = set()
with tarfile.open(sys.argv[1], "r:gz") as archive:
    for member in archive.getmembers():
        if not member.isfile() or not member.name.endswith("/desc"):
            continue
        extracted = archive.extractfile(member)
        if extracted is None:
            continue
        lines = extracted.read().decode("utf-8", errors="replace").splitlines()
        for index, line in enumerate(lines[:-1]):
            if line == "%VERSION%" and lines[index + 1].startswith("sha-"):
                version = lines[index + 1]
                versions.add(version)
                if "-" in version:
                    versions.add(version.rsplit("-", 1)[0])
for version in sorted(versions):
    print(version)
PY
    rm -f "$archive"
  done
}

delete_url() {
  local label="$1"
  local url="$2"

  if [[ "$DRY_RUN" == true ]]; then
    echo "DRY-RUN ${label}: DELETE $(sanitize_url_origin "$url")"
    return 0
  fi

  local headers_file request_id response_file status
  headers_file="$(mktemp)"
  response_file="$(mktemp)"
  if ! status="$(curl -sS -D "$headers_file" -o "$response_file" -w '%{http_code}' \
    -u "${GITEA_USERNAME}:${GITEA_TOKEN}" \
    -X DELETE \
    "$url")"; then
    echo "Failed ${label} (request failed before an HTTP response)" >&2
    rm -f "$headers_file" "$response_file"
    return 1
  fi
  request_id="$(grep -Ei -m1 '^(x-attune-request-id|x-request-id|request-id):' "$headers_file" \
    | cut -d: -f2- | tr -d '\r' | xargs || true)"

  case "$status" in
    204)
      echo "Deleted ${label}"
      ;;
    404)
      echo "Not found ${label}"
      ;;
    *)
      echo "Failed ${label} (HTTP ${status})" >&2
      if [[ "$request_id" =~ ^[A-Za-z0-9._:-]{1,128}$ ]]; then
        echo "Request ID: ${request_id}" >&2
      fi
      echo "Response body omitted" >&2
      rm -f "$headers_file" "$response_file"
      return 1
      ;;
  esac

  rm -f "$headers_file" "$response_file"
}

delete_version() {
  local package="$1"
  local version="$2"

  if [[ ! "$version" =~ -[0-9]+$ ]]; then
    for arch in "${DEBIAN_ARCHES[@]}"; do
      delete_url \
        "debian/${package}/${version}/${arch}" \
        "${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/debian/pool/${DEBIAN_DISTRIBUTION}/${DEBIAN_COMPONENT}/${package}/${version}/${arch}"
    done
  fi

  for arch in "${RPM_ARCHES[@]}"; do
    delete_url \
      "rpm/${package}/${version}/${arch}" \
      "${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/rpm/${RPM_GROUP}/package/${package}/${version}/${arch}"
  done

  for arch in "${ARCH_ARCHES[@]}"; do
    delete_url \
      "arch/${package}/${version}/${arch}" \
      "${GITEA_BASE_URL}/api/packages/${PACKAGE_NAMESPACE}/arch/${ARCH_REPOSITORY}/${package}/${version}/${arch}"
  done
}

main() {
  if [[ "$DISCOVER" == true ]]; then
    mapfile -t VERSIONS < <(discover_versions)
  fi

  if [[ "${#VERSIONS[@]}" -eq 0 ]]; then
    echo "No legacy sha-* versions found."
    exit 0
  fi

  echo "Gitea origin: ${GITEA_URL_ORIGIN}"
  echo "Namespace: ${PACKAGE_NAMESPACE}"
  echo "Versions: ${VERSIONS[*]}"
  if [[ "$DRY_RUN" == true ]]; then
    echo "Mode: dry-run (pass --execute to delete)"
  else
    echo "Mode: execute"
    prompt_for_credentials
  fi

  for package in "${PACKAGES[@]}"; do
    for version in "${VERSIONS[@]}"; do
      delete_version "$package" "$version"
    done
  done
}

main "$@"
