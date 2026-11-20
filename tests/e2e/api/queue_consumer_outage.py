"""Explicitly selected by the owned runner after it stops its notifier."""

from .test_worker_native_completion import _publish_single_item_queue


def test_missing_notifier_rejects_without_accepting(client, tmp_path):
    ref = _publish_single_item_queue(client, tmp_path)
    base = f"/api/v1/queues/{ref}.work/items"
    item = {"item_key": "readiness-blocked", "payload": {}}
    for target, request in ((base, item), (base + "/bulk", {"items": [item]})):
        response = client.post(target, json=request)
        assert response.status_code == 501
        assert "requires coordinated runtime readiness" in response.text

    listing = client.get(base)
    listing.raise_for_status()
    assert listing.json()["pagination"]["total_items"] == 0
    assert client.list_executions(action_ref=ref + ".ack", limit=20) == []
