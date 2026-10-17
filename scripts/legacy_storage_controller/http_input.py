"""Aggregate HTTP deadline and header/body bounds, including trickle clients."""
import time

from protocol import MAX_BODY, require


class BoundedInput:
    def __init__(self, raw, connection, seconds=5):
        self.raw, self.connection = raw, connection
        self.deadline = time.monotonic() + seconds
        self.buffer = bytearray()
        self.header_bytes = 0

    def check_deadline(self):
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("RequestDeadline")
        self.connection.settimeout(left)

    def fill(self):
        self.check_deadline()
        chunk = self.raw.read1(4096)
        self.buffer.extend(chunk)
        return bool(chunk)

    def readline(self, size=-1):
        require(size <= 65537, "RequestLineBound")
        while True:
            self.check_deadline()
            end = self.buffer.find(b"\n") + 1
            if size >= 0 and len(self.buffer) >= size:
                end = min(end or size, size)
            if end:
                result = bytes(self.buffer[:end])
                del self.buffer[:end]
                self.header_bytes += len(result)
                require(self.header_bytes <= 16384, "HeaderByteBound")
                return result
            require(self.header_bytes + len(self.buffer) <= 16384, "HeaderByteBound")
            if not self.fill():
                result = bytes(self.buffer)
                self.buffer.clear()
                self.header_bytes += len(result)
                require(self.header_bytes <= 16384, "HeaderByteBound")
                return result

    def read(self, size):
        require(type(size) is int and 0 <= size <= MAX_BODY, "RequestTooLarge")
        result = bytearray()
        while len(result) < size:
            self.check_deadline()
            if not self.buffer and not self.fill():
                break
            count = min(size - len(result), len(self.buffer))
            result.extend(self.buffer[:count])
            del self.buffer[:count]
        return bytes(result)

    def close(self):
        self.raw.close()
