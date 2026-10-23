"""Pure protocol, mock and source checks. No DB, native process or provider work."""
import copy
from pathlib import Path
import unittest
from unittest.mock import Mock

from controller import Controller, catalogue_items, current_inventory
from protocol import Quarantine, canonical, decode, digest, member, relative

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OP = "00000000-0000-4000-8000-000000000001"
ENTRY = {"operation": OP, "version": 1, "root_read": 2, "object_key": "artifacts/1/body",
         "provider_version": None, "staging_path": None, "execution_attempt": OP,
         "worker_instance": OP, "native_instance": OP}
PLAN = {"id": OP, "scope": {"backend": "a" * 64, "staging_prefix": ".attune-upload/"},
        "members": [ENTRY]}
IDENTITY = {"device": "10", "inode": "20", "ctime_ns": "30", "mtime_ns": "40",
            "size": "50", "links": "1"}


def physical(key, root="blob", identity=None):
    return {"root": root, "key": key, "identity": identity or copy.deepcopy(IDENTITY)}


class ProtocolTests(unittest.TestCase):
    def test_jcs_subset_sorts_and_preserves_utf8(self):
        self.assertEqual(canonical({"z": 2, "a": "\u00e9"}), b'{"a":"\xc3\xa9","z":2}')
        self.assertEqual(digest({"z": 2, "a": 1}), digest({"a": 1, "z": 2}))

    def test_duplicate_json_keys_rejected(self):
        with self.assertRaisesRegex(Quarantine, "DuplicateJsonKey"):
            decode(b'{"operation":1,"operation":2}')

    def test_unsafe_numbers_rejected_not_silently_rounded(self):
        for value in (1.2, 2 ** 63, float("inf")):
            with self.assertRaises(Quarantine):
                canonical(value)

    def test_every_original_writer_identity_required(self):
        for key in ("native_instance", "worker_instance", "execution_attempt"):
            entry = dict(ENTRY, **{key: None})
            with self.assertRaisesRegex(Quarantine, "UntrackedWriterIdentity"):
                member(entry)

    def test_paths_never_escape_or_alias(self):
        for value in ("/tmp/file", "../file", "x/../y", "x//y", "a\\b", "a:b", "a/./b"):
            with self.assertRaises(Quarantine):
                relative(value)

    def test_real_local_multipart_child_keeps_exact_physical_identity(self):
        items = catalogue_items(PLAN, [physical(".attune-upload/123#1")], OP)
        self.assertEqual(items[0]["kind"], "multipart")
        self.assertEqual(items[0]["key"], ".attune-upload/123")
        self.assertEqual(items[0]["ownership_proof"]["physical_key"], ".attune-upload/123#1")
        self.assertEqual(items[0]["ownership_proof"]["identity"], IDENTITY)

    def test_local_final_copy_temporary_has_exact_member_key(self):
        item = catalogue_items(PLAN, [physical("artifacts/1/body#2")], OP)[0]
        self.assertEqual(item["key"], ENTRY["object_key"])
        self.assertEqual(item["kind"], "final_object")

    def test_unknown_nonmember_object_stays_quarantined(self):
        with self.assertRaisesRegex(Quarantine, "UnknownBlobPathOwner"):
            catalogue_items(PLAN, [physical("other/body#1")], OP)

    def test_unknown_shared_path_is_not_adopted(self):
        with self.assertRaisesRegex(Quarantine, "UnknownSharedPathOwner"):
            catalogue_items(PLAN, [physical("staging/file", "shared")], OP)

    def test_absence_requires_matching_durable_delete_intent(self):
        items = catalogue_items(PLAN, [physical(ENTRY["object_key"])], OP)
        with self.assertRaisesRegex(Quarantine, "UnconfirmedPhysicalAbsence"):
            current_inventory(items, [], lambda _: False)
        self.assertEqual(current_inventory(items, [], lambda _: True), {})

    def test_new_physical_child_cannot_hide_under_a_saved_receipt(self):
        with self.assertRaisesRegex(Quarantine, "UncataloguedPhysicalChild"):
            current_inventory([], [physical(".attune-upload/456")], lambda _: True)

    def test_inode_replacement_and_content_mutation_are_rejected(self):
        items = catalogue_items(PLAN, [physical(ENTRY["object_key"])], OP)
        for field in ("inode", "ctime_ns", "mtime_ns", "size", "links"):
            changed = dict(IDENTITY, **{field: "900"})
            with self.assertRaisesRegex(Quarantine, "PhysicalIdentityChanged"):
                current_inventory(items, [physical(ENTRY["object_key"], identity=changed)], lambda _: True)

    def test_internal_hardlink_cleanup_only_accounts_for_our_missing_link(self):
        linked = dict(IDENTITY, links="2")
        entries = [physical(ENTRY["object_key"], identity=linked),
                   physical(".attune-upload/123", identity=linked)]
        items = catalogue_items(PLAN, entries, OP)
        remaining = [physical(ENTRY["object_key"], identity=dict(linked, links="1", ctime_ns="31"))]
        current_inventory(items, remaining,
                          lambda item: item["ownership_proof"]["physical_key"] == ".attune-upload/123")
        with self.assertRaises(Quarantine):
            current_inventory(items, remaining, lambda _: False)


