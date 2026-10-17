"""Offline driver contract tests, not real-server acceptance evidence."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from fixture_support import FixtureError
import interoperability_probe as probe
from interoperability_transports import Transport, authored, denial, environment, resource_path
from interoperability_observer import Observer, canonical, consumed_input_ids
from provision_interoperability import grants
from types import SimpleNamespace
import hashlib
import hmac


class InteroperabilityContract(unittest.TestCase):
    def test_authorized_output_witness_never_changes_a_signed_native_observation(self):
        observer=Observer.__new__(Observer)
        native={"execution_id":7,"binding":{"kind":"action","action_ref":"ginterop_g1.probe"},"entrypoint":"interoperability_worker.py","authority_digest":"a"*64}
        receipt={"protocol":"interop-v1","nonce":"b"*24,"api_token_present":False,"runtime_read_status":None}
        notes=[]
        api=SimpleNamespace(call=Mock(return_value={"data":{"id":7,"config":{"secret":"must-never-be-in-evidence"},"result":{"data":receipt,"succeeded":True,"exit_code":0}}}))
        observer.p=SimpleNamespace(b={"pack_ref":"ginterop_g1"},api=api,tokens={"maintenance":"private-token"},note=lambda case,**fields:notes.append((case,fields)))
        with patch.object(observer,"call",return_value=[native]):
            verified=observer.execution(7,receipt)
        self.assertTrue(verified["worker_receipt_verified"])
        self.assertNotIn("worker_receipt_verified",native)
        self.assertNotIn("must-never-be-in-evidence",repr(notes))
        self.assertEqual(api.call.call_args.args[0],"/api/v1/executions/7?include_secret_values=true")

    def test_extended_scenarios_do_not_elevate_the_limited_deployer(self):
        roles=grants("ginterop_g1")
        self.assertEqual(roles["deployer"],[{"resource":"packs","actions":["read","install","configure"],"constraints":{"refs":["ginterop_g1"]}}])
        for role in ("runtimeconsumer","reader"):
            self.assertFalse(any(grant["resource"] in ("keys","permissions","identities") for grant in roles[role]))
    def test_native_denial_requires_status_and_machine_code_and_discards_details(self):
        for value in ({"code":"COMMAND_FAILED"}, {"status":403}, {"status":200,"code":"OK"}, {"status":403,"code":"secret free-form server message"}):
            with self.assertRaises(FixtureError):
                denial(value)
        result = denial({"status":403,"code":"FORBIDDEN","details":{"secret":"do-not-copy"},"error":"raw-private-server-body"})
        self.assertEqual(result,{"status":403,"code":"FORBIDDEN"})

    def test_mcp_native_denial_is_not_a_command_error_or_rest_substitution(self):
        with tempfile.TemporaryDirectory() as directory:
            transport = Transport("mcp",{"command":["/owned/attune-mcp"]},"http://127.0.0.1:12345","private-token",Path(directory))
            for diagnostic in ({"code":"COMMAND_FAILED"},{"status":403,"code":"FORBIDDEN"}):
                output=json.dumps({"jsonrpc":"2.0","id":2,"result":{"isError":True,"structuredContent":diagnostic}})
                with patch("interoperability_transports.foreground",return_value=output):
                    if "status" in diagnostic:
                        self.assertEqual(transport.call("execute",body={"action_ref":"owned.probe"},expect_denial=True),diagnostic)
                    else:
                        with self.assertRaises(FixtureError):
                            transport.call("execute",body={"action_ref":"owned.probe"},expect_denial=True)

    def test_observer_checks_exact_nonce_and_signed_metadata_before_accepting(self):
        with tempfile.TemporaryDirectory() as directory:
            observer=Observer.__new__(Observer)
            observer.key="b"*64
            observer.path=Path(directory)/"context.private.json"
            observer.p=SimpleNamespace(ctx={"run_id":"g1","project":"eaccept-g1-sharedfile"},root=Path(directory),b={"inspector_binary":"/owned/inspector"},note=lambda *args,**kwargs:None)
            operation={"kind":"execution","execution_id":7,"expected_receipt":None}
            request={"run_id":"g1","project":"eaccept-g1-sharedfile","nonce":"a"*24,"operation":operation}
            rows=[{"execution_id":7,"status":"completed"}]
            mac=hmac.new(observer.key.encode(),canonical([1,request,rows]).encode(),hashlib.sha256).hexdigest()
            response={"protocol":1,"request":request,"observations":rows,"mac":mac}
            with patch("interoperability_observer.secrets.token_hex",return_value="a"*24), patch("interoperability_observer.foreground",return_value=json.dumps(response)):
                self.assertEqual(observer.call(operation),rows)
            response["observations"]=[{"execution_id":8,"status":"completed"}]
            with patch("interoperability_observer.secrets.token_hex",return_value="a"*24), patch("interoperability_observer.foreground",return_value=json.dumps(response)):
                with self.assertRaisesRegex(FixtureError,"MAC mismatch"):
                    observer.call(operation)

    def test_input_approval_ids_come_from_saved_selection_receipts(self):
        row={"input_receipts":[{"id":1,"input_approvals":[{"id":17,"digest":"immutable"}]}],"runtime_approvals":[{"id":99}],"current_options":{"approval":100}}
        self.assertEqual(consumed_input_ids(row),{17})

    def test_matrix_has_no_optional_transport_or_merged_principal(self):
        self.assertEqual(set(probe.LANES), {"api", "cli", "mcp", "python", "javascript", "java"})
        self.assertEqual(set(probe.ROLES), {"deployer", "maintenance", "runtimeconsumer", "reader", "sponsor"})
        ctx = {"run_id": "g1", "project": "eaccept-g1-sharedfile", "api_url": "http://127.0.0.1:12345"}
        binding = dict(ctx)
        with self.assertRaisesRegex(FixtureError, "five separate principals"):
            probe.validate(ctx, binding)
        binding["roles"] = {role: {} for role in probe.ROLES}
        with self.assertRaisesRegex(FixtureError, "zero skips"):
            probe.validate(ctx, binding)

    def test_child_environment_never_inherits_runtime_or_developer_credentials(self):
        env = environment(Path("/owned/home"))
        self.assertNotIn("ATTUNE_API_TOKEN", env)
        self.assertNotIn("ATTUNE_PROFILE", env)
        self.assertNotIn("HTTP_PROXY", env)
        self.assertNotIn("PYTHONPATH", env)
        self.assertEqual(env["TMPDIR"], "/owned/home")

    def test_actual_http_body_keeps_omitted_empty_explicit_distinct(self):
        for body in ({"action_ref": "owned.probe"}, {"action_ref": "owned.probe", "permission_set_refs": []}, {"action_ref": "owned.probe", "permission_set_refs": ["owned.runtime"]}):
            method, path, actual = resource_path("execute", {"body": body})
            self.assertEqual((method, path), ("POST", "/api/v1/executions/execute"))
            self.assertEqual(actual, body)

    def test_export_never_falls_back_to_effective_grants(self):
        with self.assertRaisesRegex(FixtureError, "never export effective grants"):
            authored({"grants": [{"resource": "keys", "actions": ["read"]}]})
        definition = {"ref": "owned.runtime", "scope": "pack", "grants": []}
        self.assertEqual(authored({"authored_definition": definition, "label": None, "description": None}), dict(definition, label=None, description=None))

    def test_native_cli_uses_private_profile_and_explicit_selection_flags(self):
        with tempfile.TemporaryDirectory() as directory:
            transport = Transport("cli", {"command": ["/owned/attune"]}, "http://127.0.0.1:12345", "private-token", Path(directory))
            for refs, expected in ((None, []), ([], ["--no-api-token"]), (["owned.runtime"], ["--permission-set", "owned.runtime"])):
                body = {"action_ref": "owned.probe", "parameters": {}}
                if refs is not None:
                    body["permission_set_refs"] = refs
                with patch("interoperability_transports.foreground", return_value='{"id":1}') as process:
                    transport.call("execute", body=body)
                argv = process.call_args.args[0]
                self.assertNotIn("private-token", argv)
                if expected:
                    self.assertEqual(argv[-len(expected):], expected)
                else:
                    self.assertNotIn("--no-api-token", argv)
                    self.assertNotIn("--permission-set", argv)

    def test_mcp_consumes_tool_error_without_http_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            transport = Transport("mcp", {"command": ["/owned/attune-mcp"]}, "http://127.0.0.1:12345", "private-token", Path(directory))
            output = json.dumps({"jsonrpc": "2.0", "id": 2, "result": {"isError": True, "content": []}})
            with patch("interoperability_transports.foreground", return_value=output):
                with self.assertRaisesRegex(FixtureError, "no REST fallback"):
                    transport.call("config_reset", pack="owned", fields=["counter"])

    def test_mcp_input_consent_is_flat_not_a_nested_request(self):
        with tempfile.TemporaryDirectory() as directory:
            transport = Transport("mcp", {"command": ["/owned/attune-mcp"]}, "http://127.0.0.1:12345", "private-token", Path(directory))
            output = json.dumps({"jsonrpc": "2.0", "id": 2, "result": {"isError": False, "structuredContent": {"id": 1}}})
            body = {"policy_revision": 7, "invocation_binding": {"kind": "action", "action_ref": "owned.input_probe"}, "declaration_revision": "revision", "source_path": "/input", "expression": "literal", "grants": []}
            with patch("interoperability_transports.foreground", return_value=output) as process:
                transport.call("issue", pack="owned", body=body)
            messages = [json.loads(line) for line in process.call_args.args[1].splitlines()]
            arguments = messages[-1]["params"]["arguments"]
            self.assertEqual(arguments, dict(body, ref="owned"))
            self.assertNotIn("request", arguments)


if __name__ == "__main__":
    unittest.main()
