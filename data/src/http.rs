use std::collections::HashMap;
use std::io::Read;

use flate2::read::{GzDecoder, ZlibDecoder};

pub struct Http<'a> {
    pub status: String,
    pub headers: HashMap<String, String>,
    pub payload: &'a [u8],
}

pub fn parse(body: &[u8]) -> Result<Http<'_>, String> {
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut response = httparse::Response::new(&mut headers);
    let head_len = match response.parse(body).map_err(|err| format!("HTTP: {err}"))? {
        httparse::Status::Complete(len) => len,
        httparse::Status::Partial => return Err("truncated HTTP head".to_string()),
    };
    let status = response.code.ok_or("no HTTP status line")?.to_string();
    let headers = response
        .headers
        .iter()
        .map(|header| {
            let value = String::from_utf8_lossy(header.value);
            (header.name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect();
    Ok(Http {
        status,
        headers,
        payload: &body[head_len..],
    })
}

pub fn media_type(http: &Http<'_>) -> String {
    http.headers
        .get("content-type")
        .map_or("", String::as_str)
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub fn is_html(http: &Http<'_>) -> bool {
    matches!(
        media_type(http).as_str(),
        "text/html" | "application/xhtml+xml"
    )
}

pub fn decode_payload(http: &Http<'_>) -> Result<Vec<u8>, String> {
    let mut data = http.payload.to_vec();
    if http
        .headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        data = decode_chunked(&data)?;
    }
    if let Some(encoding) = http.headers.get("content-encoding") {
        for coding in encoding.split(',').map(str::trim).rev() {
            data = match coding.to_ascii_lowercase().as_str() {
                "" | "identity" => data,
                "gzip" | "x-gzip" => {
                    let mut out = Vec::new();
                    GzDecoder::new(data.as_slice())
                        .read_to_end(&mut out)
                        .map_err(|err| format!("gzip: {err}"))?;
                    out
                }
                "deflate" => {
                    let mut out = Vec::new();
                    ZlibDecoder::new(data.as_slice())
                        .read_to_end(&mut out)
                        .map_err(|err| format!("deflate: {err}"))?;
                    out
                }
                other => return Err(format!("unsupported content-encoding {other:?}")),
            };
        }
    }
    Ok(data)
}

fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    let mut rest = data;
    loop {
        let Some(end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Err("chunk size without CRLF".to_string());
        };
        let size_text = std::str::from_utf8(&rest[..end])
            .map_err(|_| "non-UTF8 chunk size".to_string())?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size + 2 {
            return Err("truncated chunk".to_string());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 5\r\nX-Dup: a\r\nX-Dup: b\r\n\r\nhello";

    #[test]
    fn parses_status_headers_and_payload() {
        let http = parse(RESPONSE).unwrap();
        assert_eq!(http.status, "200");
        assert_eq!(http.headers["content-type"], "text/html; charset=utf-8");
        assert_eq!(http.headers["x-dup"], "b");
        assert_eq!(http.payload, b"hello");
        assert!(is_html(&http));
        assert_eq!(decode_payload(&http).unwrap(), b"hello");
    }

    #[test]
    fn decodes_chunked_and_gzip() {
        let mut body = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
        body.extend_from_slice(b"5\r\nhello\r\n0\r\n\r\n");
        let http = parse(&body).unwrap();
        assert_eq!(decode_payload(&http).unwrap(), b"hello");

        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"hello").unwrap();
        let gz = encoder.finish().unwrap();
        let mut body = b"HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\n\r\n".to_vec();
        body.extend_from_slice(&gz);
        let http = parse(&body).unwrap();
        assert_eq!(decode_payload(&http).unwrap(), b"hello");
    }

    #[test]
    fn rejects_malformed_heads() {
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/html").is_err());
        assert!(parse(b"not http").is_err());
    }
}
