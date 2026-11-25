use std::collections::HashMap;
use std::io::Read;

use flate2::read::{GzDecoder, ZlibDecoder};

pub struct Http<'a> {
    pub status: String,
    pub headers: HashMap<String, String>,
    pub payload: &'a [u8],
}

pub fn parse(body: &[u8]) -> Result<Http<'_>, String> {
    let split = body
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no HTTP header terminator")?;
    let head = &body[..split];
    let payload = &body[split + 4..];
    let mut lines = head.split(|byte| *byte == b'\n');
    let status_line = lines.next().ok_or("no HTTP status line")?;
    let status = String::from_utf8_lossy(status_line)
        .split_whitespace()
        .nth(1)
        .ok_or("no HTTP status code")?
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let name = String::from_utf8_lossy(&line[..colon])
            .trim()
            .to_ascii_lowercase();
        let value = String::from_utf8_lossy(&line[colon + 1..])
            .trim()
            .to_string();
        headers.insert(name, value);
    }
    Ok(Http {
        status,
        headers,
        payload,
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