class ControllerMockTests(unittest.TestCase):
    def setUp(self):
        self.journal, self.native = Mock(), Mock()
        self.native.binding.return_value = {"roots": {}, "cgroup": {}}
        self.journal.binding.return_value = self.native.binding.return_value
        self.journal.audit.return_value = (None, None)
        self.native.scan.return_value = []
        self.controller = Controller({"scope": PLAN["scope"], "generation": OP},
                                     self.journal, self.native)

    def test_drain_failure_never_records_settlement(self):
        self.native.stop_and_drain.side_effect = Quarantine("ProviderDrainPending")
        with self.assertRaises(Quarantine):
            self.controller.stop(PLAN)
        self.journal.fence.assert_called_once_with(PLAN)
        self.journal.settled.assert_not_called()

    def test_stop_calls_journal_before_native_drain(self):
        calls = []
        self.journal.fence.side_effect = lambda _: calls.append("fence")
        self.native.stop_and_drain.side_effect = lambda _: calls.append("drain")
        self.journal.settled.side_effect = lambda *_: calls.append("settled")
        result = self.controller.stop(PLAN)
        self.assertEqual(calls, ["fence", "drain", "settled"])
        self.assertEqual(result["stopped_execution_attempts"], [OP])
        self.assertEqual(result["restart_fence"]["frozen"], 1)

    def test_lost_kernel_fence_not_replaced_by_stored_json(self):
        self.journal.audit.return_value = ({"saved": "receipt"}, [])
        self.native.require_frozen.side_effect = Quarantine("RestartFenceLost")
        with self.assertRaisesRegex(Quarantine, "RestartFenceLost"):
            self.controller.stop(PLAN)
        self.native.stop_and_drain.assert_not_called()

    def test_changed_native_generation_cannot_recover(self):
        self.native.binding.return_value = {"different": "volume"}
        with self.assertRaisesRegex(Quarantine, "NativeGenerationChanged"):
            self.controller.stop(PLAN)
        self.journal.fence.assert_not_called()

    def test_unknown_principal_cannot_use_writer(self):
        with self.assertRaisesRegex(Quarantine, "WriterPrincipalRequired"):
            self.controller.write({}, {"role": "manager"})

    def writer_request(self):
        return {"member": copy.deepcopy(ENTRY), "mode": "blob", "payload_base64": "",
                "pause_after_first_chunk": True}

    def writer_principal(self):
        return {"role": "writer", **{key: ENTRY[key] for key in
                ("native_instance", "worker_instance", "execution_attempt")}}

    def test_duplicate_accepted_operation_does_not_spawn_again(self):
        self.journal.admit.return_value = False
        result = self.controller.write(self.writer_request(), self.writer_principal())
        self.assertFalse(result["restarted"])
        self.native.launch.assert_not_called()

    def test_forged_attempt_cannot_reach_durable_admission(self):
        principal = self.writer_principal()
        principal["execution_attempt"] = "00000000-0000-4000-8000-000000000002"
        with self.assertRaisesRegex(Quarantine, "WriterIdentityMismatch"):
            self.controller.write(self.writer_request(), principal)
        self.journal.admit.assert_not_called()

    def test_write_admission_precedes_actual_native_launch(self):
        calls = []
        self.journal.admit.side_effect = lambda *_: calls.append("admit") or True
        self.native.launch.side_effect = lambda *_: calls.append("exec")
        self.controller.write(self.writer_request(), self.writer_principal())
        self.assertEqual(calls, ["admit", "exec"])

    def test_caller_cannot_choose_executable_or_root(self):
        body = dict(self.writer_request(), root="/other/provider")
        with self.assertRaisesRegex(Quarantine, "InvalidWrite"):
            self.controller.write(body, self.writer_principal())

    def test_combined_original_member_keeps_both_owned_destinations(self):
        body = self.writer_request()
        body["mode"] = "blob_and_shared"
        body["member"]["staging_path"] = "uploads/original.stage"
        self.journal.admit.return_value = True
        self.controller.write(body, self.writer_principal())
        self.assertEqual(self.journal.admit.call_args.args[0], body["member"])
        self.assertEqual(self.native.launch.call_args.args[:2],
                         (body["member"], "blob_and_shared"))
        items = catalogue_items(dict(PLAN, members=[body["member"]]),
                                [physical(ENTRY["object_key"]),
                                 physical("uploads/original.stage", "shared",
                                          dict(IDENTITY, inode="21"))], OP)
        self.assertEqual({item["kind"] for item in items}, {"local_staging", "final_object"})

    def test_combined_member_cannot_silently_skip_the_staging_write(self):
        body = self.writer_request()
        body["member"]["staging_path"] = "uploads/original.stage"
        with self.assertRaisesRegex(Quarantine, "UnlocatedWriteMode"):
            self.controller.write(body, self.writer_principal())
        self.journal.admit.assert_not_called()

    def test_unverified_fence_echo_cannot_reach_native_cleanup(self):
        self.journal.audit.return_value = ({"saved": "original"}, [])
        with self.assertRaisesRegex(Quarantine, "FenceReceiptMismatch"):
            self.controller.delete(PLAN, {"fence": {"saved": "replacement"}, "item": {}})
        self.journal.delete_intent.assert_not_called()
        self.native.remove.assert_not_called()

    def test_empty_catalogue_still_requires_native_completion(self):
        self.journal.audit.return_value = ({"saved": "original"}, [])
        result = self.controller.catalogue(PLAN, {"fence": {"saved": "original"},
                                           "cursor": None, "limit": 1, "max_bytes": 4096})
        self.native.require_frozen.assert_called_once()
        self.native.stop_and_drain.assert_called_once()
        self.assertEqual(result["complete"]["proof"]["count"], 0)
        self.assertEqual(result["complete"]["fence_digest"], digest({"saved": "original"}))


