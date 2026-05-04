// Top5 — the running k=5 nearest-neighbor list. Layout and tie-break rule
// match api/internal/ivf/kernel/top5.go and kernel_amd64.c byte-for-byte.

#[derive(Clone, Copy)]
pub struct Top5 {
    pub best_d: [u64; 5],
    pub best_id: [u32; 5],
    pub best_l: [u8; 5],
    pub worst: usize,
    pub worst_d: u64,
    pub worst_id: u32,
}

impl Top5 {
    pub fn new() -> Self {
        Self {
            best_d: [u64::MAX; 5],
            best_id: [u32::MAX; 5],
            best_l: [0; 5],
            worst: 0,
            worst_d: u64::MAX,
            worst_id: u32::MAX,
        }
    }

    #[inline]
    pub fn frauds(&self) -> i32 {
        let mut n = 0;
        for i in 0..5 {
            if self.best_l[i] == 1 {
                n += 1;
            }
        }
        n
    }

    /// Insert `(d, label, oid)` if it beats the current worst slot. Tie-break:
    /// `(d, id)` lex order (smaller wins), matching the C/Go reference.
    #[inline]
    pub fn try_insert(&mut self, d: u64, label: u8, oid: u32) {
        if !is_better(d, oid, self.worst_d, self.worst_id) {
            return;
        }
        let w = self.worst;
        self.best_d[w] = d;
        self.best_l[w] = label;
        self.best_id[w] = oid;
        let nw = find_worst(&self.best_d, &self.best_id);
        self.worst = nw;
        self.worst_d = self.best_d[nw];
        self.worst_id = self.best_id[nw];
    }
}

#[inline]
pub fn is_better(da: u64, ia: u32, db: u64, ib: u32) -> bool {
    da < db || (da == db && ia < ib)
}

#[inline]
pub fn is_worse(da: u64, ia: u32, db: u64, ib: u32) -> bool {
    da > db || (da == db && ia > ib)
}

#[inline]
fn find_worst(d: &[u64; 5], id: &[u32; 5]) -> usize {
    let mut w = 0;
    if is_worse(d[1], id[1], d[w], id[w]) {
        w = 1;
    }
    if is_worse(d[2], id[2], d[w], id[w]) {
        w = 2;
    }
    if is_worse(d[3], id[3], d[w], id[w]) {
        w = 3;
    }
    if is_worse(d[4], id[4], d[w], id[w]) {
        w = 4;
    }
    w
}
