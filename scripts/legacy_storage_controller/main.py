#!/usr/bin/env python3
"""Deployment-owned native legacy writer management. Disabled by default."""
import argparse
import base64
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import os
from pathlib import Path
import re
import ssl
import sqlite3
import stat
import subprocess
import sys
import time

# Supports Python isolated mode without trusting PYTHONPATH or the current
# directory. E installs this complete directory root-owned and read-only.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from controller import Controller
from journal import Journal
from http_input import BoundedInput
from native import Native
from protocol import MAX_BODY, Quarantine, canonical, decode, digest, identity, require


def protected_file(path, private_material=False):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        value = os.fstat(stream.fileno())
        require(stat.S_ISREG(value.st_mode) and value.st_uid == 0 and value.st_mode & 0o022 == 0,
                "DeploymentOwnedFileRequired")
        require(not private_material or value.st_mode & 0o007 == 0, "PrivateKeyPermissionsRequired")
        require(not private_material or os.geteuid() == 0 or value.st_gid == os.getegid(),
                "PrivateKeyGroupMismatch")
        data = stream.read(MAX_BODY + 1)
        require(len(data) <= MAX_BODY, "ConfigurationBound")
        return data


def configuration(path):
    config = decode(protected_file(path))
    expected = {"enabled_for_controlled_acceptance", "generation", "scope", "service_uid",
                "host_pid_namespace", "roots", "cgroup", "writer_binary", "writer_sha256",
                "state_dir", "bind", "port", "tls_certificate", "tls_key", "tls_client_ca",
                "signing_key", "issuer", "audience", "principals"}
    require(isinstance(config, dict) and set(config) == expected, "InvalidConfiguration")
    identity(config["generation"])
    require(set(config["scope"]) == {"backend", "staging_prefix"}
            and re.fullmatch("[a-f0-9]{64}", config["scope"]["backend"])
            and config["scope"]["staging_prefix"] == ".attune-upload/", "InvalidScope")
    require(set(config["roots"]) == {"blob", "shared"}, "InvalidRoots")
    require(re.fullmatch("[a-f0-9]{64}", config["writer_sha256"]) is not None,
            "InvalidWriterDigest")
    require(type(config["service_uid"]) is int and config["service_uid"] > 0,
            "DedicatedUnprivilegedServiceRequired")
    require(type(config["enabled_for_controlled_acceptance"]) is bool, "InvalidGate")
    require(config["issuer"] and config["audience"] and len(config["principals"]) <= 257,
            "InvalidTrustConfiguration")
    roles = set()
    for fingerprint, principal in config["principals"].items():
        require(re.fullmatch("[a-f0-9]{64}", fingerprint) is not None, "InvalidPrincipal")
        require(principal["role"] in ("manager", "writer"), "InvalidPrincipal")
        roles.add(principal["role"])
        if principal["role"] == "writer":
            require(set(principal) == {"role", "native_instance", "worker_instance", "execution_attempt"},
                    "InvalidPrincipal")
            for key in ("native_instance", "worker_instance", "execution_attempt"):
                identity(principal[key])
        else:
            require(set(principal) == {"role"}, "InvalidPrincipal")
    require(roles == {"manager", "writer"}, "DistinctPrincipalsRequired")
    for field in ("tls_certificate", "tls_key", "tls_client_ca", "signing_key"):
        protected_file(config[field], private_material=field in ("tls_key", "signing_key"))
    return config


def signed_observation(config, request, data):
    now = int(time.time())
    claims = {"iss": config["issuer"], "aud": config["audience"], "iat": now, "exp": now + 30,
              "nonce": request["nonce"], "audit": request["plan"]["id"],
              "operation": request["operation"], "request_digest": digest(request),
              "scope": config["scope"], "data": data}
    return sign_claims(config, claims)


def sign_claims(config, claims):
    encode = lambda value: base64.urlsafe_b64encode(canonical(value)).rstrip(b"=")
    message = encode({"alg": "RS256", "typ": "JWT"}) + b"." + encode(claims)
    # No shell, inherited environment, credentials in argv, or error-body logs.
    result = subprocess.run(["/usr/bin/openssl", "dgst", "-sha256", "-sign", config["signing_key"],
                             "-sigopt", "rsa_padding_mode:pkcs1"],
                            input=message, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            env={}, timeout=2, check=False)
    require(result.returncode == 0 and 256 <= len(result.stdout) <= 1024, "SigningUnavailable")
    signed = message + b"." + base64.urlsafe_b64encode(result.stdout).rstrip(b"=")
    require(len(signed) <= MAX_BODY, "ObservationBound")
    return signed


