// Pre-built HTTP/1.1 responses. Six fraud-score bodies (one per top-5 fraud
// vote count 0..5) and a small set of error responses. Built once at startup,
// emitted as raw byte slices on the hot path. Body shape is byte-identical to
// api/internal/responses/responses.go.

use std::sync::OnceLock;

static FRAUD_SCORE: OnceLock<[Vec<u8>; 6]> = OnceLock::new();
static READY: OnceLock<Vec<u8>> = OnceLock::new();
static NOT_FOUND: OnceLock<Vec<u8>> = OnceLock::new();
static BAD_REQUEST: OnceLock<Vec<u8>> = OnceLock::new();
static TOO_LARGE: OnceLock<Vec<u8>> = OnceLock::new();
static SERVER_ERROR: OnceLock<Vec<u8>> = OnceLock::new();

pub fn init() {
    let _ = FRAUD_SCORE.set([
        build200(0),
        build200(1),
        build200(2),
        build200(3),
        build200(4),
        build200(5),
    ]);
    let _ = READY.set(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n".to_vec(),
    );
    let _ = NOT_FOUND.set(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    );
    let _ = BAD_REQUEST.set(
        b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"error\":\"invalid_payload\"}".to_vec(),
    );
    let _ = TOO_LARGE.set(
        b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    );
    let _ = SERVER_ERROR.set(
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    );
}

#[inline]
pub fn fraud_score(frauds: usize) -> &'static [u8] {
    FRAUD_SCORE.get().expect("responses::init() not called")[frauds].as_slice()
}

#[inline]
pub fn ready() -> &'static [u8] {
    READY.get().expect("responses::init() not called").as_slice()
}

#[inline]
pub fn not_found() -> &'static [u8] {
    NOT_FOUND.get().expect("responses::init() not called").as_slice()
}

#[inline]
pub fn bad_request() -> &'static [u8] {
    BAD_REQUEST.get().expect("responses::init() not called").as_slice()
}

#[inline]
pub fn too_large() -> &'static [u8] {
    TOO_LARGE.get().expect("responses::init() not called").as_slice()
}

#[inline]
pub fn server_error() -> &'static [u8] {
    SERVER_ERROR.get().expect("responses::init() not called").as_slice()
}

fn build200(frauds: usize) -> Vec<u8> {
    let score = frauds as f32 * 0.2;
    let approved = if frauds < 3 { "true" } else { "false" };
    // Match Go's `%.4f` formatting.
    let body = format!(r#"{{"approved":{approved},"fraud_score":{score:.4}}}"#);
    let mut out = Vec::with_capacity(128 + body.len());
    out.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ");
    out.extend_from_slice(body.len().to_string().as_bytes());
    out.extend_from_slice(b"\r\nConnection: keep-alive\r\n\r\n");
    out.extend_from_slice(body.as_bytes());
    out
}
