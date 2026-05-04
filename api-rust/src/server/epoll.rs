// Single-threaded epoll event loop. Used as a Linux fallback when io_uring
// init fails (older kernels, seccomp restrictions, resource limits). Matches
// the Go runtime's goroutine-on-epoll model with a tighter inner loop —
// every fd is non-blocking + edge-triggered, so the kernel only wakes us
// when there's real work.
//
// Per-connection state lives in a pre-allocated CONNS array indexed by the
// epoll_event.u64 user data; that gives us O(1) dispatch with no per-event
// allocation.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::os::unix::net::UnixListener;

use crate::config::Config;
use crate::index::Index;
use crate::mcc;
use crate::server::http::{self, Done};

const REQ_BUF_SIZE: usize = 16 * 1024;
const MAX_EVENTS: usize = 256;
const LISTENER_TAG: u64 = u64::MAX;

#[repr(C)]
struct Conn {
    fd: i32,
    in_use: bool,
    close_after_write: bool,
    epoll_writable: bool,
    req_len: usize,
    res_ptr: *const u8,
    res_len: usize,
    res_sent: usize,
    req_buf: [u8; REQ_BUF_SIZE],
}

pub fn run(cfg: &Config, idx: &Index, mcc_table: &mcc::Table) -> Result<(), String> {
    let server_fd = bind_uds(&cfg.uds_path, cfg.backlog)?;
    set_nonblock(server_fd)?;

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return Err(format!("epoll_create1: {}", last_os_error()));
    }
    eprintln!(
        "server mode: epoll (single-threaded, max_conns={})",
        cfg.max_conns
    );

    epoll_add(epfd, server_fd, LISTENER_TAG, libc::EPOLLIN as u32)?;

    let mut conns = alloc_conns(cfg.max_conns);
    let n_conns = conns.len();
    let mut events: Vec<libc::epoll_event> = vec![
        libc::epoll_event { events: 0, u64: 0 };
        MAX_EVENTS
    ];
    let mut q = [0f32; 14];

    loop {
        let n = unsafe { libc::epoll_wait(epfd, events.as_mut_ptr(), MAX_EVENTS as i32, -1) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("epoll_wait: {err}"));
        }

        for ev in events.iter().take(n as usize) {
            let tag = ev.u64;
            let mask = ev.events;
            if tag == LISTENER_TAG {
                accept_loop(server_fd, epfd, &mut conns)?;
                continue;
            }
            let cidx = tag as usize;
            if cidx >= n_conns {
                continue;
            }
            if !conns[cidx].in_use {
                continue;
            }

            let mut should_close = false;
            if mask & (libc::EPOLLERR | libc::EPOLLHUP) as u32 != 0 {
                should_close = true;
            }

            if !should_close && (mask & libc::EPOLLIN as u32 != 0) {
                if read_loop(cidx, &mut conns, idx, mcc_table, cfg.ivf_nprobe, &mut q) {
                    should_close = true;
                }
            }

            // EPOLLOUT (or in-line write attempt after read parses a request)
            if !should_close && conns[cidx].res_len > conns[cidx].res_sent {
                if write_loop(cidx, &mut conns, epfd) {
                    should_close = true;
                }
            }

            if !should_close
                && conns[cidx].close_after_write
                && conns[cidx].res_sent >= conns[cidx].res_len
                && conns[cidx].res_len > 0
            {
                should_close = true;
            }

            if should_close {
                free_conn(epfd, &mut conns[cidx]);
            }
        }
    }
}

/// Returns true if the connection should be closed.
fn read_loop(
    cidx: usize,
    conns: &mut [Conn],
    idx: &Index,
    mcc_table: &mcc::Table,
    nprobe: u32,
    q: &mut [f32; 14],
) -> bool {
    loop {
        let c = &mut conns[cidx];
        if c.req_len >= REQ_BUF_SIZE - 1 {
            // Buffer full: try one process pass with is_full=true.
            let bytes = &c.req_buf[..c.req_len];
            if let Some(Done { response, close }) =
                http::process(bytes, true, q, idx, mcc_table, nprobe)
            {
                c.res_ptr = response.as_ptr();
                c.res_len = response.len();
                c.res_sent = 0;
                c.close_after_write = close;
            } else {
                return true;
            }
            return false;
        }
        let space = REQ_BUF_SIZE - 1 - c.req_len;
        let n = unsafe {
            libc::read(
                c.fd,
                c.req_buf.as_mut_ptr().add(c.req_len) as *mut libc::c_void,
                space,
            )
        };
        if n == 0 {
            return true;
        }
        if n < 0 {
            let err = std::io::Error::last_os_error();
            match err.kind() {
                std::io::ErrorKind::WouldBlock => return false,
                std::io::ErrorKind::Interrupted => continue,
                _ => return true,
            }
        }
        c.req_len += n as usize;

        // Try to satisfy a complete request from accumulated bytes.
        let bytes = &c.req_buf[..c.req_len];
        let is_full = c.req_len >= REQ_BUF_SIZE - 1;
        match http::process(bytes, is_full, q, idx, mcc_table, nprobe) {
            None => continue,
            Some(Done { response, close }) => {
                c.res_ptr = response.as_ptr();
                c.res_len = response.len();
                c.res_sent = 0;
                c.close_after_write = close;
                return false;
            }
        }
    }
}

