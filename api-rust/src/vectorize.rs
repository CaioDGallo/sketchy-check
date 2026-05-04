// Manual JSON parser → 14-dim feature vector. Direct port of
// api/internal/vectorize/vectorize.go. Same key names, same normalization,
// same date arithmetic — must produce byte-identical f32 output to the Go
// version (otherwise the IVF top-5 diverges and detection drops).

use crate::mcc;

const MAX_AMOUNT: f32 = 10000.0;
const MAX_INSTALLMENTS: f32 = 12.0;
const AMOUNT_VS_AVG_RATIO: f32 = 10.0;
const MAX_MINUTES: f32 = 1440.0;
const MAX_KM: f32 = 1000.0;
const MAX_TX_COUNT_24H: f32 = 20.0;
const MAX_MERCHANT_AVG_AMOUNT: f32 = 10000.0;

const KEY_TRANSACTION: &[u8] = b"\"transaction\"";
const KEY_CUSTOMER: &[u8] = b"\"customer\"";
const KEY_MERCHANT: &[u8] = b"\"merchant\"";
const KEY_TERMINAL: &[u8] = b"\"terminal\"";
const KEY_LAST_TRANSACTION: &[u8] = b"\"last_transaction\"";
const KEY_AMOUNT: &[u8] = b"\"amount\"";
const KEY_INSTALLMENTS: &[u8] = b"\"installments\"";
const KEY_REQUESTED_AT: &[u8] = b"\"requested_at\"";
const KEY_AVG_AMOUNT: &[u8] = b"\"avg_amount\"";
const KEY_TX_COUNT_24H: &[u8] = b"\"tx_count_24h\"";
const KEY_KNOWN_MERCHANTS: &[u8] = b"\"known_merchants\"";
const KEY_ID: &[u8] = b"\"id\"";
const KEY_MCC: &[u8] = b"\"mcc\"";
const KEY_IS_ONLINE: &[u8] = b"\"is_online\"";
const KEY_CARD_PRESENT: &[u8] = b"\"card_present\"";
const KEY_KM_FROM_HOME: &[u8] = b"\"km_from_home\"";
const KEY_TIMESTAMP: &[u8] = b"\"timestamp\"";
const KEY_KM_FROM_CURRENT: &[u8] = b"\"km_from_current\"";

#[derive(Debug)]
pub struct ParseError;

pub fn build(body: &[u8], q: &mut [f32; 14], mcc_table: &mcc::Table) -> Result<(), ParseError> {
    let (tx_start, tx_end) = object_range(body, 0, body.len(), KEY_TRANSACTION).ok_or(ParseError)?;
    let (cust_start, cust_end) =
        object_range(body, 0, body.len(), KEY_CUSTOMER).ok_or(ParseError)?;
    let (merch_start, merch_end) =
        object_range(body, 0, body.len(), KEY_MERCHANT).ok_or(ParseError)?;
    let (term_start, term_end) =
        object_range(body, 0, body.len(), KEY_TERMINAL).ok_or(ParseError)?;

    let amount = json_number(body, tx_start, tx_end, KEY_AMOUNT).ok_or(ParseError)?;
    let installments = json_number(body, tx_start, tx_end, KEY_INSTALLMENTS).ok_or(ParseError)?;
    let requested_at = json_string(body, tx_start, tx_end, KEY_REQUESTED_AT).ok_or(ParseError)?;
    if requested_at.len() < 19 {
        return Err(ParseError);
    }

    let customer_avg = json_number(body, cust_start, cust_end, KEY_AVG_AMOUNT).ok_or(ParseError)?;
    let tx24h = json_number(body, cust_start, cust_end, KEY_TX_COUNT_24H).ok_or(ParseError)?;

    let merchant_id = json_string(body, merch_start, merch_end, KEY_ID).ok_or(ParseError)?;
    let mcc_bytes = json_string(body, merch_start, merch_end, KEY_MCC).ok_or(ParseError)?;
    let merchant_avg =
        json_number(body, merch_start, merch_end, KEY_AVG_AMOUNT).ok_or(ParseError)?;

    let is_online = json_bool(body, term_start, term_end, KEY_IS_ONLINE).ok_or(ParseError)?;
    let card_present = json_bool(body, term_start, term_end, KEY_CARD_PRESENT).ok_or(ParseError)?;
    let km_from_home =
        json_number(body, term_start, term_end, KEY_KM_FROM_HOME).ok_or(ParseError)?;

    let mut minutes_since_last: f32 = -1.0;
    let mut km_from_last: f32 = -1.0;
    if let Some((lt_start, lt_end)) = object_range(body, 0, body.len(), KEY_LAST_TRANSACTION) {
        let last_ts = json_string(body, lt_start, lt_end, KEY_TIMESTAMP).ok_or(ParseError)?;
        if last_ts.len() < 19 {
            return Err(ParseError);
        }
        let km_from_current =
            json_number(body, lt_start, lt_end, KEY_KM_FROM_CURRENT).ok_or(ParseError)?;
        let mins = minutes_between(requested_at, last_ts);
        minutes_since_last = clamp01(mins as f32 / MAX_MINUTES);
        km_from_last = clamp01(km_from_current / MAX_KM);
    }

    let known = array_contains_string(body, cust_start, cust_end, KEY_KNOWN_MERCHANTS, merchant_id);
    let unknown_merchant: f32 = if known { 0.0 } else { 1.0 };

    let amount_vs_avg = if customer_avg > 0.0 {
        clamp01((amount / customer_avg) / AMOUNT_VS_AVG_RATIO)
    } else {
        1.0
    };

    q[0] = clamp01(amount / MAX_AMOUNT);
    q[1] = clamp01(installments / MAX_INSTALLMENTS);
    q[2] = amount_vs_avg;
    q[3] = clamp01(iso_hour_utc(requested_at) as f32 / 23.0);
    q[4] = clamp01(weekday_monday_zero(requested_at) as f32 / 6.0);
    q[5] = minutes_since_last;
    q[6] = km_from_last;
    q[7] = clamp01(km_from_home / MAX_KM);
    q[8] = clamp01(tx24h / MAX_TX_COUNT_24H);
    q[9] = if is_online { 1.0 } else { 0.0 };
    q[10] = if card_present { 1.0 } else { 0.0 };
    q[11] = unknown_merchant;
    q[12] = mcc_table.get(mcc_bytes);
    q[13] = clamp01(merchant_avg / MAX_MERCHANT_AVG_AMOUNT);
    Ok(())
}

