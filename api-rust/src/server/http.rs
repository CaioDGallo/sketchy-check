// HTTP/1.1 request dispatch. Mirrors api/internal/server/server.go's hot
// loop: find headers, classify request line, parse Content-Length, vectorize
// body, search index, pick pre-built response.

use crate::index::Index;
use crate::kernel;
use crate::mcc;
use crate::responses;
use crate::vectorize;

pub struct Done {
    pub response: &'static [u8],
    pub close: bool,
}

/// Process one request from `bytes`. Returns `None` if more data is needed,
/// `Some(Done)` once a complete response has been chosen. `is_full` is true
/// when the caller's read buffer is already at capacity and cannot grow.
pub fn process(
    bytes: &[u8],
    is_full: bool,
    q: &mut [f32; 14],
    idx: &Index,
    mcc_table: &mcc::Table,
    nprobe: u32,
) -> Option<Done> {
    let header_end = match find_header_end(bytes) {
        Some(e) => e,
        None => {
            if is_full {
                return Some(Done {
                    response: responses::too_large(),
                    close: true,
                });
            }
            return None;
        }
    };
    let body_start = header_end + 4;

    let line_end = match find_crlf(&bytes[..header_end]) {
        Some(e) => e,
        None => {
            return Some(Done {
                response: responses::bad_request(),
                close: true,
            });
        }
    };
    let line = &bytes[..line_end];

    if line.starts_with(b"GET /ready") {
        return Some(Done {
            response: responses::ready(),
            close: false,
        });
    }
    if !line.starts_with(b"POST /fraud-score") {
        return Some(Done {
            response: responses::not_found(),
            close: true,
        });
    }

    let cl = match parse_content_length(&bytes[line_end + 2..header_end]) {
        Some(n) => n,
        None => {
            return Some(Done {
                response: responses::bad_request(),
                close: true,
            });
        }
    };
    let max_body = bytes.len() - body_start;
    if cl > max_body {
        if is_full {
            return Some(Done {
                response: responses::too_large(),
                close: true,
            });
        }
        return None;
    }
    if bytes.len() < body_start + cl {
        return None;
    }

    let body = &bytes[body_start..body_start + cl];
    if vectorize::build(body, q, mcc_table).is_err() {
        return Some(Done {
            response: responses::bad_request(),
            close: true,
        });
    }
    let frauds = kernel::search_frauds(idx, q, nprobe);
    if !(0..=5).contains(&frauds) {
        return Some(Done {
            response: responses::server_error(),
            close: true,
        });
    }
    Some(Done {
        response: responses::fraud_score(frauds as usize),
        close: false,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let mut i = 0;
    let last = buf.len() - 4;
    while i <= last {
        if buf[i] == b'\r'
            && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r'
            && buf[i + 3] == b'\n'
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    let mut i = 0;
    let last = buf.len() - 2;
    while i <= last {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    const TARGET: &[u8] = b"content-length:";
    let mut h = headers;
    while !h.is_empty() {
        let line_end = match find_crlf(h) {
            Some(e) => e,
            None => h.len(),
        };
        let (line, rest) = if line_end < h.len() {
            (&h[..line_end], &h[line_end + 2..])
        } else {
            (h, &[][..])
        };
        h = rest;
        if line.len() < TARGET.len() {
            continue;
        }
        if !header_prefix_matches(line, TARGET) {
            continue;
        }
        let mut v = &line[TARGET.len()..];
        while !v.is_empty() && (v[0] == b' ' || v[0] == b'\t') {
            v = &v[1..];
        }
        while !v.is_empty() && (v[v.len() - 1] == b' ' || v[v.len() - 1] == b'\t') {
            v = &v[..v.len() - 1];
        }
        if v.is_empty() {
            return None;
        }
        let s = std::str::from_utf8(v).ok()?;
        return s.parse::<usize>().ok();
    }
    None
}

fn header_prefix_matches(line: &[u8], target: &[u8]) -> bool {
    if line.len() < target.len() {
        return false;
    }
    for i in 0..target.len() {
        let mut c = line[i];
        if (b'A'..=b'Z').contains(&c) {
            c += b'a' - b'A';
        }
        if c != target[i] {
            return false;
        }
    }
    true
}
