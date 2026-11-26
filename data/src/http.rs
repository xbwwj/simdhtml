//! The inner HTTP response of a WARC `response` record.
//!
//! Common Crawl stores the payload decoded: original encodings are recorded in
//! `x-crawler-*` headers. Anything still chunked or compressed is rejected
//! rather than silently decoded, so a different corpus cannot skew samples.

use std::str;

pub struct Http<'a> {
    pub status: u16,
    pub content_type: Option<&'a str>,
    pub content_encoding: Option<&'a str>,
    pub transfer_encoding: Option<&'a str>,
    pub payload: &'a [u8],
}

pub fn parse(body: &[u8]) -> Result<Http<'_>, String> {
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut response = httparse::Response::new(&mut headers);
    let head_len = match response.parse(body).map_err(|err| format!("HTTP: {err}"))? {
        httparse::Status::Complete(len) => len,
        httparse::Status::Partial => return Err("truncated HTTP head".to_string()),
    };
    let status = response.code.ok_or("no HTTP status line")?;
    let header = |name: &str| {
        response
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .and_then(|header| str::from_utf8(header.value).ok())
            .map(str::trim)
    };
    Ok(Http {
        status,
        content_type: header("content-type"),
        content_encoding: header("content-encoding"),
        transfer_encoding: header("transfer-encoding"),
        payload: &body[head_len..],
    })
}

pub fn is_html(http: &Http<'_>) -> bool {
    let media_type = http
        .content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    media_type.eq_ignore_ascii_case("text/html")
        || media_type.eq_ignore_ascii_case("application/xhtml+xml")
}

/// The payload bytes, or an error when they are not stored as identity.
pub fn decode_payload<'a>(http: &Http<'a>) -> Result<&'a [u8], String> {
    if let Some(value) = http.transfer_encoding
        && value.to_ascii_lowercase().contains("chunked")
    {
        return Err("chunked transfer-encoding".to_string());
    }
    if let Some(value) = http.content_encoding {
        let codings: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
            .collect();
        if !codings.is_empty() {
            return Err(format!("content-encoding {}", codings.join(",")));
        }
    }
    Ok(http.payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nX-Dup: a\r\nX-Dup: b\r\n\r\nhello";

    #[test]
    fn parses_status_headers_and_payload() {
        let http = parse(RESPONSE).unwrap();
        assert_eq!(http.status, 200);
        assert_eq!(http.content_type.unwrap(), "text/html; charset=utf-8");
        assert_eq!(http.payload, b"hello");
        assert!(is_html(&http));
        assert_eq!(decode_payload(&http).unwrap(), b"hello");
    }

    #[test]
    fn rejects_stored_encodings() {
        let body = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(decode_payload(&parse(body).unwrap()).is_err());

        let body = b"HTTP/1.1 200 OK\r\ncontent-encoding: identity, gzip\r\n\r\nhello";
        assert!(decode_payload(&parse(body).unwrap()).is_err());

        let body = b"HTTP/1.1 200 OK\r\ncontent-encoding: identity\r\n\r\nhello";
        assert_eq!(decode_payload(&parse(body).unwrap()).unwrap(), b"hello");
    }

    #[test]
    fn rejects_malformed_heads() {
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/html").is_err());
        assert!(parse(b"not http").is_err());
    }
}