/// Returns true if the connection should be closed.
fn write_loop(cidx: usize, conns: &mut [Conn], epfd: i32) -> bool {
    loop {
        let c = &mut conns[cidx];
        if c.res_sent >= c.res_len {
            // Done with this response. Reset for next request (keep-alive).
            if c.close_after_write {
                return true;
            }
            c.req_len = 0;
            c.res_ptr = std::ptr::null();
            c.res_len = 0;
            c.res_sent = 0;
            // Disarm EPOLLOUT.
            if c.epoll_writable {
                let _ = epoll_mod(epfd, c.fd, cidx as u64, libc::EPOLLIN as u32);
                c.epoll_writable = false;
            }
            return false;
        }
        let n = unsafe {
            libc::write(
                c.fd,
                c.res_ptr.add(c.res_sent) as *const libc::c_void,
                c.res_len - c.res_sent,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            match err.kind() {
                std::io::ErrorKind::WouldBlock => {
                    // Arm EPOLLOUT and wait for kernel to wake us.
                    if !c.epoll_writable {
                        let _ = epoll_mod(
                            epfd,
                            c.fd,
                            cidx as u64,
                            (libc::EPOLLIN | libc::EPOLLOUT) as u32,
                        );
                        c.epoll_writable = true;
                    }
                    return false;
                }
                std::io::ErrorKind::Interrupted => continue,
                _ => return true,
            }
        }
        c.res_sent += n as usize;
    }
}

fn accept_loop(server_fd: i32, epfd: i32, conns: &mut [Conn]) -> Result<(), String> {
    loop {
        let fd = unsafe {
            libc::accept4(
                server_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = std::io::Error::last_os_error();
            match err.kind() {
                std::io::ErrorKind::WouldBlock => return Ok(()),
                std::io::ErrorKind::Interrupted => continue,
                _ => return Ok(()),
            }
        }
        match alloc_conn(conns, fd) {
            Some(idx) => {
                if let Err(_) = epoll_add(epfd, fd, idx as u64, libc::EPOLLIN as u32) {
                    free_conn(epfd, &mut conns[idx]);
                }
            }
            None => unsafe {
                libc::close(fd);
            },
        }
    }
}

fn epoll_add(epfd: i32, fd: i32, tag: u64, events: u32) -> Result<(), String> {
    let mut ev = libc::epoll_event {
        events,
        u64: tag,
    };
    let rc = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, fd, &mut ev) };
    if rc < 0 {
        return Err(format!("epoll_ctl ADD: {}", last_os_error()));
    }
    Ok(())
}

fn epoll_mod(epfd: i32, fd: i32, tag: u64, events: u32) -> Result<(), String> {
    let mut ev = libc::epoll_event {
        events,
        u64: tag,
    };
    let rc = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, fd, &mut ev) };
    if rc < 0 {
        return Err(format!("epoll_ctl MOD: {}", last_os_error()));
    }
    Ok(())
}

fn epoll_del(epfd: i32, fd: i32) {
    unsafe {
        libc::epoll_ctl(epfd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut());
    }
}

fn bind_uds(path: &str, backlog: i32) -> Result<i32, String> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| format!("bind {path}: {e}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| format!("chmod {path}: {e}"))?;
    let fd = listener.as_raw_fd();
    let _keep = listener;
    let owned = _keep.into_raw_fd();
    if backlog > 0 {
        unsafe {
            libc::listen(owned, backlog);
        }
    }
    let _ = fd;
    Ok(owned)
}

fn set_nonblock(fd: i32) -> Result<(), String> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags < 0 {
            return Err(format!("F_GETFL: {}", last_os_error()));
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(format!("F_SETFL: {}", last_os_error()));
        }
    }
    Ok(())
}

fn alloc_conns(n: usize) -> Box<[Conn]> {
    let layout = std::alloc::Layout::array::<Conn>(n).expect("conn layout");
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout) as *mut Conn;
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        let slice = std::slice::from_raw_parts_mut(ptr, n);
        Box::from_raw(slice as *mut [Conn])
    }
}

fn alloc_conn(conns: &mut [Conn], fd: i32) -> Option<usize> {
    for (i, c) in conns.iter_mut().enumerate() {
        if !c.in_use {
            c.fd = fd;
            c.in_use = true;
            c.close_after_write = false;
            c.epoll_writable = false;
            c.req_len = 0;
            c.res_ptr = std::ptr::null();
            c.res_len = 0;
            c.res_sent = 0;
            return Some(i);
        }
    }
    None
}

fn free_conn(epfd: i32, c: &mut Conn) {
    if !c.in_use {
        return;
    }
    epoll_del(epfd, c.fd);
    unsafe {
        libc::close(c.fd);
    }
    c.fd = -1;
    c.in_use = false;
    c.close_after_write = false;
    c.epoll_writable = false;
    c.req_len = 0;
    c.res_ptr = std::ptr::null();
    c.res_len = 0;
    c.res_sent = 0;
}

fn last_os_error() -> std::io::Error {
    std::io::Error::last_os_error()
}
