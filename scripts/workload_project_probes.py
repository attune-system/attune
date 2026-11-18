"""Finite TLS/caller rejection probes; report only fixed categories/numbers."""
import socket
import ssl
import urllib.error
import urllib.request


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
