// Blocking thread-per-connection fallback. Used when io_uring isn't
// available (e.g. macOS Docker Desktop running an amd64 image under
// qemu emulation, where io_uring_setup returns ENOSYS). Slower than the
// io_uring loop on real hardware but byte-identical results, so smoke tests
// can run anywhere.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;

use crate::config::Config;
use crate::index::Index;
use crate::mcc;
use crate::server::http::{self, Done};

const REQ_BUF_SIZE: usize = 16 * 1024;

pub fn run(cfg: &Config, idx: &'static Index, mcc_table: &'static mcc::Table) -> Result<(), String> {
    let _ = std::fs::remove_file(&cfg.uds_path);
    let listener = UnixListener::bind(&cfg.uds_path)
        .map_err(|e| format!("bind {}: {e}", cfg.uds_path))?;
    std::fs::set_permissions(&cfg.uds_path, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| format!("chmod {}: {e}", cfg.uds_path))?;

    let nprobe = cfg.ivf_nprobe;
    eprintln!(
        "blocking-mode listening on {} (no io_uring; threads spawned per conn)",
        cfg.uds_path
    );

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                thread::spawn(move || {
                    handle(s, idx, mcc_table, nprobe);
                });
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => continue,
        }
    }
    Ok(())
}

fn handle(mut s: UnixStream, idx: &Index, mcc_table: &mcc::Table, nprobe: u32) {
    let mut buf = vec![0u8; REQ_BUF_SIZE];
    let mut used: usize = 0;
    let mut q = [0f32; 14];

    loop {
        let n = match s.read(&mut buf[used..]) {
            Ok(0) => return,
            Ok(n) => n,
            Err(_) => return,
        };
        used += n;
        loop {
            let is_full = used >= buf.len() - 1;
            match http::process(&buf[..used], is_full, &mut q, idx, mcc_table, nprobe) {
                None => break,
                Some(Done { response, close }) => {
                    if s.write_all(response).is_err() {
                        return;
                    }
                    if close {
                        return;
                    }
                    used = 0;
                    break;
                }
            }
        }
    }
}
