#!/usr/bin/env bash
# package-docker-dist.sh — Assemble a self-contained Docker Compose distribution bundle.
#
# The template directory (docker/distributable/) contains only the three
# distributable-specific files: docker-compose.yaml, config.docker.yaml, README.md.
# Everything else is copied from canonical source locations so there are no
# stale duplicates committed to the repo.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bundle_dir="${1:-${repo_root}/docker/distributable}"
archive_path="${2:-${repo_root}/artifacts/attune-docker-dist.tar.gz}"

template_dir="${repo_root}/docker/distributable"
bundle_dir="$(realpath -m "${bundle_dir}")"
archive_path="$(realpath -m "${archive_path}")"
template_dir="$(realpath -m "${template_dir}")"

mkdir -p "${bundle_dir}/docker" "${bundle_dir}/migrations" "${bundle_dir}/packs" "${bundle_dir}/scripts"
mkdir -p "$(dirname "${archive_path}")"

copy_file() {
    local src="$1"
    local dst="$2"
    mkdir -p "$(dirname "${dst}")"
    cp "${src}" "${dst}"
}

# Copy the distributable-specific templates (compose, config, README)
if [ "${bundle_dir}" != "${template_dir}" ]; then
    copy_file "${template_dir}/docker-compose.yaml" "${bundle_dir}/docker-compose.yaml"
    copy_file "${template_dir}/README.md" "${bundle_dir}/README.md"
    copy_file "${template_dir}/config.docker.yaml" "${bundle_dir}/config.docker.yaml"
fi

# Copy helper scripts from canonical docker/ and scripts/ directories
copy_file "${repo_root}/docker/run-migrations.sh" "${bundle_dir}/docker/run-migrations.sh"
copy_file "${repo_root}/docker/init-user.sh" "${bundle_dir}/docker/init-user.sh"
copy_file "${repo_root}/docker/init-packs.sh" "${bundle_dir}/docker/init-packs.sh"
copy_file "${repo_root}/docker/init-roles.sql" "${bundle_dir}/docker/init-roles.sql"
copy_file "${repo_root}/docker/nginx.conf" "${bundle_dir}/docker/nginx.conf"
copy_file "${repo_root}/docker/inject-env.sh" "${bundle_dir}/docker/inject-env.sh"
copy_file "${repo_root}/scripts/load_core_pack.py" "${bundle_dir}/scripts/load_core_pack.py"
copy_file "${repo_root}/scripts/seed-standard-pack-index.sh" "${bundle_dir}/scripts/seed-standard-pack-index.sh"

# Copy migrations and packs from canonical source directories
rm -rf "${bundle_dir}/migrations" "${bundle_dir}/packs/core"
mkdir -p "${bundle_dir}/migrations" "${bundle_dir}/packs"
cp -R "${repo_root}/migrations/." "${bundle_dir}/migrations/"
cp -R "${repo_root}/packs/core" "${bundle_dir}/packs/core"

source_date_epoch="${SOURCE_DATE_EPOCH:-$(git -C "${repo_root}" show -s --format=%ct HEAD)}"
python3 "${repo_root}/scripts/package-cli-archive.py" \
    "${archive_path}" "${source_date_epoch}" "${bundle_dir}" "$(basename "${bundle_dir}")"

echo "Docker dist bundle assembled at ${bundle_dir}"
echo "Docker dist archive created at ${archive_path}"
