#!/usr/bin/env python3
"""Nginx startup failures remain identifiable without exposing log contents."""
import json
import unittest
from workload_proxy_diagnostics import classify


class ProxyDiagnosticsTests(unittest.TestCase):
    def test_key_readability_is_distinct_from_readonly_module_scratch(self):
        key = classify('cannot load certificate key "/etc/attune/controller.key": BIO_new_file() failed (Permission denied) private-token')
        self.assertEqual(key["nginx_private_key_load_failed"], 1)
        self.assertEqual(key["nginx_private_key_path_failure"], 1)
        cache = classify('mkdir() "/var/cache/nginx/fastcgi_temp" failed (30: Read-only file system) private-token')
        self.assertEqual(cache["nginx_default_cache_path_failure"], 1)
        self.assertEqual(cache["nginx_readonly_filesystem"], 1)
        self.assertNotIn("private-token", json.dumps([key, cache]))

    def test_directive_executable_and_pid_failures_are_fixed_categories(self):
        cases = [("unknown directive secret-directive", "nginx_unknown_directive"),
                 ("/usr/sbin/nginx: Permission denied", "nginx_executable_path_failure"),
                 ('open() "/tmp/nginx.pid" failed (13: Permission denied)', "nginx_private_pid_path_failure")]
        for logs, expected in cases:
            self.assertEqual(classify(logs)[expected], 1)
            self.assertNotIn("secret-directive", json.dumps(classify(logs)))

    def test_private_unrecognized_content_is_never_relayed(self):
        self.assertEqual(classify("Authorization: Bearer private-token\nPRIVATE KEY private-bytes\nopaque raw input"), {})


if __name__ == "__main__":
    unittest.main()
