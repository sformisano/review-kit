//! HTTP/1.1 message framing shared by the fixture proxy and file server.
//!
//! Bodies stay in their transmitted encoding: a chunked body keeps its chunk
//! framing so a forwarded message is byte-identical to the received one.

use std::io::{self, BufRead, Read, Write};

/// Largest accepted start line plus headers.
pub const HEAD_LIMIT: usize = 64 * 1024;
/// Largest accepted message body in transmitted bytes.
pub const BODY_LIMIT: usize = 64 * 1024 * 1024;

const HOP_BY_HOP: [&str; 6] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "upgrade",
    "expect",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub start: String,
    pub headers: Vec<(String, String)>,
}

/// How the body following a head is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Empty,
    Length(usize),
    Chunked,
    UntilClose,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Returns a copy without hop-by-hop headers and with `Connection: close`,
    /// so each forwarded exchange owns exactly one connection.
    pub fn for_single_exchange(&self) -> Head {
        let listed: Vec<String> = self
            .header("connection")
            .map(|value| {
                value
                    .split(',')
                    .map(|name| name.trim().to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let mut headers: Vec<_> = self
            .headers
            .iter()
            .filter(|(key, _)| {
                let key = key.to_ascii_lowercase();
                !HOP_BY_HOP.contains(&key.as_str()) && !listed.contains(&key)
            })
            .cloned()
            .collect();
        headers.push(("Connection".to_owned(), "close".to_owned()));
        Head {
            start: self.start.clone(),
            headers,
        }
    }

    pub fn write_to(&self, output: &mut impl Write) -> io::Result<()> {
        let mut text = format!("{}\r\n", self.start);
        for (key, value) in &self.headers {
            text.push_str(key);
            text.push_str(": ");
            text.push_str(value);
            text.push_str("\r\n");
        }
        text.push_str("\r\n");
        output.write_all(text.as_bytes())
    }

    /// Parses a request line into its method and request target.
    pub fn request_line(&self) -> io::Result<(String, String)> {
        let mut parts = self.start.split(' ');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(method), Some(target), Some(version), None)
                if !method.is_empty() && version.starts_with("HTTP/1.") =>
            {
                Ok((method.to_owned(), target.to_owned()))
            }
            _ => Err(invalid("malformed request line")),
        }
    }

    /// Parses a status line into its numeric status code.
    pub fn status(&self) -> io::Result<u16> {
        let mut parts = self.start.splitn(3, ' ');
        match (parts.next(), parts.next()) {
            (Some(version), Some(code)) if version.starts_with("HTTP/1.") => {
                code.parse().map_err(|_| invalid("malformed status code"))
            }
            _ => Err(invalid("malformed status line")),
        }
    }

    fn chunked(&self) -> bool {
        self.header("transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
        })
    }

    fn content_length(&self) -> io::Result<Option<usize>> {
        self.header("content-length")
            .map(|value| {
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid("malformed Content-Length"))
            })
            .transpose()
    }

    pub fn request_framing(&self) -> io::Result<Framing> {
        if self.chunked() {
            return Ok(Framing::Chunked);
        }
        Ok(self
            .content_length()?
            .map_or(Framing::Empty, Framing::Length))
    }

    /// Response framing per RFC 9112 section 6.3 for a request with `method`.
    pub fn response_framing(&self, method: &str) -> io::Result<Framing> {
        let status = self.status()?;
        if method.eq_ignore_ascii_case("HEAD")
            || (100..200).contains(&status)
            || matches!(status, 204 | 304)
        {
            return Ok(Framing::Empty);
        }
        if self.chunked() {
            return Ok(Framing::Chunked);
        }
        Ok(self
            .content_length()?
            .map_or(Framing::UntilClose, Framing::Length))
    }
}

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// Reads one message head. Returns `None` when the peer closes before sending a byte.
pub fn read_head(reader: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let remaining = HEAD_LIMIT.saturating_sub(total) as u64 + 1;
        let read = reader
            .by_ref()
            .take(remaining)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            if lines.is_empty() && total == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed inside a message head",
            ));
        }
        total += read;
        if total > HEAD_LIMIT {
            return Err(invalid("message head exceeds its size limit"));
        }
        let text = String::from_utf8(line).map_err(|_| invalid("message head is not UTF-8"))?;
        let text = text
            .strip_suffix('\n')
            .ok_or_else(|| invalid("message head line is unterminated"))?;
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(text.to_owned());
    }
    let mut lines = lines.into_iter();
    let start = lines.next().ok_or_else(|| invalid("missing start line"))?;
    let headers = lines
        .map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                .filter(|(key, _)| !key.is_empty())
                .ok_or_else(|| invalid("malformed header line"))
        })
        .collect::<io::Result<_>>()?;
    Ok(Some(Head { start, headers }))
}

