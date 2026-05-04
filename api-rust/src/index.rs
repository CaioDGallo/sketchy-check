// IVF6 binary index loader. Format matches api/internal/ivf/index.go:
//
//   magic "IVF6"      : [u8; 4]
//   N, K, D, stride   : u32 le
//   scale             : f32 le         (must equal FIX_SCALE = 10000)
//   centroids         : K*D f32 le
//   bbox_min          : K*D i16 le
//   bbox_max          : K*D i16 le
//   offsets           : (K+1) u32 le
//   vectors           : N*D i16 le     (row-major; transposed at load time)
//   labels            : N u8
//   orig_ids          : N u32 le

use std::fs::File;
use std::io::{BufReader, Read};

pub const D: usize = 14;
pub const K: usize = 256;
pub const FIX_SCALE: f32 = 10000.0;

pub struct Index {
    pub n: usize,
    pub dims_buf: Vec<i16>,
    pub labels: Vec<u8>,
    pub orig_ids: Vec<u32>,
    pub centroids: Vec<f32>,
    pub bbox_min: Vec<i16>,
    pub bbox_max: Vec<i16>,
    pub offsets: [u32; K + 1],
}

impl Index {
    pub fn load(path: &str) -> Result<Self, String> {
        let f = File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        let mut br = BufReader::with_capacity(1 << 20, f);

        let mut magic = [0u8; 4];
        read_exact(&mut br, &mut magic, "magic")?;
        if &magic != b"IVF6" {
            return Err(format!("bad magic: {magic:?}, want IVF6"));
        }

        let n = read_u32(&mut br, "N")? as usize;
        let k = read_u32(&mut br, "K")?;
        let d = read_u32(&mut br, "D")?;
        let stride = read_u32(&mut br, "stride")?;
        let scale = read_f32(&mut br, "scale")?;
        if k as usize != K || d as usize != D || stride as usize != D {
            return Err(format!(
                "incompatible index: K={k} D={d} stride={stride} (expected K={K} D={D})"
            ));
        }
        if (scale - FIX_SCALE).abs() > 0.01 {
            return Err(format!("incompatible scale: {scale}, want {FIX_SCALE}"));
        }

        let mut centroids = vec![0f32; K * D];
        read_f32_slice(&mut br, &mut centroids, "centroids")?;
        let mut bbox_min = vec![0i16; K * D];
        read_i16_slice(&mut br, &mut bbox_min, "bbox_min")?;
        let mut bbox_max = vec![0i16; K * D];
        read_i16_slice(&mut br, &mut bbox_max, "bbox_max")?;

        let mut offsets = [0u32; K + 1];
        for slot in offsets.iter_mut() {
            *slot = read_u32(&mut br, "offsets")?;
        }

        // Vectors: row-major on disk, transpose into Dim[j][i] = vector i's dim j.
        // dims_buf is one contiguous N*D buffer; Dim[j] = dims_buf[j*N..(j+1)*N].
        let mut dims_buf = vec![0i16; D * n];
        const CHUNK: usize = 16384;
        let mut tmp = vec![0i16; CHUNK * D];
        let mut done = 0usize;
        while done < n {
            let take = (n - done).min(CHUNK);
            let buf = &mut tmp[..take * D];
            read_i16_slice(&mut br, buf, "vectors")?;
            for i in 0..take {
                let row = &buf[i * D..(i + 1) * D];
                for j in 0..D {
                    dims_buf[j * n + (done + i)] = row[j];
                }
            }
            done += take;
        }

        let mut labels = vec![0u8; n];
        read_exact(&mut br, &mut labels, "labels")?;
        let mut orig_ids = vec![0u32; n];
        read_u32_slice(&mut br, &mut orig_ids, "orig_ids")?;

        Ok(Self {
            n,
            dims_buf,
            labels,
            orig_ids,
            centroids,
            bbox_min,
            bbox_max,
            offsets,
        })
    }

    /// Returns Dim[j] — the slice of all N records' j-th dimension.
    #[inline]
    pub fn dim(&self, j: usize) -> &[i16] {
        &self.dims_buf[j * self.n..(j + 1) * self.n]
    }
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8], what: &str) -> Result<(), String> {
    r.read_exact(buf).map_err(|e| format!("read {what}: {e}"))
}

fn read_u32<R: Read>(r: &mut R, what: &str) -> Result<u32, String> {
    let mut b = [0u8; 4];
    read_exact(r, &mut b, what)?;
    Ok(u32::from_le_bytes(b))
}

fn read_f32<R: Read>(r: &mut R, what: &str) -> Result<f32, String> {
    let mut b = [0u8; 4];
    read_exact(r, &mut b, what)?;
    Ok(f32::from_le_bytes(b))
}

fn read_f32_slice<R: Read>(r: &mut R, dst: &mut [f32], what: &str) -> Result<(), String> {
    // SAFETY: f32 has the same layout as [u8; 4]; we only treat the storage
    // as bytes for the read, then byte-swap if needed (we are little-endian
    // on x86_64 and aarch64 macOS — this is a no-op on those hosts).
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u8, std::mem::size_of_val(dst))
    };
    read_exact(r, bytes, what)?;
    if cfg!(target_endian = "big") {
        for v in dst.iter_mut() {
            *v = f32::from_le_bytes(v.to_be_bytes());
        }
    }
    Ok(())
}

fn read_i16_slice<R: Read>(r: &mut R, dst: &mut [i16], what: &str) -> Result<(), String> {
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u8, std::mem::size_of_val(dst))
    };
    read_exact(r, bytes, what)?;
    if cfg!(target_endian = "big") {
        for v in dst.iter_mut() {
            *v = i16::from_le_bytes(v.to_be_bytes());
        }
    }
    Ok(())
}

fn read_u32_slice<R: Read>(r: &mut R, dst: &mut [u32], what: &str) -> Result<(), String> {
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u8, std::mem::size_of_val(dst))
    };
    read_exact(r, bytes, what)?;
    if cfg!(target_endian = "big") {
        for v in dst.iter_mut() {
            *v = u32::from_le_bytes(v.to_be_bytes());
        }
    }
    Ok(())
}
