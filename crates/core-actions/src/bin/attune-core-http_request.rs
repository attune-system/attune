#[path = "http_request/destination.rs"]
mod destination;

use reqwest::{blocking::Client, header, redirect, Method, Url};
use serde_json::{json, Map, Value};
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

// Bound buffering independently of the worker's stdout limit.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

fn string<'a>(
    p: &'a Map<String, Value>,
    key: &str,
    default: &'a str,
) -> Result<&'a str, &'static str> {
    p.get(key).map_or(Ok(default), |v| {
        v.as_str().ok_or("expected a string parameter")
    })
}

fn boolean(p: &Map<String, Value>, key: &str, default: bool) -> Result<bool, &'static str> {
    p.get(key).map_or(Ok(default), |v| {
        v.as_bool().ok_or("expected a boolean parameter")
    })
}

fn safe_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn request(p: &Map<String, Value>) -> Result<Value, &'static str> {
    request_with_resolver(
        p,
        std::sync::Arc::new(destination::PublicResolver),
        #[cfg(test)]
        false,
    )
}

fn request_with_resolver<R: reqwest::dns::Resolve + 'static>(
    p: &Map<String, Value>,
    resolver: std::sync::Arc<R>,
    #[cfg(test)] test_transport: bool,
) -> Result<Value, &'static str> {
    let raw = string(p, "url", "")?;
    if raw.chars().any(|c| c.is_control() || c == ' ') {
        return Err("URL contains unencoded whitespace or control characters");
    }
    let mut url = Url::parse(raw).map_err(|_| "expected an absolute HTTP or HTTPS URL")?;
    if !safe_url(&url) {
        return Err("URL must use HTTP or HTTPS without embedded credentials");
    }
    if let Some(query) = p.get("query_params") {
        let query = query.as_object().ok_or("query_params must be an object")?;
        for (key, value) in query {
            let value = match value {
                Value::String(value) => value.clone(),
                Value::Number(_) | Value::Bool(_) => value.to_string(),
                _ => return Err("query parameter values must be strings, numbers or booleans"),
            };
            url.query_pairs_mut().append_pair(key, &value);
        }
    }
    let method = string(p, "method", "GET")?.to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err("unsupported HTTP method");
    }
    let method = Method::from_bytes(method.as_bytes()).map_err(|_| "invalid HTTP method")?;
    let timeout = core_actions::integer(p, "timeout", 30, 300)?;
    if timeout == 0 {
        return Err("timeout must be at least one second");
    }
    let follow = boolean(p, "follow_redirects", true)?;
    let max_redirects = usize::try_from(core_actions::integer(p, "max_redirects", 10, u64::MAX)?)
        .map_err(|_| "max_redirects is outside the supported range")?;
    let mut headers = header::HeaderMap::new();
    if let Some(values) = p.get("headers") {
        for (name, value) in values.as_object().ok_or("headers must be an object")? {
            let name = header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "invalid HTTP header name")?;
            let value = value.as_str().ok_or("header values must be strings")?;
            let value = header::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|_| "invalid HTTP header value")?;
            headers.insert(name, value);
        }
    }
    let client = Client::builder()
        .no_proxy()
        .dns_resolver(resolver)
        .timeout(Duration::from_secs(timeout))
        .connect_timeout(Duration::from_secs(timeout.min(10)))
        .danger_accept_invalid_certs(!boolean(p, "verify_ssl", true)?)
        .redirect(redirect::Policy::none())
        .referer(false)
        .build()
        .map_err(|_| "HTTP client initialization failed")?;
    let mut request = client.request(method, url).headers(headers);
    match string(p, "auth_type", "none")? {
        "none" => (),
        "basic" => {
            let username = string(p, "auth_username", "")?;
            let password = string(p, "auth_password", "")?;
            if username
                .chars()
                .any(|c| matches!(c, '\r' | '\n' | '\0' | ':'))
                || password.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
            {
                return Err("invalid basic authentication value");
            }
            request = request.basic_auth(username, Some(password));
        }
        "bearer" => {
            let token = string(p, "auth_token", "")?;
            if token.is_empty()
                || token
                    .bytes()
                    .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
            {
                return Err("invalid bearer authentication value");
            }
            request = request.bearer_auth(token);
        }
        _ => return Err("unsupported authentication type"),
    }
    // Preserve JSON-body precedence; explicit Content-Type headers still win.
    if let Some(body) = p.get("json_body") {
        if !body.is_object() {
            return Err("json_body must be an object");
        }
        request = request.json(body);
    } else if p.contains_key("body") {
        request = request.body(string(p, "body", "")?.as_bytes().to_owned());
    }
    let start = Instant::now();
    let mut request = request.build().map_err(|_| "invalid HTTP request")?;
    let mut redirects = 0;
    let response = loop {
        // IP literals bypass reqwest's DNS resolver, so admit them here on every
        // hop. Only unit-test loopback transport can substitute this check.
        #[cfg(test)]
        let check_destination = !test_transport;
        #[cfg(not(test))]
        let check_destination = true;
        #[cfg(test)]
        let fixture_literal = test_transport
            && request.url().host_str().is_some_and(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            });
        #[cfg(not(test))]
        let fixture_literal = false;
        if !fixture_literal {
            destination::check_literal(request.url())?;
        }
        let remaining = Duration::from_secs(timeout)
            .checked_sub(start.elapsed())
            .ok_or("HTTP request timed out")?;
        *request.timeout_mut() = Some(remaining);
        let response = client
            .execute(request.try_clone().ok_or("HTTP body cannot be replayed")?)
            .map_err(|e| {
                if e.is_timeout() {
                    "HTTP request timed out"
                } else {
                    "HTTP request failed"
                }
            })?;
        if check_destination
            && !response
                .remote_addr()
                .is_some_and(|addr| destination::public_address(addr.ip()))
        {
            return Err("HTTP destination is not public");
        }
        let status = response.status().as_u16();
        if !follow || !matches!(status, 301 | 302 | 303 | 307 | 308) {
            break response;
        }
        let Some(location) = response.headers().get(header::LOCATION) else {
            break response;
        };
        let next = response
            .url()
            .join(location.to_str().map_err(|_| "invalid redirect location")?)
            .map_err(|_| "invalid redirect URL")?;
        if !safe_url(&next) {
            return Err("redirect must use HTTP or HTTPS without embedded credentials");
        }
        if redirects >= max_redirects {
            return Err("redirect limit exceeded");
        }
        redirects += 1;
        // Scheme, host AND effective port define the origin. Never forward
        // credentials across origins, including HTTPS-to-HTTP on the same port.
        if next.origin() != request.url().origin() {
            for name in [
                header::AUTHORIZATION,
                header::COOKIE,
                header::PROXY_AUTHORIZATION,
            ] {
                request.headers_mut().remove(name);
            }
        }
        request.headers_mut().remove(header::HOST);
        if ((status == 301 || status == 302) && request.method() == Method::POST)
            || (status == 303 && request.method() != Method::HEAD)
        {
            *request.method_mut() = Method::GET;
            *request.body_mut() = None;
            for name in [
                header::CONTENT_LENGTH,
                header::CONTENT_TYPE,
                header::TRANSFER_ENCODING,
            ] {
                request.headers_mut().remove(name);
            }
        }
        *request.url_mut() = next;
    };
    let status = response.status();
    let final_url = response.url().as_str().to_owned();
    let mut headers = Map::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes());
        let entry = headers
            .entry(name.as_str().to_owned())
            .or_insert_with(|| Value::String(String::new()));
        let previous = entry.as_str().unwrap();
        *entry = Value::String(if previous.is_empty() {
            value.into_owned()
        } else {
            format!("{previous}, {value}")
        });
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "HTTP response read failed")?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("HTTP response exceeds 16 MiB limit");
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let parsed = serde_json::from_str::<Value>(&body)
        .ok()
        .filter(|v| v.is_object() || v.is_array());
    Ok(
        json!({"status_code":status.as_u16(),"headers":headers,"body":body,
        "json":parsed,"elapsed_ms":start.elapsed().as_millis() as u64,"url":final_url,"success":status.is_success()}),
    )
}

