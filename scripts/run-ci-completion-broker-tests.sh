#!/usr/bin/env bash
set -euo pipefail

: "${ATTUNE_TEST_RABBITMQ_URL:?Set the owned RabbitMQ URL for the consumer and executor tests}"
: "${ATTUNE_MANAGER_TEST_AMQP_URL:?Set a separate owned RabbitMQ vhost URL for the API test}"

results=$(mktemp)
trap 'rm -f "$results"' EXIT

run_broker_test() {
    cargo test "$@" -- --ignored --exact --nocapture --test-threads=4 | tee "$results"
    if ! grep -q '^test result: ok\. 1 passed;' "$results"; then
        printf 'Expected exactly one passing broker test for %s\n' "${*: -1}" >&2
        return 1
    fi
}

run_broker_test -p attune-common --lib mq::consumer::tests::keyed_consumer_orders_related_deliveries_and_recovers_unsettled_work
run_broker_test -p attune-executor --lib service::tests::test_service_drains_completion_before_closing_resources
run_broker_test -p attune-api --test manager_session_tests work::completion_survives_broker_return_and_reuses_one_event_id