class ImplementationSourceTests(unittest.TestCase):
    def test_native_barriers_are_kernel_and_provider_operations(self):
        source = (HERE / "native.py").read_text()
        stop = source.split("def stop_and_drain(", 1)[1].split("def require_frozen(", 1)[0]
        self.assertLess(stop.index('"cgroup.freeze"'), stop.index('"cgroup.kill"'))
        self.assertLess(stop.index('"populated 0"'), stop.index("child.wait("))
        self.assertLess(stop.index("child.wait("), stop.index("LIBC.syncfs(fd)"))
        self.assertIn("self.require_frozen()", stop)
        self.assertIn("WriterJoinPending", stop)
        self.assertIn("PR_SET", (ROOT / "docs/permissions/native-legacy-filesystem-controller.md").read_text())

    def test_accepted_write_has_no_reexecution_recovery(self):
        source = (HERE / "journal.py").read_text()
        admit = source.split("def admit(", 1)[1].split("def joined(", 1)[0]
        self.assertLess(admit.index("return False"), admit.index("GenerationFenced"))
        self.assertIn('PRAGMA synchronous=FULL', source)
        self.assertIn('BEGIN IMMEDIATE', source)
        self.assertIn('UPDATE generation SET fenced=1', source)
        self.assertNotIn('SET fenced=0', source)

    def test_serve_does_not_initialize_or_adopt_existing_storage(self):
        source = (HERE / "main.py").read_text()
        self.assertIn('UntrackedHistoricalRoot', source)
        self.assertIn('GenerationAlreadyInitialized', source)
        self.assertIn('JournalNotProvisioned', source)
        self.assertIn('ssl.CERT_REQUIRED', source)
        self.assertIn('ssl.TLSVersion.TLSv1_3', source)
        self.assertIn('ControlledAcceptanceGateDisabled', source)
        example = decode((HERE / "config.example.json").read_bytes())
        self.assertFalse(example["enabled_for_controlled_acceptance"])

    def test_empty_directory_does_not_prove_no_historical_open_descriptors(self):
        source = (HERE / "native.py").read_text()
        initialize = source.split("if initialize:", 1)[1].split("self.roots =", 1)[0]
        self.assertIn('not os.path.lexists(path)', initialize)
        self.assertIn('os.mkdir(path.name, 0o700, dir_fd=parent)', initialize)
        self.assertIn('os.fsync(parent)', initialize)
        self.assertLess(source.index('self.exclusive_processes()'), source.index('os.mkdir(path.name'))

    def test_actual_native_writer_uses_legacy_put_not_atomic_file_protocol(self):
        source = (ROOT / "crates/common/examples/legacy_native_writer.rs").read_text()
        self.assertIn('.put(&ObjectKey::new(request.key)?, body, expected)', source)
        self.assertNotIn('.put_file_atomic(', source)
        self.assertIn('resolve_checked_path(', source)
        self.assertIn('.create_new(true)', source)
        self.assertNotIn('println!', source)

    def test_remote_providers_are_not_inferred_from_local_process_exit(self):
        source = (HERE / "native.py").read_text()
        self.assertIn('fs_type(fd) in (0xEF53, 0x58465342)', source)
        self.assertIn('ProviderCannotAttestSettlement', source)
        self.assertIn('UncontrolledWriterProcess', source)
        self.assertIn('UnownedPhysicalAlias', source)
        self.assertIn('"boot": Path("/proc/sys/kernel/random/boot_id")', source)

    def test_signer_is_independent_and_no_inherited_child_environment(self):
        source = (HERE / "main.py").read_text()
        self.assertIn('"rsa_padding_mode:pkcs1"', source)
        self.assertIn('"request_digest": digest(request)', source)
        self.assertIn('"nonce": request["nonce"]', source)
        self.assertIn('env={}, timeout=2', source)
        self.assertIn('hashlib.sha256(certificate).hexdigest()', source)
        native = (HERE / "native.py").read_text()
        self.assertIn('env={}, close_fds=True', native)
        self.assertIn('LIBC.prctl(1, signal.SIGKILL', native)
        self.assertIn('LIBC.prctl(38, 1', native)


if __name__ == "__main__":
    unittest.main()