#[inline]
fn clamp01(x: f32) -> f32 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

fn find_key(body: &[u8], start: usize, end: usize, key: &[u8]) -> Option<usize> {
    if start >= end {
        return None;
    }
    memmem(&body[start..end], key).map(|p| start + p)
}

fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let first = needle[0];
    let mut i = 0;
    while i <= last {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn index_byte_from(body: &[u8], start: usize, end: usize, c: u8) -> Option<usize> {
    if start >= end {
        return None;
    }
    body[start..end].iter().position(|&b| b == c).map(|p| start + p)
}

fn skip_ws(body: &[u8], mut start: usize, end: usize) -> usize {
    while start < end {
        match body[start] {
            b' ' | b'\t' | b'\n' | b'\r' => start += 1,
            _ => return start,
        }
    }
    end
}

fn match_brace(body: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut esc = false;
    let mut p = start;
    while p < end {
        let c = body[p];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            p += 1;
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(p + 1);
                }
            }
            _ => {}
        }
        p += 1;
    }
    None
}

fn object_range(body: &[u8], start: usize, end: usize, key: &[u8]) -> Option<(usize, usize)> {
    let k = find_key(body, start, end, key)?;
    let colon = index_byte_from(body, k + key.len(), end, b':')?;
    let p = skip_ws(body, colon + 1, end);
    if p >= end || body[p] != b'{' {
        return None;
    }
    let close = match_brace(body, p, end)?;
    Some((p, close))
}

fn json_number(body: &[u8], start: usize, end: usize, key: &[u8]) -> Option<f32> {
    let k = find_key(body, start, end, key)?;
    let colon = index_byte_from(body, k + key.len(), end, b':')?;
    let p = skip_ws(body, colon + 1, end);
    if p >= end {
        return None;
    }
    let mut q = p;
    if body[q] == b'-' || body[q] == b'+' {
        q += 1;
    }
    while q < end {
        let c = body[q];
        let digit_or_sign = c.is_ascii_digit()
            || c == b'.'
            || c == b'e'
            || c == b'E'
            || c == b'-'
            || c == b'+';
        if !digit_or_sign {
            break;
        }
        q += 1;
    }
    if q == p {
        return None;
    }
    let raw = std::str::from_utf8(&body[p..q]).ok()?;
    raw.parse::<f32>().ok()
}

