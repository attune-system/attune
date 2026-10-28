"""Owned loopback HTTPS origins for real index, Git, and archive requests."""

import functools
import http.server
import ipaddress
import json
from pathlib import Path
import socket
import ssl
import sys
import threading


def main(root):
    root = Path(root)
    config = json.loads((root / "server.json").read_text())
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(root / "server.pem", root / "server.key")
    record_lock = threading.Lock()
    origins = {}

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, role, **kwargs):
            self.role = role
            super().__init__(*args, directory=str(root / "public"), **kwargs)

        def log_message(self, *args):
            # Never log headers or error bodies. The record contains verdicts only.
            pass

        def serve(self, head=False):
            address = ipaddress.ip_address(self.client_address[0])
            if not address.is_loopback:
                self.send_error(403)
                return
            authorized = self.headers.get_all("Authorization") == [config["tokens"][self.role]]
            isolated = (
                self.headers.get_all("X-Pack-Source") == [self.role]
                and self.headers.get_all(f"X-{self.role.title()}-Only") == [config["tokens"][self.role]]
            ) and all(
                token not in str(self.headers)
                for role, token in config["tokens"].items()
                if role != self.role
            )
            with record_lock, (root / "requests.jsonl").open("a") as records:
                records.write(json.dumps({
                    "role": self.role, "path": self.path,
                    "authorized": authorized, "isolated": isolated,
                }) + "\n")
            if not authorized or not isolated:
                self.send_error(401)
                return
            if self.role == "git" and config["fail_git"]:
                # Git's failure must not disclose even a malicious server's message.
                self.send_error(503, config["tokens"]["git"])
                return
            if self.role == "index":
                body = json.dumps({
                    "registry_name": "Private fixture",
                    "registry_url": origins["index"],
                    "version": "1.0", "last_updated": "2026-01-01T00:00:00Z",
                    "packs": [{
                        "ref": config["pack_ref"], "label": "Private fixture",
                        "description": "Private source integration test",
                        "version": "1.0.0", "author": "Test", "license": "MIT",
                        "keywords": [], "runtime_deps": [],
                        "install_sources": [
                            {"type": "git", "url": origins["git"] + "/pack.git",
                             "ref": config["git_ref"], "checksum": config["git_checksum"]},
                            {"type": "archive", "url": origins["archive"] + "/pack.tar.gz",
                             "checksum": config["archive_checksum"]},
                        ],
                        "contents": {"actions": [], "sensors": [], "triggers": [],
                                     "rules": [], "workflows": []},
                    }],
                }).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if not head:
                    self.wfile.write(body)
            elif head:
                super().do_HEAD()
            else:
                # Bare Git repositories with update-server-info support real dumb HTTP clones.
                super().do_GET()

        def do_GET(self):
            self.serve()

        def do_HEAD(self):
            self.serve(head=True)

    class IPv6Server(http.server.ThreadingHTTPServer):
        address_family = socket.AF_INET6

        def server_bind(self):
            self.socket.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            super().server_bind()

    servers = []
    threads = []
    try:
        for role in config["tokens"]:
            handler = functools.partial(Handler, role=role)
            ipv4 = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
            servers.append(ipv4)
            port = ipv4.server_address[1]
            # The installer pins one DNS answer for Git. Serve both localhost answers.
            servers.append(IPv6Server(("::1", port), handler))
            origins[role] = f"https://localhost:{port}"
        for server in servers:
            server.socket = context.wrap_socket(server.socket, server_side=True)
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
            thread.start()
            threads.append(thread)
        print(json.dumps(origins), flush=True)
        sys.stdin.readline()
    finally:
        for server in servers[:len(threads)]:
            server.shutdown()
        for server in servers:
            server.server_close()
        for thread in threads:
            thread.join()


if __name__ == "__main__":
    main(sys.argv[1])
