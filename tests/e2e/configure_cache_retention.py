"""Configure short cache lifecycle windows for the ephemeral E2E stack."""

from helpers import AttuneClient


def main() -> None:
    client = AttuneClient()
    try:
        response = client._request("GET", "/api/v1/retention-config")
        if response.status_code != 200:
            raise RuntimeError(f"Failed to read retention config: {response.text}")

        config = response.json()["data"]
        config["check_interval_seconds"] = 2
        cache = config["cache_retention"]
        cache["min_traversal_window_seconds"] = 5
        cache["staging_expiry_seconds"] = 5
        cache["dry_run"] = False
        cache["freshness_alerts_enabled"] = False

        response = client._request(
            "PUT", "/api/v1/retention-config", json=config
        )
        if response.status_code != 200:
            raise RuntimeError(f"Failed to update retention config: {response.text}")
    finally:
        client.logout()


if __name__ == "__main__":
    main()