/// Reads a body in its transmitted encoding.
pub fn read_body(reader: &mut impl BufRead, framing: Framing) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    match framing {
        Framing::Empty => {}
        Framing::Length(length) => {
            if length > BODY_LIMIT {
                return Err(invalid("message body exceeds its size limit"));
            }
            body.resize(length, 0);
            reader.read_exact(&mut body)?;
        }
        Framing::UntilClose => {
            reader
                .by_ref()
                .take(BODY_LIMIT as u64 + 1)
                .read_to_end(&mut body)?;
            if body.len() > BODY_LIMIT {
                return Err(invalid("message body exceeds its size limit"));
            }
        }
        Framing::Chunked => loop {
            let line = read_line(reader, &mut body)?;
            let size = line
                .split(';')
                .next()
                .map(str::trim)
                .and_then(|size| usize::from_str_radix(size, 16).ok())
                .ok_or_else(|| invalid("malformed chunk size"))?;
            if body.len().saturating_add(size) > BODY_LIMIT {
                return Err(invalid("message body exceeds its size limit"));
            }
            if size == 0 {
                while !read_line(reader, &mut body)?.is_empty() {}
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            if !read_line(reader, &mut body)?.is_empty() {
                return Err(invalid("chunk data is not followed by CRLF"));
            }
        },
    }
    Ok(body)
}

/// Appends one raw line to `raw` and returns it without its line ending.
fn read_line(reader: &mut impl BufRead, raw: &mut Vec<u8>) -> io::Result<String> {
    let mut line = Vec::new();
    reader
        .by_ref()
        .take(HEAD_LIMIT as u64)
        .read_until(b'\n', &mut line)?;
    if !line.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed inside chunk framing",
        ));
    }
    raw.extend_from_slice(&line);
    let text = String::from_utf8(line).map_err(|_| invalid("chunk framing is not UTF-8"))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

/// Splits a request target into its path and query parameter names.
/// Query values are discarded so that recorded paths never contain credentials.
pub fn path_and_query_keys(target: &str) -> (String, Vec<String>) {
    let origin = match target.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("/", |index| &rest[index..]),
        None => target,
    };
    let origin = origin.split_once('#').map_or(origin, |(path, _)| path);
    match origin.split_once('?') {
        Some((path, query)) => (
            path.to_owned(),
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key).to_owned())
                .collect(),
        ),
        None => (origin.to_owned(), Vec::new()),
    }
}

/// Methods that cannot change provider state.
pub fn is_mutation(method: &str) -> bool {
    !["GET", "HEAD", "OPTIONS", "TRACE"]
        .iter()
        .any(|safe| method.eq_ignore_ascii_case(safe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn chunked_bodies_keep_their_transmitted_bytes() {
        let raw = b"4\r\nabcd\r\n3;ext=1\r\nefg\r\n0\r\nTrailer: x\r\n\r\nNEXT";
        let mut reader = Cursor::new(&raw[..]);
        let body = read_body(&mut reader, Framing::Chunked).unwrap();
        assert_eq!(body, &raw[..raw.len() - 4]);
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "NEXT");
    }

    #[test]
    fn response_framing_follows_method_and_status() {
        let head = |start: &str, headers: &[(&str, &str)]| Head {
            start: start.to_owned(),
            headers: headers
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        };
        let length = head("HTTP/1.1 200 OK", &[("Content-Length", "3")]);
        assert_eq!(length.response_framing("GET").unwrap(), Framing::Length(3));
        assert_eq!(length.response_framing("HEAD").unwrap(), Framing::Empty);
        let chunked = head(
            "HTTP/1.1 201 Created",
            &[("Transfer-Encoding", "gzip, chunked")],
        );
        assert_eq!(chunked.response_framing("POST").unwrap(), Framing::Chunked);
        assert_eq!(
            head("HTTP/1.1 204 No Content", &[])
                .response_framing("DELETE")
                .unwrap(),
            Framing::Empty
        );
        assert_eq!(
            head("HTTP/1.1 200 OK", &[])
                .response_framing("GET")
                .unwrap(),
            Framing::UntilClose
        );
        assert!(
            head("HTTP/1.1 200 OK", &[("Content-Length", "x")])
                .response_framing("GET")
                .is_err()
        );
    }

    #[test]
    fn single_exchange_heads_drop_hop_by_hop_headers() {
        let head = Head {
            start: "GET / HTTP/1.1".to_owned(),
            headers: vec![
                ("Host".to_owned(), "127.0.0.1:8931".to_owned()),
                (
                    "Connection".to_owned(),
                    "keep-alive, X-Private-Hop".to_owned(),
                ),
                ("X-Private-Hop".to_owned(), "1".to_owned()),
                ("Expect".to_owned(), "100-continue".to_owned()),
                ("PRIVATE-TOKEN".to_owned(), "kept".to_owned()),
            ],
        };
        assert_eq!(
            head.for_single_exchange().headers,
            vec![
                ("Host".to_owned(), "127.0.0.1:8931".to_owned()),
                ("PRIVATE-TOKEN".to_owned(), "kept".to_owned()),
                ("Connection".to_owned(), "close".to_owned()),
            ]
        );
    }

    #[test]
    fn recorded_targets_keep_only_query_names() {
        assert_eq!(
            path_and_query_keys("/api/v4/user?private_token=secret&page=2&flag"),
            (
                "/api/v4/user".to_owned(),
                vec![
                    "private_token".to_owned(),
                    "page".to_owned(),
                    "flag".to_owned()
                ]
            )
        );
        assert_eq!(
            path_and_query_keys("http://127.0.0.1:8931/api/v4/version"),
            ("/api/v4/version".to_owned(), Vec::new())
        );
        assert!(is_mutation("POST") && is_mutation("delete") && !is_mutation("get"));
    }
}
