"""Source checks ONLY. Never imports live_cases or runs SQLite/kernel/app work."""
import ast
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


class LiveLaneSourceTests(unittest.TestCase):
    def setUp(self):
        self.source = (HERE / "live_cases.py").read_text()
        self.tree = ast.parse(self.source)
        self.functions = {node.name: ast.get_source_segment(self.source, node)
                          for node in self.tree.body if isinstance(node, ast.FunctionDef)}

    def test_all_seven_actual_cases_exist(self):
        for name in ("paused_combined", "same_boot_restart", "lost_delete_reply", "untracked_pid",
                     "mount_replacement", "external_hardlink", "cert_role_attempts"):
            self.assertIn("case_" + name, self.functions)
        self.assertNotIn("unittest.mock", self.source)
        self.assertNotIn("Mock(", self.source)

    def test_live_gates_precede_sqlite_or_process_actions(self):
        main = self.functions["main"]
        self.assertLess(main.index('require(args.execute_owned_live'), main.index('run_journal_cases(root)'))
        self.assertIn('suite.countTestCases() == 9', self.functions["run_journal_cases"])
        self.assertIn('result.testsRun == 9', self.functions["run_journal_cases"])
        self.assertIn('ssl.TLSVersion.TLSv1_3', self.source)
        self.assertIn('LocalNativeProviderUnavailable', self.source)
        self.assertIn('CgroupDelegationUnavailable', self.source)
        self.assertIn('"acceptance_complete": False', self.source)
        self.assertIn('"cloud_acceptance": False', self.source)

    def test_real_cgroup_namespace_inode_and_file_handles_are_observed(self):
        for text in ('"frozen 1"', '"populated 0"', '"populated 1"',
                     '"/proc/sys/kernel/random/boot_id"', '"/proc/self/ns/pid"',
                     'f"/proc/{pid}/fd"', 'LIBC.syncfs(fd) == 0', 'root_identity(self.cgroup)',
                     '"NativeWriterNotReaped"'):
            self.assertIn(text, self.source)
        self.assertIn('user=self.uid, group=self.gid', self.source)
        self.assertIn('self.daemon.wait(timeout=10)', self.source)

    def test_same_boot_crash_does_not_release_or_reexecute(self):
        case = self.functions["case_same_boot_restart"]
        self.assertLess(case.index('lane.stop()'), case.index('lane.retained()'))
        self.assertLess(case.index('lane.retained()'), case.index('lane.start()'))
        self.assertIn('AcceptedOperationReexecutedAfterCrash', case)
        self.assertIn('lane.finish()', case)

    def test_lost_receipt_has_real_delete_and_same_journal_restart(self):
        case = self.functions["case_lost_delete_reply"]
        self.assertIn('drop_reply=True', case)
        self.assertIn('wait_for(lambda: not target.exists()', case)
        self.assertLess(case.index('lane.retained()'), case.index('lane.stop()'))
        self.assertIn('ExactIntentRecoveryMismatch', case)
        self.assertIn('lane.finish()', case)

    def test_negative_faults_are_exact_owned_mutations_not_mocked_flags(self):
        self.assertIn('os.link(target, alias, follow_symlinks=False)', self.functions["case_external_hardlink"])
        mount = self.functions["case_mount_replacement"]
        self.assertIn('"/bin/mount", "--bind"', mount)
        self.assertIn('PrivateMountPropagationRequired', mount)
        self.assertIn('"/bin/umount"', mount)
        self.assertIn('process.wait(timeout=5)', self.functions["case_untracked_pid"])
        roles = self.functions["case_cert_role_attempts"]
        for diagnostic in ('WriterPrincipalRequired', 'UnregisteredDeploymentPrincipal',
                           'WriterIdentityMismatch', 'ManagerPrincipalRequired', 'InvalidWrite'):
            self.assertIn(diagnostic, roles)

    def test_real_app_api_and_repository_pins_are_mandatory(self):
        self.assertIn('method="DELETE"', self.source)
        self.assertIn('"/file-audits/" + self.audit + "/step"', self.source)
        self.assertIn('self.adapter("observe-original-native-custody", version_id=self.member_version)', self.source)
        self.assertIn('self.adapter("prepare-original-native-producer")', self.source)
        self.assertIn('"CallerMemberNotAccepted"', self.source)
        self.assertIn('value["pins"] > 0', self.source)
        self.assertIn('value["pins"] == value["operations"] == 0', self.source)
        self.assertIn('"ActualRepositoryPlanDiffersFromOriginal"', self.source)
        self.assertNotIn('sqlite3.connect', self.source)
        self.assertNotIn('SELECT ', self.source)

    def test_inspector_legacy_identity_has_no_policy_observer_or_secret_projection(self):
        source = (ROOT / "crates/common/src/repositories/storage_probe_inspection.rs").read_text()
        self.assertIn('LegacyEvidence {', source)
        self.assertIn('access.verify_legacy(scope, &identity)?', source)
        self.assertIn('"legacy-evidence"', source)
        self.assertTrue('state.root_read.is_some_and' in "".join(source.split()))
        self.assertIn('pub legacy_identity: Option<ObservedLegacyIdentity>', source)
        self.assertIn('legacy_evidence_identity_is_issued_before_metadata_gc_and_never_rebound', source)

    def test_transport_security_uses_fixed_role_pins_and_private_key_permissions(self):
        source = (HERE / "main.py").read_text()
        self.assertIn('BoundedInput(self.rfile, self.connection)', source)
        self.assertIn('private_material=field in ("tls_key", "signing_key")', source)
        self.assertIn('"PrivateKeyPermissionsRequired"', source)
        self.assertIn('"JsonContentTypeRequired"', source)
        self.assertIn('"http_input.py"', source)
        self.assertIn('"PrivateKeyGroupMismatch"', source)
        native = (HERE / "native.py").read_text()
        self.assertIn('"NativeCredentialGroupShared"', native)
        self.assertIn('"service_gid": os.getegid()', native)


if __name__ == "__main__":
    unittest.main()