fn main() {
    let result = core_actions::read_parameters().and_then(|parameters| request(&parameters));
    emit_result(result);
}

fn emit_result(result: Result<Value, &'static str>) {
    let value = match result {
        Ok(value) => value,
        Err(error) => json!({"status_code":0,"headers":{},"body":"","json":null,
            "elapsed_ms":0,"url":"","success":false,"error":error}),
    };
    let success = value["success"] == true;
    let mut output = io::stdout().lock();
    if serde_json::to_writer(&mut output, &value).is_err()
        || writeln!(output).is_err()
        || output.flush().is_err()
    {
        std::process::exit(1);
    }
    if !success {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::dns::{Addrs, Name, Resolve, Resolving};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    struct MockResolver {
        answers: Mutex<Vec<Vec<SocketAddr>>>,
        check: bool,
    }
    impl Resolve for MockResolver {
        fn resolve(&self, _: Name) -> Resolving {
            let addresses = self.answers.lock().unwrap().remove(0);
            let check = self.check;
            Box::pin(async move {
                if check {
                    Ok(destination::checked_addresses(addresses)?)
                } else {
                    Ok(Box::new(addresses.into_iter()) as Addrs)
                }
            })
        }
    }

    #[test]
    fn address_policy_and_normalized_literals() {
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "168.63.129.16",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "fe80::1",
            "fc00::1",
            "fd00:ec2::254",
            "ff02::1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(!destination::public_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "100.128.0.1",
            "172.32.0.1",
            "192.169.0.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            assert!(destination::public_address(ip.parse().unwrap()), "{ip}");
        }
        for host in [
            "127.1",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "[::ffff:7f00:1]",
        ] {
            assert!(
                destination::check_literal(&Url::parse(&format!("http://{host}/")).unwrap())
                    .is_err()
            );
        }
    }

    #[test]
    fn checked_dns_answers_are_the_exact_connector_addresses() {
        let public = vec![
            "8.8.8.8:0".parse().unwrap(),
            "[2606:4700::1111]:0".parse().unwrap(),
        ];
        assert_eq!(
            destination::checked_addresses(public.clone())
                .unwrap()
                .collect::<Vec<_>>(),
            public
        );
        for bad in [
            "127.0.0.1:0",
            "10.1.2.3:0",
            "169.254.169.254:0",
            "[::ffff:127.0.0.1]:0",
            "[fd00::1]:0",
        ] {
            assert!(destination::checked_addresses(vec![public[0], bad.parse().unwrap()]).is_err());
        }
        assert!(destination::checked_addresses(vec![]).is_err());
    }

    #[test]
    fn dns_denial_has_no_connection_or_credential_effect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let resolver = Arc::new(MockResolver {
            answers: Mutex::new(vec![vec![addr]]),
            check: true,
        });
        let p = json!({"url":format!("http://public-name.invalid:{}/", addr.port()),
            "auth_type":"bearer","auth_token":"never-send-this", "timeout":1});
        let error = request_with_resolver(p.as_object().unwrap(), resolver, false).unwrap_err();
        assert!(!error.contains("never-send-this"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn redirect_dns_rebinding_cannot_reconnect_to_private_answer() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("owned listener failed: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(stream.read(&mut buffer).unwrap() > 0);
            stream.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            // Return the listener to assert no second connection after denial.
            listener
        });
        struct RebindingResolver {
            resolutions: std::sync::atomic::AtomicUsize,
            fixture: SocketAddr,
        }
        impl Resolve for RebindingResolver {
            fn resolve(&self, _: Name) -> Resolving {
                let first = self
                    .resolutions
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 0;
                let fixture = self.fixture;
                Box::pin(async move {
                    if first {
                        // Simulate the first admitted public hop using owned local
                        // transport. Subsequent answers use the production guard.
                        Ok(Box::new(vec![fixture].into_iter()) as Addrs)
                    } else {
                        Ok(destination::checked_addresses(vec![fixture])?)
                    }
                })
            }
        }
        let resolver = Arc::new(RebindingResolver {
            resolutions: std::sync::atomic::AtomicUsize::new(0),
            fixture: addr,
        });
        let p = json!({"url":format!("http://rebind.invalid:{}/start",addr.port()),"timeout":2});
        let result = request_with_resolver(p.as_object().unwrap(), resolver.clone(), true);
        let listener = server.join().unwrap();
        assert!(result.is_err());
        assert_eq!(
            resolver
                .resolutions
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    // Invoked only as a subprocess by the Python mock suite. This branch and
    // its loopback resolver do not exist in any production/release binary.
    #[test]
    fn mock_transport_entrypoint() {
        if std::env::var("ATTUNE_CORE_HTTP_TEST_HARNESS").as_deref() != Ok("1") {
            return;
        }
        struct LoopbackResolver;
        impl Resolve for LoopbackResolver {
            fn resolve(&self, _: Name) -> Resolving {
                Box::pin(async {
                    Ok(
                        Box::new(vec!["127.0.0.1:0".parse::<SocketAddr>().unwrap()].into_iter())
                            as Addrs,
                    )
                })
            }
        }
        emit_result(
            core_actions::read_parameters()
                .and_then(|p| request_with_resolver(&p, Arc::new(LoopbackResolver), true)),
        );
    }

    #[test]
    fn existing_http_semantics_with_test_only_local_transport() {
        // The owned OCI ABI fixture pins this public script inside its RO Pack
        // input. It must not mount the compile-time source checkout/host path.
        // This override exists only in the test executable, never the action.
        let script = std::env::var_os("ATTUNE_CORE_HTTP_TEST_SCRIPT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../scripts/test_native_http_action.py")
            });
        assert!(std::process::Command::new("python3")
            .arg(script)
            .env("ATTUNE_CORE_HTTP_BINARY", std::env::current_exe().unwrap())
            .env("ATTUNE_CORE_HTTP_TEST_HARNESS", "1")
            .status()
            .unwrap()
            .success());
    }
}