class Handler(BaseHTTPRequestHandler):
    # HTTPServer is intentionally single-threaded. Durable admit/fence ordering,
    # fork-before-exec cgroup attachment, and exact inventory mutation are serial.
    protocol_version = "HTTP/1.0"

    def setup(self):
        super().setup()
        self.rfile = BoundedInput(self.rfile, self.connection)

    def log_message(self, *_):
        pass

    def reply(self, status, body, content_type="application/json"):
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.send_header("cache-control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        try:
            certificate = self.connection.getpeercert(binary_form=True)
            principal = self.server.config["principals"].get(hashlib.sha256(certificate).hexdigest())
            require(principal is not None, "UnregisteredDeploymentPrincipal")
            require(self.headers.get("transfer-encoding") is None, "UnsupportedTransferEncoding")
            require(self.headers.get_all("content-type", []) == ["application/json"],
                    "JsonContentTypeRequired")
            lengths = self.headers.get_all("content-length", [])
            require(len(lengths) == 1 and lengths[0].isascii() and lengths[0].isdigit(),
                    "ContentLengthRequired")
            length = int(lengths[0])
            require(0 < length <= MAX_BODY, "RequestTooLarge")
            data = self.rfile.read(length)
            require(len(data) == length, "IncompleteRequest")
            request = decode(data)
            origin = re.fullmatch(r"/native-generations/([a-f0-9-]{36})/prepare-origin", self.path)
            if origin:
                require(principal["role"] == "manager", "ManagerPrincipalRequired")
                require(request["generation"] == origin[1], "GenerationPathMismatch")
                identity(request["nonce"])
                original = self.server.controller.prepare_origin(request)
                now = int(time.time())
                claims = {"iss": self.server.config["issuer"], "aud": self.server.config["audience"],
                          "iat": now, "exp": now + 30, "purpose": "controlled-native-origin",
                          "nonce": request["nonce"], "request_digest": digest(request),
                          "generation": origin[1], "scope": self.server.config["scope"], "data": original}
                self.reply(200, sign_claims(self.server.config, claims), "application/jose")
                return
            writer = re.fullmatch(r"/native-writers/([a-f0-9-]{36})", self.path)
            if writer:
                require(request["member"]["operation"] == writer[1], "OperationPathMismatch")
                result = self.server.controller.write(request, principal)
                self.reply(202, canonical(result))
                return
            require(principal["role"] == "manager", "ManagerPrincipalRequired")
            match = re.fullmatch(r"/legacy-file-audits/([a-f0-9-]{36})/([a-z-]+)", self.path)
            require(match is not None and request["plan"]["id"] == match[1]
                    and request["operation"] == match[2], "ManagementPathMismatch")
            identity(request["nonce"])
            result = self.server.controller.management(request)
            self.reply(200, signed_observation(self.server.config, request, result), "application/jose")
        except Quarantine as error:
            self.reply(409, canonical({"state": "quarantined", "diagnostic": str(error)}))
        except (OSError, ValueError, KeyError, TypeError, RecursionError,
                subprocess.SubprocessError, sqlite3.Error):
            # Never expose backend paths, payloads, certificate material, sqlite
            # errors or subprocess stderr. Lost replies cannot mint settlement.
            self.reply(503, canonical({"state": "quarantined", "diagnostic": "NativeControlUnavailable"}))


class Server(HTTPServer):
    def get_request(self):
        sock, address = super().get_request()
        sock.settimeout(5)
        try:
            return self.tls.wrap_socket(sock, server_side=True), address
        except BaseException:
            sock.close()
            raise

    def handle_error(self, *_):
        pass


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Examples: main.py check --config /etc/attune/native-files.json; "
               "main.py init --config /etc/attune/native-files.json --enable-controlled-acceptance; "
               "main.py serve --config /etc/attune/native-files.json --enable-controlled-acceptance")
    parser.add_argument("command", choices=("check", "init", "serve"))
    parser.add_argument("--config", required=True, help="Root-owned private deployment configuration")
    parser.add_argument("--enable-controlled-acceptance", action="store_true",
                        help="Explicitly opt in to owned validation only; production gate remains off")
    args = parser.parse_args()
    os.umask(0o077)
    for name in ("main.py", "controller.py", "journal.py", "native.py", "protocol.py", "http_input.py"):
        protected_file(Path(__file__).resolve().parent / name)
    config = configuration(args.config)
    if args.command == "check":
        print('{"configuration":"valid","production_enabled":false}')
        return
    require(args.enable_controlled_acceptance and config["enabled_for_controlled_acceptance"],
            "ControlledAcceptanceGateDisabled")
    state = Path(config["state_dir"])
    mode = state.stat(follow_symlinks=False)
    require(stat.S_ISDIR(mode.st_mode) and mode.st_uid == os.geteuid()
            and stat.S_IMODE(mode.st_mode) == 0o700, "PrivatePersistentJournalRequired")
    require(state.resolve() == state, "StatePathAlias")
    lock = os.open(state / "controller.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    path = state / "journal.sqlite3"
    if args.command == "init":
        require(not path.exists(), "GenerationAlreadyInitialized")
    else:
        require(path.is_file() and not path.is_symlink(), "JournalNotProvisioned")
    native = Native(config, initialize=args.command == "init")
    if args.command == "init":
        require(all(not os.listdir(fd) for fd in native.roots.values()), "UntrackedHistoricalRoot")
        require(not native.cgroup_read("cgroup.procs").strip(), "UntrackedHistoricalWriter")
    journal = Journal(path)
    journal.bind(native.binding(), initialize=args.command == "init")
    state_fd = os.open(state, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(state_fd)
    finally:
        os.close(state_fd)
    if args.command == "init":
        journal.db.close()
        print('{"generation":"initialized","production_enabled":false}')
        return
    server = Server((config["bind"], config["port"]), Handler)
    server.config = config
    server.controller = Controller(config, journal, native)
    server.tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    server.tls.minimum_version = ssl.TLSVersion.TLSv1_3
    server.tls.verify_mode = ssl.CERT_REQUIRED
    server.tls.load_cert_chain(config["tls_certificate"], config["tls_key"])
    server.tls.load_verify_locations(config["tls_client_ca"])
    try:
        server.serve_forever()
    finally:
        # Shutdown is not a settlement receipt. Native fence + syncfs recovery
        # still runs on the next authenticated audit request.
        server.server_close()
        journal.db.close()
        os.close(lock)


if __name__ == "__main__":
    try:
        main()
    except Quarantine as error:
        print(canonical({"state": "quarantined", "diagnostic": str(error)}).decode(), file=sys.stderr)
        sys.exit(2)
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error):
        print('{"state":"quarantined","diagnostic":"NativeControlUnavailable"}', file=sys.stderr)
        sys.exit(2)
