"""Pure bounded-reader tests. No TLS sockets, DB, processes or providers."""
import io
import unittest
from unittest.mock import Mock, patch

from http_input import BoundedInput
from protocol import Quarantine


class HttpInputTests(unittest.TestCase):
    def test_buffered_header_does_not_consume_or_lose_body(self):
        value = BoundedInput(io.BufferedReader(io.BytesIO(b"Header: value\r\n\r\nbody")), Mock())
        self.assertEqual(value.readline(65537), b"Header: value\r\n")
        self.assertEqual(value.readline(65537), b"\r\n")
        self.assertEqual(value.read(4), b"body")

    def test_header_has_aggregate_bound(self):
        value = BoundedInput(io.BufferedReader(io.BytesIO(b"X: 1\r\n" * 3000)), Mock())
        with self.assertRaisesRegex(Quarantine, "HeaderByteBound"):
            for _ in range(3000):
                value.readline(65537)

    def test_trickle_does_not_refresh_deadline(self):
        with patch("http_input.time.monotonic", return_value=1) as clock:
            raw = Mock()
            raw.read1.side_effect = [b"x", b"y", b"z"]
            value = BoundedInput(raw, Mock(), seconds=5)
            clock.side_effect = [2, 3, 4, 5, 6]
            with self.assertRaisesRegex(TimeoutError, "RequestDeadline"):
                value.read(3)
            self.assertLess(raw.read1.call_count, 3)

    def test_body_bound_precedes_any_socket_read(self):
        raw = Mock()
        value = BoundedInput(raw, Mock())
        with self.assertRaisesRegex(Quarantine, "RequestTooLarge"):
            value.read(1024 * 1024 + 1)
        raw.read1.assert_not_called()

    def test_short_body_is_returned_for_the_handler_to_reject(self):
        value = BoundedInput(io.BufferedReader(io.BytesIO(b"short")), Mock())
        self.assertEqual(value.read(12), b"short")


if __name__ == "__main__":
    unittest.main()
