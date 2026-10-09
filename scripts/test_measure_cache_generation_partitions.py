"""Pure regressions for the cold benchmark's Docker network ownership."""

import importlib.util
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


DRIVER = Path(os.environ.get("CACHE_MEASURE_TEST_DRIVER", str(
    Path(__file__).with_name("measure-cache-generation-partitions.py"))))
spec = importlib.util.spec_from_file_location("cache_measure", DRIVER)
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class ColdEndpointTests(unittest.TestCase):
    def test_database_restart_keeps_the_anchor_publication(self):
        args = ("docker", "run", "-d", "--name", "owned-db", "--label", "owner=run",
                "--publish", "127.0.0.1::5432", "--mount", "type=volume,src=owned,dst=/data",
                "postgres:16-alpine")
        actual = measure.anchored_command(args, "owned-db", "owned-network")
        self.assertEqual(actual, args[:7] + ("--network", "container:owned-network") + args[9:])
        self.assertEqual(measure.anchored_command(
            ("docker", "port", "owned-db", "5432"), "owned-db", "owned-network"),
            ("docker", "port", "owned-network", "5432"))
        restart = ("docker", "restart", "owned-db")
        self.assertEqual(measure.anchored_command(restart, "owned-db", "owned-network"), restart)

    def test_anchor_does_not_rewrite_other_resources_or_unexpected_publication(self):
        args = ("docker", "run", "--name", "other", "--publish", "127.0.0.1::5432", "postgres:16-alpine")
        self.assertEqual(measure.anchored_command(args, "owned-db", "owned-network"), args)
        with self.assertRaisesRegex(RuntimeError, "unexpected benchmark port"):
            measure.anchored_command(
                ("docker", "run", "--name", "owned-db", "--publish", "5432:5432", "postgres:16-alpine"),
                "owned-db", "owned-network")


