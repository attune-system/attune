"""Finite TLS/caller rejection probes; report only fixed categories/numbers."""
import socket
import ssl
import urllib.error
import urllib.request

ASSERTIONS = {
    "proxy received controller authority/storage": "proxy_authority_mount",
    "controller received proxy TLS private key": "controller_private_tls_mount",
    "public CP trust roots were not read-only": "public_ca_mount_not_readonly",
    "project process confinement changed": "process_policy_mismatch",
    "project capabilities changed": "capability_ceiling_mismatch",
    "project security policy was relaxed": "security_policy_mismatch",
    "project process inherited extra environment": "environment_clearing_missing",
    "project entrypoint received general manager configuration": "manager_configuration_exposed",
    "private log absence cannot be established from incomplete diagnostics": "incomplete_log_evidence",
    "private marker appeared in project logs": "private_marker_detected",
}


def assertion_category(error):
    # Match only this script's fixed assertions. Never serialize an arbitrary
    # exception message, subprocess argv, request body, token or certificate.
    if isinstance(error, RuntimeError) and len(error.args) == 1 and isinstance(error.args[0], str) and len(error.args[0]) <= 256:
        return ASSERTIONS.get(error.args[0], "other_assertion")
    return "not_a_known_assertion"


def capability_evidence(policy):
    added = set(policy.get("CapAdd") or [])
    recognized = {name for name in added if name in {"CHOWN", "DAC_OVERRIDE", "FOWNER", "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FOWNER"}}
    return {"known_added_capabilities": len(recognized), "unexpected_added_capabilities": len(added - recognized),
            "prefixed_known_capabilities": sum(name.startswith("CAP_") for name in recognized),
            "drops_all_capabilities": set(policy.get("CapDrop") or []) == {"ALL"}}


def exception_category(error):
    if isinstance(error, urllib.error.URLError):
        return exception_category(error.reason)
    if isinstance(error, ssl.SSLCertVerificationError):
        code = getattr(error, "verify_code", None)
        if code in {62, 64}:
            return "hostname_verification"
        if code in {18, 19, 20, 21}:
            return "untrusted_certificate"
        return "certificate_verification"
    if isinstance(error, ssl.SSLError):
        return "tls_protocol"
    if isinstance(error, (TimeoutError, socket.timeout)):
        return "transport_timeout"
    if isinstance(error, ConnectionRefusedError):
        return "connection_refused"
    if isinstance(error, OSError):
        return "transport_error"
    return "assertion_failed"


def tls_rejection(port, context, hostname, subphase, report):
    if subphase not in {"untrusted_ca", "wrong_hostname"}:
        raise ValueError("invalid static TLS probe")
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=3) as transport:
            with context.wrap_socket(transport, server_hostname=hostname):
                report({"phase": "tls-and-caller-denials", "subphase": subphase,
                        "outcome": "unexpected_tls_acceptance"})
                raise RuntimeError("TLS rejection probe unexpectedly accepted")
    except ssl.SSLCertVerificationError as error:
        category = exception_category(error)
        code = getattr(error, "verify_code", None)
        evidence = {"phase": "tls-and-caller-denials", "subphase": subphase,
                    "outcome": "tls_rejected", "exception_category": category}
        if type(code) is int and 0 <= code <= 2**31 - 1:
            evidence["ssl_verify_code"] = code
        report(evidence)
        expected = "untrusted_certificate" if subphase == "untrusted_ca" else "hostname_verification"
        if category != expected:
            raise RuntimeError("TLS rejection used an unrelated verification condition") from None
    except (OSError, urllib.error.URLError) as error:
        report({"phase": "tls-and-caller-denials", "subphase": subphase,
                "outcome": "unexpected_transport_failure", "exception_category": exception_category(error)})
        raise RuntimeError("TLS probe transport failed") from None


def caller_rejection(client, origin, body, headers, subphase, report):
    if subphase not in {"missing_grant", "wrong_grant", "broad_bearer"}:
        raise ValueError("invalid static caller probe")
    request = urllib.request.Request(origin + "/v1/launches/1/status", data=body,
        headers={"Content-Type": "application/json", **headers}, method="POST")
    try:
        with client.open(request, timeout=5) as response:
            report({"phase": "tls-and-caller-denials", "subphase": subphase,
                    "outcome": "unexpected_http_acceptance", "http_status": response.status})
        raise RuntimeError("caller rejection probe unexpectedly accepted")
    except urllib.error.HTTPError as error:
        status = error.code
        error.close()  # Never read or log response bodies/reason/private headers.
        report({"phase": "tls-and-caller-denials", "subphase": subphase,
                "outcome": "http_rejected" if status in {401, 403} else "unexpected_http_status",
                "http_status": status})
        if status not in {401, 403}:
            raise RuntimeError("caller denial changed into an unrelated response") from None
    except (OSError, urllib.error.URLError) as error:
        report({"phase": "tls-and-caller-denials", "subphase": subphase,
                "outcome": "unexpected_transport_failure", "exception_category": exception_category(error)})
        raise RuntimeError("caller probe transport failed") from None
