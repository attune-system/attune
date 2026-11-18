"""Classify bounded nginx startup evidence without relaying log text."""
SIGNATURES = {
    "nginx_certificate_load_failed": "cannot load certificate ",
    "nginx_private_key_load_failed": "cannot load certificate key ",
    "nginx_certificate_key_mismatch": "key values mismatch",
    "nginx_unknown_directive": "unknown directive",
    "nginx_invalid_directive_context": "directive is not allowed here",
    "nginx_invalid_directive_arguments": "invalid number of arguments",
    "nginx_log_open_failed": "could not open error log file",
    "nginx_readonly_filesystem": "Read-only file system",
    "nginx_permission_denied": "Permission denied",
    "nginx_missing_file": "No such file or directory",
    "nginx_bind_failed": "bind()",
}


def classify(logs):
    result = {key: logs.count(value) for key, value in SIGNATURES.items() if value in logs}
    for path, category in [
        ("/etc/attune/controller.crt", "nginx_certificate_path_failure"),
        ("/etc/attune/controller.key", "nginx_private_key_path_failure"),
        ("/var/cache/nginx/", "nginx_default_cache_path_failure"),
        ("/tmp/nginx.pid", "nginx_private_pid_path_failure"),
        ("/tmp/", "nginx_private_scratch_path_failure"),
        ("/usr/sbin/nginx", "nginx_executable_path_failure"),
    ]:
        if path in logs and any(marker in logs for marker in ["failed", "Permission denied", "No such file", "Read-only"]):
            result[category] = logs.count(path)
    return result