class HeapCreationTests(unittest.TestCase):
    def test_heap_cleanup_preserves_native_bounded_admission_and_transport_shell(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-heap-budget-") as temporary:
            target = Path(temporary)
            files = [measure.REPO, measure.STORAGE] + [
                Path("migrations") / name for name in [measure.CACHE, measure.ACCOUNTING, measure.PARTITIONS]]
            for name in files:
                (target / name).parent.mkdir(parents=True, exist_ok=True)
                (target / name).write_bytes((measure.ROOT / name).read_bytes())
            native = measure.function((target / measure.REPO).read_text(), "drop_if_cleanup_eligible")
            measure.heap_baseline(target, Path("/tmp/opencode/native-workload-current-final/source"))
            heap_source = (target / measure.REPO).read_text()
            step = measure.function(heap_source, "cleanup_heap_step")
            # Only the operation body and its Error wrapper differ. Admission,
            # phase timers, lease cancellation, rollback and commit are identical.
            self.assertEqual(measure.heap_cleanup_shell(native, step), native)
            wrapper = measure.function(heap_source, "drop_if_cleanup_eligible")
            self.assertIn("bounded.max_cleanup_cycle_milliseconds = remaining;", wrapper)
            self.assertIn("Self::cleanup_heap_step(pool, generation_id, &bounded, finalizing)", wrapper)
            self.assertNotIn("pool.begin()", measure.function(heap_source, "delete_cleanup_batch"))
            self.assertNotIn("tx.commit()", measure.function(heap_source, "delete_if_empty"))
            with self.assertRaisesRegex(RuntimeError, "source freeze changed"):
                measure.heap_cleanup_shell(native, step.replace("finalizing: bool,", "finalizing: usize,"))

    def test_only_partition_attachment_lock_changes_for_heap_creation(self):
        source = (measure.ROOT / measure.REPO).read_text()
        native = measure.function(source, "protect_transaction")
        heap = measure.heap_creation_protection(native)
        self.assertEqual(heap, native.replace(
            '"LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE"',
            '"LOCK TABLE ONLY cache_entry IN ROW EXCLUSIVE MODE"', 1))
        self.assertIn("CacheTransactionMode::PinMutation", heap)
        self.assertIn("!matches!(mode, CacheTransactionMode::Read)", heap)
        self.assertIn("lock_cache_admission(connection).await?", heap)
        self.assertIn("config.ddl_lock_timeout_milliseconds", heap)
        self.assertIn("config.ddl_creation_statement_timeout_milliseconds", heap)

    def test_heap_lock_adapter_rejects_unknown_or_duplicate_ddl_locks(self):
        with self.assertRaisesRegex(RuntimeError, "source freeze changed"):
            measure.heap_creation_protection("no native attachment lock")
        with self.assertRaisesRegex(RuntimeError, "source freeze changed"):
            measure.heap_creation_protection(
                '"LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE"\n' * 2)

    def test_parallel_vacuum_shm_ceiling_is_verified_with_unchanged_total_memory(self):
        host = {"Memory":measure.PG_MEMORY_BYTES,"MemorySwap":measure.PG_MEMORY_BYTES,
                "NanoCpus":measure.PG_NANO_CPUS,"ShmSize":measure.PG_SHM_BYTES}
        actual = measure.effective_resource_caps({"HostConfig":host})
        self.assertEqual(actual["memory_bytes"], 1024**3)
        self.assertEqual(actual["shm_size_bytes"], 256 * 1024**2)
        host["ShmSize"] = 64 * 1024**2
        with self.assertRaisesRegex(RuntimeError, "did not provision"):
            measure.effective_resource_caps({"HostConfig":host})


class ArmBuildTests(unittest.TestCase):
    def test_each_arm_rebuilds_from_fresh_byte_identical_copy(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-arm-build-") as temporary:
            output = Path(temporary) / "evidence"
            target = Path(temporary) / "target"
            (target / "debug/examples").mkdir(parents=True)
            frozen_times = {}
            for arm in ["baseline", "treatment"]:
                file = output / arm / "crates/common/src/lib.rs"
                file.parent.mkdir(parents=True)
                file.write_text(arm + " source\n")
                os.utime(file, (1, 1))
                frozen_times[arm] = file.stat().st_mtime_ns
            built = []
            def build(command, *, cwd, **kwargs):
                arm = cwd.name.split(".")[0]
                self.assertEqual(cwd, output / (arm + ".build-source"))
                self.assertEqual(measure.fingerprint(cwd), measure.fingerprint(output / arm))
                self.assertGreater((cwd / "crates/common/src/lib.rs").stat().st_mtime_ns, frozen_times[arm])
                built.append(arm)
                (target / "debug/examples" / measure.EXAMPLE).write_text(arm + " executable\n")
                return SimpleNamespace(returncode=0, stdout="", stderr="Compiling attune-common v0.7.5\n")
            with patch.object(measure, "disk_storage"), patch.object(measure.subprocess, "run", side_effect=build):
                measure.compile_examples(output, target)
            self.assertEqual(built, ["treatment", "baseline"])
            for arm in built:
                self.assertEqual((output / (arm + ".bin")).read_text(), arm + " executable\n")
                self.assertEqual((output / arm / "crates/common/src/lib.rs").stat().st_mtime_ns, frozen_times[arm])

    def test_cached_common_library_is_not_accepted_as_an_arm_build(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-arm-cached-") as temporary:
            output = Path(temporary) / "evidence"
            (output / "treatment").mkdir(parents=True)
            (output / "treatment/lib.rs").write_text("treatment\n")
            result = SimpleNamespace(returncode=0, stdout="", stderr="Finished dev profile\n")
            with patch.object(measure, "disk_storage"), patch.object(measure.subprocess, "run", return_value=result):
                with self.assertRaisesRegex(RuntimeError, "did not compile its own"):
                    measure.compile_examples(output, Path(temporary) / "target")


if __name__ == "__main__":
    unittest.main(verbosity=2)
