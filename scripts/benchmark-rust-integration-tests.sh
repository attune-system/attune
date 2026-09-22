#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
output="${1:-/tmp/attune-rust-integration-benchmark.tsv}"
cold_samples="${ATTUNE_BENCHMARK_COLD_SAMPLES:-3}"
warm_samples="${ATTUNE_BENCHMARK_WARM_SAMPLES:-5}"
threads="${ATTUNE_BENCHMARK_THREADS:-4}"
target_crate="${ATTUNE_BENCHMARK_CRATE:-common}"
run_suffix="$(date -u +%Y%m%d%H%M%S)"
warm_run_id="bench-warm-${run_suffix}"
warm_project="attune-rust-bench-warm-${run_suffix}"
warm_test_run_id="bw${run_suffix:2:18}"

for value in "$cold_samples" "$warm_samples" "$threads"; do
  [[ "$value" =~ ^[0-9]+$ ]] || {
    echo "ERROR: sample counts and thread count must be non-negative integers" >&2
    exit 2
  }
done
((threads >= 4)) || {
  echo "ERROR: ATTUNE_BENCHMARK_THREADS must be at least four" >&2
  exit 2
}

mkdir -p "$(dirname "$output")"
printf 'sample\tmode\ttest_run_id\tthreads\tselected_tests\tselected_sha256\tinventory_tests\tinventory_sha256\tbuild_ms\tstartup_ms\ttest_ms\tcleanup_ms\ttotal_ms\texit_code\tpeak_sessions\tpre_clones\tpre_migrations\tpre_templates\tpre_sessions\tpre_migration_sessions\tpre_schemas\n' > "$output"

cleanup_warm_stack() {
  ATTUNE_E2E_RUN_ID="$warm_run_id" \
  ATTUNE_TEST_RUN_ID="$warm_test_run_id" \
    docker compose \
      --project-name "$warm_project" \
      -f "$project_root/docker-compose.yaml" \
      -f "$project_root/docker-compose.e2e.yaml" \
      down --remove-orphans --timeout 10 --volumes >/dev/null 2>&1
}
cleanup_on_exit() {
  local status=$?
  trap - EXIT
  if ! cleanup_warm_stack && ((status == 0)); then
    status=1
  fi
  exit "$status"
}
trap cleanup_on_exit EXIT

for ((sample = 1; sample <= cold_samples; sample++)); do
  run_id="bench-cold-${sample}-${run_suffix}"
  ATTUNE_E2E_RUN_ID="$run_id" \
  ATTUNE_E2E_PROJECT_NAME="attune-rust-${run_id}" \
  ATTUNE_TEST_RUN_ID="bc${sample}${run_suffix:4:16}" \
  ATTUNE_RUST_TEST_THREADS="$threads" \
  ATTUNE_BENCHMARK_OUTPUT="$output" \
  ATTUNE_BENCHMARK_SAMPLE="cold-${sample}" \
  ATTUNE_BENCHMARK_MODE="cold-stack" \
  BUILDKIT_PROGRESS=plain \
    bash "$project_root/scripts/run-rust-integration-tests.sh" --crate "$target_crate"
done

if ((warm_samples > 0)); then
  ATTUNE_E2E_RUN_ID="$warm_run_id" \
  ATTUNE_E2E_PROJECT_NAME="$warm_project" \
  ATTUNE_TEST_RUN_ID="$warm_test_run_id" \
  ATTUNE_RUST_TEST_THREADS="$threads" \
    bash "$project_root/scripts/run-rust-integration-tests.sh" \
      --no-teardown \
      --test test_database_lifecycle_tests \
      --filter explicit_cleanup_removes_owned_database

  for ((sample = 1; sample <= warm_samples; sample++)); do
    ATTUNE_E2E_RUN_ID="$warm_run_id" \
    ATTUNE_E2E_PROJECT_NAME="$warm_project" \
    ATTUNE_TEST_RUN_ID="$warm_test_run_id" \
    ATTUNE_RUST_TEST_THREADS="$threads" \
    ATTUNE_BENCHMARK_OUTPUT="$output" \
    ATTUNE_BENCHMARK_SAMPLE="warm-${sample}" \
    ATTUNE_BENCHMARK_MODE="warm-stack" \
      bash "$project_root/scripts/run-rust-integration-tests.sh" \
        --no-startup \
        --no-build \
        --no-teardown \
        --crate "$target_crate"
  done
fi

expected_samples=$((cold_samples + warm_samples))
observed_samples=0
expected_selected=""
expected_selected_sha256=""
expected_inventory_tests=""
expected_inventory_sha256=""
while IFS=$'\t' read -r sample mode test_run_id sample_threads selected selected_sha256 inventory_tests inventory_sha256 build_ms startup_ms test_ms cleanup_ms total_ms exit_code peak_sessions pre_clones pre_migrations pre_templates pre_sessions pre_migration_sessions pre_schemas; do
  [[ "$sample" != "sample" ]] || continue
  observed_samples=$((observed_samples + 1))
  for value in "$sample_threads" "$selected" "$inventory_tests" "$build_ms" "$startup_ms" "$test_ms" "$cleanup_ms" "$total_ms" "$exit_code" "$peak_sessions" "$pre_clones" "$pre_migrations" "$pre_templates" "$pre_sessions" "$pre_migration_sessions" "$pre_schemas"; do
    [[ "$value" =~ ^[0-9]+$ ]] || {
      echo "ERROR: benchmark sample $sample has invalid or missing numeric metadata" >&2
      exit 1
    }
  done
  [[ "$sample_threads" == "$threads" ]] || {
    echo "ERROR: benchmark sample $sample used $sample_threads threads, expected $threads" >&2
    exit 1
  }
  ((peak_sessions <= 16)) || {
    echo "ERROR: benchmark sample $sample exceeded the 16-session budget" >&2
    exit 1
  }
  [[ "$exit_code" == 0 ]] || {
    echo "ERROR: benchmark sample $sample failed with exit code $exit_code" >&2
    exit 1
  }
  [[ "$pre_clones" == 0 && "$pre_migrations" == 0 && "$pre_templates" == 1 && "$pre_sessions" == 0 && "$pre_migration_sessions" == 0 && "$pre_schemas" == 0 ]] || {
    echo "ERROR: benchmark sample $sample reported unexpected pre-teardown resources" >&2
    exit 1
  }
  if [[ -z "$expected_selected" ]]; then
    expected_selected="$selected"
    expected_selected_sha256="$selected_sha256"
    expected_inventory_tests="$inventory_tests"
    expected_inventory_sha256="$inventory_sha256"
  elif [[ "$selected" != "$expected_selected" || "$selected_sha256" != "$expected_selected_sha256" || "$inventory_tests" != "$expected_inventory_tests" || "$inventory_sha256" != "$expected_inventory_sha256" ]]; then
    echo "ERROR: benchmark selection changed at sample $sample" >&2
    exit 1
  fi
done < "$output"

[[ "$observed_samples" == "$expected_samples" ]] || {
  echo "ERROR: expected $expected_samples benchmark samples, found $observed_samples" >&2
  exit 1
}

cleanup_warm_stack
trap - EXIT
echo "Benchmark results: $output"
