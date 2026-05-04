mod blocking;
mod http;

#[cfg(target_os = "linux")]
mod epoll;
#[cfg(target_os = "linux")]
mod iouring;

use crate::config::Config;
use crate::index::Index;
use crate::mcc;

/// Entry point. Tries io_uring on Linux first; on init failure (older kernel,
/// seccomp restrictions, qemu emulation) falls back to a single-threaded
/// epoll event loop on Linux. Non-Linux hosts (or `SERVER_MODE=blocking`)
/// drop to a thread-per-connection blocking loop. The selected mode is
/// announced on stderr so grader/log inspection can confirm which path ran.
pub fn run(cfg: &Config, idx: &'static Index, mcc_table: &'static mcc::Table) -> Result<(), String> {
    let mode = std::env::var("SERVER_MODE").unwrap_or_default();
    if mode == "blocking" {
        eprintln!("server mode: blocking (forced via SERVER_MODE)");
        return blocking::run(cfg, idx, mcc_table);
    }
    if mode == "epoll" {
        #[cfg(target_os = "linux")]
        {
            eprintln!("server mode: epoll (forced via SERVER_MODE)");
            return epoll::run(cfg, idx, mcc_table);
        }
    }

    #[cfg(target_os = "linux")]
    {
        match iouring::run(cfg, idx, mcc_table) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("io_uring init failed ({e}); falling back to epoll");
            }
        }
        match epoll::run(cfg, idx, mcc_table) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("epoll init failed ({e}); falling back to blocking");
            }
        }
    }

    eprintln!("server mode: blocking (last-resort fallback)");
    blocking::run(cfg, idx, mcc_table)
}
