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
