use reqwest::Url;

pub fn url_for_log(value: &str) -> String {
    let Ok(url) = Url::parse(value) else {
        return "<redacted URL>".to_string();
    };

    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        return "<redacted URL>".to_string();
    }

    origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_url_keeps_only_the_origin() {
        for (value, expected) in [
            (
                "https://api-user:api-password@example.com:8443/path-credential/attune?query-credential=secret#fragment-credential",
                "https://example.com:8443",
            ),
            (
                "wss://ws-user:ws-password@notifier.example/ws-path-secret?ws-query-secret#ws-fragment-secret",
                "wss://notifier.example",
            ),
        ] {
            let safe = url_for_log(value);
            assert_eq!(safe, expected);
            for credential in [
                "user",
                "password",
                "path",
                "query",
                "secret",
                "fragment",
            ] {
                assert!(!safe.contains(credential), "leaked {credential}: {safe}");
            }
        }
    }

    #[test]
    fn log_url_fails_closed_when_parsing_fails() {
        let value = "not a URL/path-credential?query-credential#fragment-credential";
        assert_eq!(url_for_log(value), "<redacted URL>");
        assert!(!url_for_log(value).contains("credential"));
    }
}
