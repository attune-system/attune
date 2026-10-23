"""Credential-free wire types shared by the native controller and its journal."""
import hashlib
import json
import uuid

MAX_BODY = 1024 * 1024
MAX_REQUESTS = 256
MAX_ITEMS = 2048


class Quarantine(Exception):
    """Only finite diagnostic codes cross the management boundary."""


def require(condition, code):
    if not condition:
        raise Quarantine(code)


def canonical(value):
    # JCS subset used by this protocol. Reject floats/unsafe integers rather
    # than signing a digest different from Rust serde_jcs's IEEE754 encoding.
    if value is None or type(value) is bool:
        return json.dumps(value).encode()
    if type(value) is int:
        require(abs(value) <= 9007199254740991, "UnsafeInteger")
        return str(value).encode()
    if isinstance(value, str):
        try:
            return json.dumps(value, ensure_ascii=False).encode("utf-8")
        except UnicodeError:
            raise Quarantine("InvalidUnicode") from None
    if isinstance(value, list):
        return b"[" + b",".join(map(canonical, value)) + b"]"
    require(isinstance(value, dict), "InvalidJsonType")
    require(all(isinstance(key, str) for key in value), "InvalidJsonKey")
    return b"{" + b",".join(
        canonical(key) + b":" + canonical(value[key])
        for key in sorted(value, key=lambda key: key.encode("utf-16be"))
    ) + b"}"


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def decode(data):
    require(len(data) <= MAX_BODY, "RequestTooLarge")

    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, "DuplicateJsonKey")
            result[key] = value
        return result

    try:
        value = json.loads(data, object_pairs_hook=pairs)
        canonical(value)
        return value
    except (ValueError, UnicodeError, RecursionError):
        raise Quarantine("InvalidJson") from None


def identity(value):
    try:
        require(str(uuid.UUID(value)) == value, "InvalidIdentity")
    except (ValueError, TypeError, AttributeError):
        raise Quarantine("InvalidIdentity") from None
    return value


def relative(key):
    require(isinstance(key, str) and 0 < len(key) <= 512, "InvalidPath")
    require(not key.startswith("/") and "\\" not in key and "\x00" not in key and ":" not in key,
            "InvalidPath")
    require(all(part not in ("", ".", "..") for part in key.split("/")), "InvalidPath")
    return key


def member(value):
    fields = {"operation", "version", "root_read", "object_key", "provider_version",
              "staging_path", "execution_attempt", "worker_instance", "native_instance"}
    require(isinstance(value, dict) and set(value) == fields, "InvalidMember")
    identity(value["operation"])
    for key in ("execution_attempt", "worker_instance", "native_instance"):
        require(value[key] is not None, "UntrackedWriterIdentity")
        identity(value[key])
    for key in ("version", "root_read"):
        require(type(value[key]) is int and 0 < value[key] <= 9007199254740991,
                "InvalidMember")
    for key in ("object_key", "staging_path"):
        if value[key] is not None:
            relative(value[key])
    require(value["object_key"] is not None or value["staging_path"] is not None,
            "UnlocatedMember")
    require(value["provider_version"] is None
            or isinstance(value["provider_version"], str) and len(value["provider_version"]) <= 512,
            "InvalidMember")
    return value


def plan(value, scope):
    require(isinstance(value, dict) and set(value) == {"id", "scope", "members"},
            "InvalidPlan")
    identity(value["id"])
    require(value["scope"] == scope, "ScopeMismatch")
    require(isinstance(value["members"], list) and 0 < len(value["members"]) <= MAX_REQUESTS,
            "MemberBound")
    for entry in value["members"]:
        member(entry)
    require(len({entry["operation"] for entry in value["members"]}) == len(value["members"]),
            "DuplicateOperation")
    return value
