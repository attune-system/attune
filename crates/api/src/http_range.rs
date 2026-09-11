use attune_common::blob_store::ByteRange;
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRange {
    pub(crate) bytes: Option<ByteRange>,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl ResolvedRange {
    pub(crate) fn len(self) -> u64 {
        self.end - self.start
    }

    pub(crate) fn is_partial(self) -> bool {
        self.bytes.is_some()
    }
}

pub(crate) fn resolve_range(headers: &HeaderMap, size: u64) -> Result<ResolvedRange, &'static str> {
    let mut values = headers.get_all(header::RANGE).iter();
    let Some(value) = values.next() else {
        return Ok(ResolvedRange {
            bytes: None,
            start: 0,
            end: size,
        });
    };
    if values.next().is_some() {
        return Err("Multiple Range headers are not supported");
    }
    let value = value
        .to_str()
        .map_err(|_| "Range header is not valid ASCII")?
        .trim();
    let (unit, range) = value
        .split_once('=')
        .ok_or("Range header must use the bytes unit")?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return Err("Range header must use the bytes unit");
    }
    let range = range.trim();
    if range.is_empty() || range.contains(',') || size == 0 {
        return Err("Range is not satisfiable");
    }

    let (start, end) = range
        .split_once('-')
        .ok_or("Range must contain exactly one byte range")?;
    let (start, end) = if start.is_empty() {
        let suffix = parse_number(end)?;
        if suffix == 0 {
            return Err("Range suffix must be greater than zero");
        }
        (size.saturating_sub(suffix), size)
    } else {
        let start = parse_number(start)?;
        if start >= size {
            return Err("Range starts beyond the end of the object");
        }
        let end = if end.is_empty() {
            size
        } else {
            let inclusive_end = parse_number(end)?;
            if inclusive_end < start {
                return Err("Range end precedes its start");
            }
            inclusive_end.saturating_add(1).min(size)
        };
        (start, end)
    };

    Ok(ResolvedRange {
        bytes: Some(ByteRange::new(start, end).map_err(|_| "Range is not satisfiable")?),
        start,
        end,
    })
}

fn parse_number(value: &str) -> Result<u64, &'static str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Range bounds must be unsigned decimal integers");
    }
    value.parse().map_err(|_| "Range bound is too large")
}

pub(crate) fn insert_range_headers(headers: &mut HeaderMap, range: ResolvedRange, size: u64) {
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&range.len().to_string()).expect("u64 is a valid header value"),
    );
    if range.is_partial() {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{size}", range.start, range.end - 1))
                .expect("resolved byte range is a valid header value"),
        );
    }
}

pub(crate) fn range_status(range: ResolvedRange) -> StatusCode {
    if range.is_partial() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    }
}

pub(crate) fn range_not_satisfiable(size: u64, message: &str) -> Response {
    let mut response = (StatusCode::RANGE_NOT_SATISFIABLE, message.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{size}"))
            .expect("object size is a valid header value"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(value: Option<&str>, size: u64) -> Result<ResolvedRange, &'static str> {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(header::RANGE, value.parse().unwrap());
        }
        resolve_range(&headers, size)
    }

    #[test]
    fn resolves_bounded_open_and_suffix_ranges() {
        assert_eq!(
            resolve(Some("bytes=2-5"), 10).unwrap().bytes,
            ByteRange::new(2, 6).ok()
        );
        assert_eq!(
            resolve(Some("bytes=7-"), 10).unwrap().bytes,
            ByteRange::new(7, 10).ok()
        );
        assert_eq!(
            resolve(Some("bytes=-3"), 10).unwrap().bytes,
            ByteRange::new(7, 10).ok()
        );
        assert_eq!(
            resolve(Some("bytes=7-99"), 10).unwrap().bytes,
            ByteRange::new(7, 10).ok()
        );
        assert_eq!(
            resolve(Some("bytes=-99"), 10).unwrap().bytes,
            ByteRange::new(0, 10).ok()
        );
        assert_eq!(resolve(None, 0).unwrap().len(), 0);
    }

    #[test]
    fn rejects_invalid_and_multiple_ranges() {
        for value in [
            "items=0-1",
            "bytes=",
            "bytes=1-2,4-5",
            "bytes=10-",
            "bytes=5-4",
            "bytes=-0",
            "bytes=a-b",
        ] {
            assert!(resolve(Some(value), 10).is_err(), "{value}");
        }
        assert!(resolve(Some("bytes=0-0"), 0).is_err());

        let mut headers = HeaderMap::new();
        headers.append(header::RANGE, "bytes=0-1".parse().unwrap());
        headers.append(header::RANGE, "bytes=2-3".parse().unwrap());
        assert!(resolve_range(&headers, 10).is_err());
    }

    #[test]
    fn writes_exact_partial_and_unsatisfied_headers() {
        let range = resolve(Some("bytes=2-5"), 10).unwrap();
        let mut headers = HeaderMap::new();
        insert_range_headers(&mut headers, range, 10);

        assert_eq!(range_status(range), StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(headers[header::CONTENT_LENGTH], "4");
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");

        let response = range_not_satisfiable(10, "bad range");
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");
    }
}
