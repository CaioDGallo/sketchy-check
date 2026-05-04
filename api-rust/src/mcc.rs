// MCC (Merchant Category Code) → risk lookup. Mirrors api/internal/mcc/mcc.go:
// 10000-entry table indexed by 4-digit MCC; returns DEFAULT_RISK for unknown
// codes. Built-in defaults override anything missing from mcc_risk.json.

use std::fs;

pub const DEFAULT_RISK: f32 = 0.5;

pub struct Table {
    risks: [f32; 10000],
    set: [bool; 10000],
}

impl Table {
    pub fn load(path: &str) -> Result<Self, String> {
        let mut t = Self::with_defaults();
        match fs::read(path) {
            Ok(bytes) => t.merge_json(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("read {path}: {e}")),
        }
        Ok(t)
    }

    pub fn with_defaults() -> Self {
        let mut t = Self {
            risks: [0.0; 10000],
            set: [false; 10000],
        };
        t.set_one(5411, 0.15);
        t.set_one(5812, 0.30);
        t.set_one(5912, 0.20);
        t.set_one(5944, 0.45);
        t.set_one(7801, 0.80);
        t.set_one(7802, 0.75);
        t.set_one(7995, 0.85);
        t.set_one(4511, 0.35);
        t.set_one(5311, 0.25);
        t.set_one(5999, 0.50);
        t
    }

    fn set_one(&mut self, code: usize, risk: f32) {
        self.risks[code] = risk;
        self.set[code] = true;
    }

    /// Look up a 4-byte MCC code. Returns DEFAULT_RISK if the bytes aren't
    /// 4 ASCII digits or the code isn't in the table.
    #[inline]
    pub fn get(&self, mcc: &[u8]) -> f32 {
        match code4(mcc) {
            Some(c) if self.set[c] => self.risks[c],
            _ => DEFAULT_RISK,
        }
    }

    /// Parse the JSON object {"mcc_code": risk, ...}. The schema is fixed —
    /// only top-level string→number entries are recognized. Anything else is
    /// silently skipped (matches the Go behavior on unknown keys).
    fn merge_json(&mut self, data: &[u8]) -> Result<(), String> {
        let mut i = skip_ws(data, 0);
        if i >= data.len() || data[i] != b'{' {
            return Err("mcc_risk.json: expected object".into());
        }
        i += 1;
        loop {
            i = skip_ws(data, i);
            if i >= data.len() {
                return Err("mcc_risk.json: unterminated".into());
            }
            if data[i] == b'}' {
                return Ok(());
            }
            if data[i] != b'"' {
                return Err(format!("mcc_risk.json: expected key at {i}"));
            }
            let key_start = i + 1;
            let mut key_end = key_start;
            while key_end < data.len() && data[key_end] != b'"' {
                key_end += 1;
            }
            if key_end >= data.len() {
                return Err("mcc_risk.json: unterminated key".into());
            }
            let key = &data[key_start..key_end];
            i = skip_ws(data, key_end + 1);
            if i >= data.len() || data[i] != b':' {
                return Err("mcc_risk.json: expected colon".into());
            }
            i = skip_ws(data, i + 1);
            let val_start = i;
            while i < data.len() {
                let c = data[i];
                if c == b',' || c == b'}' || c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    break;
                }
                i += 1;
            }
            let raw = std::str::from_utf8(&data[val_start..i])
                .map_err(|_| "mcc_risk.json: bad utf8 in value".to_string())?;
            let risk: f32 = raw
                .parse()
                .map_err(|_| format!("mcc_risk.json: bad number {raw:?}"))?;
            if let Some(code) = code4(key) {
                self.risks[code] = risk;
                self.set[code] = true;
            }
            i = skip_ws(data, i);
            if i < data.len() && data[i] == b',' {
                i += 1;
            }
        }
    }
}

#[inline]
fn code4(mcc: &[u8]) -> Option<usize> {
    if mcc.len() < 4 {
        return None;
    }
    let (c0, c1, c2, c3) = (mcc[0], mcc[1], mcc[2], mcc[3]);
    if !c0.is_ascii_digit()
        || !c1.is_ascii_digit()
        || !c2.is_ascii_digit()
        || !c3.is_ascii_digit()
    {
        return None;
    }
    Some(
        (c0 - b'0') as usize * 1000
            + (c1 - b'0') as usize * 100
            + (c2 - b'0') as usize * 10
            + (c3 - b'0') as usize,
    )
}

#[inline]
fn skip_ws(data: &[u8], mut i: usize) -> usize {
    while i < data.len() {
        match data[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => break,
        }
    }
    i
}
