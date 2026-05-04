mod blocking;
mod http;

#[cfg(target_os = "linux")]
mod iouring;

use crate::config::Config;
use crate::index::Index;
use crate::mcc;

/// Entry point. Tries io_uring on Linux first; on ENOSYS (qemu emulation,
/// stripped kernel) or on any other init failure, falls back to a blocking
/// thread-per-connection model. The fallback can also be forced with
/// `SERVER_MODE=blocking` for local testing.
pub fn run(cfg: &Config, idx: &'static Index, mcc_table: &'static mcc::Table) -> Result<(), String> {
    let mode = std::env::var("SERVER_MODE").unwrap_or_default();
    if mode == "blocking" {
        return blocking::run(cfg, idx, mcc_table);
    }

    #[cfg(target_os = "linux")]
    {
        match iouring::run(cfg, idx, mcc_table) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("io_uring init failed ({e}); falling back to blocking server");
            }
        }
    }

    blocking::run(cfg, idx, mcc_table)
}