fn json_bool(body: &[u8], start: usize, end: usize, key: &[u8]) -> Option<bool> {
    let k = find_key(body, start, end, key)?;
    let colon = index_byte_from(body, k + key.len(), end, b':')?;
    let p = skip_ws(body, colon + 1, end);
    if p + 4 <= end && &body[p..p + 4] == b"true" {
        return Some(true);
    }
    if p + 5 <= end && &body[p..p + 5] == b"false" {
        return Some(false);
    }
    None
}

fn json_string<'a>(body: &'a [u8], start: usize, end: usize, key: &[u8]) -> Option<&'a [u8]> {
    let k = find_key(body, start, end, key)?;
    let colon = index_byte_from(body, k + key.len(), end, b':')?;
    let p = skip_ws(body, colon + 1, end);
    if p >= end || body[p] != b'"' {
        return None;
    }
    let p = p + 1;
    let mut q = p;
    while q < end && body[q] != b'"' {
        q += 1;
    }
    if q >= end {
        return None;
    }
    Some(&body[p..q])
}

fn array_contains_string(
    body: &[u8],
    start: usize,
    end: usize,
    key: &[u8],
    needle: &[u8],
) -> bool {
    let Some(k) = find_key(body, start, end, key) else {
        return false;
    };
    let Some(colon) = index_byte_from(body, k + key.len(), end, b':') else {
        return false;
    };
    let Some(lb) = index_byte_from(body, colon + 1, end, b'[') else {
        return false;
    };
    let mut p = lb + 1;
    while p < end {
        p = skip_ws(body, p, end);
        if p >= end || body[p] == b']' {
            return false;
        }
        if body[p] != b'"' {
            p += 1;
            continue;
        }
        p += 1;
        let mut q = p;
        while q < end && body[q] != b'"' {
            q += 1;
        }
        if q >= end {
            return false;
        }
        if &body[p..q] == needle {
            return true;
        }
        p = q + 1;
    }
    false
}

fn iso_hour_utc(ts: &[u8]) -> i32 {
    let h = (ts[11] - b'0') as i32 * 10 + (ts[12] - b'0') as i32;
    if h < 0 {
        0
    } else if h > 23 {
        23
    } else {
        h
    }
}

fn weekday_monday_zero(ts: &[u8]) -> i32 {
    let y = (ts[0] - b'0') as i32 * 1000
        + (ts[1] - b'0') as i32 * 100
        + (ts[2] - b'0') as i32 * 10
        + (ts[3] - b'0') as i32;
    let m = (ts[5] - b'0') as i32 * 10 + (ts[6] - b'0') as i32;
    let d = (ts[8] - b'0') as i32 * 10 + (ts[9] - b'0') as i32;
    let days = days_from_civil(y, m, d);
    let mut w = ((days + 3) % 7) as i32;
    if w < 0 {
        w += 7;
    }
    w
}

fn minutes_between(a: &[u8], b: &[u8]) -> i64 {
    let sa = iso_to_epoch_seconds(a);
    let sb = iso_to_epoch_seconds(b);
    let mut d = sa - sb;
    if d < 0 {
        d = -d;
    }
    d / 60
}

fn iso_to_epoch_seconds(ts: &[u8]) -> i64 {
    let y = (ts[0] - b'0') as i32 * 1000
        + (ts[1] - b'0') as i32 * 100
        + (ts[2] - b'0') as i32 * 10
        + (ts[3] - b'0') as i32;
    let m = (ts[5] - b'0') as i32 * 10 + (ts[6] - b'0') as i32;
    let d = (ts[8] - b'0') as i32 * 10 + (ts[9] - b'0') as i32;
    let hh = (ts[11] - b'0') as i64 * 10 + (ts[12] - b'0') as i64;
    let mm = (ts[14] - b'0') as i64 * 10 + (ts[15] - b'0') as i64;
    let ss = (ts[17] - b'0') as i64 * 10 + (ts[18] - b'0') as i64;
    let days = days_from_civil(y, m, d);
    days * 86400 + hh * 3600 + mm * 60 + ss
}

// Howard Hinnant's "days from civil" — pure integer date math, identical to
// the Go implementation in vectorize.go.
fn days_from_civil(mut y: i32, m: i32, d: i32) -> i64 {
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = (y - era * 400) as i64; // 0..=399
    let month_shift: i64 = if m > 2 { -3 } else { 9 };
    let doy = (153 * (m as i64 + month_shift) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe - 719468
}
