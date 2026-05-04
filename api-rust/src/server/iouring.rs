// io_uring event loop. Single-threaded; one event loop per process. Per
// connection state is preallocated in CONNS — index packed into the u64
// user_data alongside the op type. Same model as
// rinha-c-good-latency/src/iouring_server.c.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::IntoRawFd;
use std::os::unix::net::UnixListener;

use io_uring::{opcode, types, IoUring};

use crate::config::Config;
use crate::index::Index;
use crate::mcc;
use crate::server::http::{self, Done};

pub const REQ_BUF_SIZE: usize = 16 * 1024;

const OP_ACCEPT: u64 = 1;
const OP_READ: u64 = 2;
const OP_WRITE: u64 = 3;

#[repr(C)]
struct Conn {
    fd: i32,
    in_use: bool,
    close_after_write: bool,
    req_len: usize,
    res_ptr: *const u8,
    res_len: usize,
    res_sent: usize,
    req_buf: [u8; REQ_BUF_SIZE],
}

#[inline]
fn pack(op: u64, idx: u32) -> u64 {
    (op << 48) | (idx as u64 & 0xFFFF_FFFF)
}

#[inline]
fn unpack_op(ud: u64) -> u64 {
    ud >> 48
}

#[inline]
fn unpack_idx(ud: u64) -> u32 {
    (ud & 0xFFFF_FFFF) as u32
}

pub fn run(cfg: &Config, idx: &Index, mcc_table: &mcc::Table) -> Result<(), String> {
    let server_fd = bind_uds(&cfg.uds_path, cfg.backlog)?;

    let mut ring = IoUring::new(cfg.iouring_qd).map_err(|e| format!("io_uring init: {e}"))?;
    let mut conns = alloc_conns(cfg.max_conns);
    let n_conns = conns.len();
    let mut cqe_buf: Vec<(u64, i32)> = Vec::with_capacity(cfg.iouring_qd as usize);
    let mut q = [0f32; 14];

    eprintln!("server mode: io_uring (qd={}, accept_sqes={})", cfg.iouring_qd, cfg.accept_sqes);

    // Prime accept SQEs.
    for _ in 0..cfg.accept_sqes {
        push_accept(&mut ring, server_fd)?;
    }
    ring.submit().map_err(|e| format!("submit: {e}"))?;

    loop {
        match ring.submit_and_wait(1) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("submit_and_wait: {e}")),
        }

        cqe_buf.clear();
        {
            let cq = ring.completion();
            for cqe in cq {
                cqe_buf.push((cqe.user_data(), cqe.result()));
            }
        }

        for &(ud, res) in &cqe_buf {
            let op = unpack_op(ud);
            let cidx = unpack_idx(ud) as usize;
            match op {
                OP_ACCEPT => {
                    if res >= 0 {
                        let client_fd = res;
                        set_nonblock(client_fd);
                        if let Some(slot) = alloc_conn(&mut conns, client_fd) {
                            push_read(&mut ring, &mut conns, slot)?;
                        } else {
                            // No free slot: close the new connection.
                            unsafe {
                                libc::close(client_fd);
                            }
                        }
                    }
                    push_accept(&mut ring, server_fd)?;
                }
                OP_READ => {
                    if cidx >= n_conns {
                        continue;
                    }
                    if !conns[cidx].in_use {
                        continue;
                    }
                    if res <= 0 {
                        free_conn(&mut conns[cidx]);
                        continue;
                    }
                    conns[cidx].req_len += res as usize;
                    let bytes_read = conns[cidx].req_len;
                    let is_full = bytes_read >= REQ_BUF_SIZE - 1;
                    let bytes = &conns[cidx].req_buf[..bytes_read];
                    let decision = http::process(bytes, is_full, &mut q, idx, mcc_table, cfg.ivf_nprobe);
                    match decision {
                        None => {
                            push_read(&mut ring, &mut conns, cidx)?;
                        }
                        Some(Done { response, close }) => {
                            conns[cidx].res_ptr = response.as_ptr();
                            conns[cidx].res_len = response.len();
                            conns[cidx].res_sent = 0;
                            conns[cidx].close_after_write = close;
                            push_write(&mut ring, &mut conns, cidx)?;
                        }
                    }
                }
                OP_WRITE => {
                    if cidx >= n_conns {
                        continue;
                    }
                    if !conns[cidx].in_use {
                        continue;
                    }
                    if res <= 0 {
                        free_conn(&mut conns[cidx]);
                        continue;
                    }
                    conns[cidx].res_sent += res as usize;
                    if conns[cidx].res_sent < conns[cidx].res_len {
                        push_write(&mut ring, &mut conns, cidx)?;
                    } else if conns[cidx].close_after_write {
                        free_conn(&mut conns[cidx]);
                    } else {
                        // Reset for keep-alive.
                        let c = &mut conns[cidx];
                        c.req_len = 0;
                        c.res_ptr = std::ptr::null();
                        c.res_len = 0;
                        c.res_sent = 0;
                        c.close_after_write = false;
                        push_read(&mut ring, &mut conns, cidx)?;
                    }
                }
                _ => {}
            }
        }
    }
}

fn bind_uds(path: &str, backlog: i32) -> Result<i32, String> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| format!("bind {path}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| format!("chmod {path}: {e}"))?;
    let fd = listener.into_raw_fd();
    if backlog > 0 {
        unsafe {
            libc::listen(fd, backlog);
        }
    }
    Ok(fd)
}

fn set_nonblock(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
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
            c.req_len = 0;
            c.res_ptr = std::ptr::null();
            c.res_len = 0;
            c.res_sent = 0;
            // req_buf intentionally not cleared — the request parser only
            // reads up to req_len bytes.
            return Some(i);
        }
    }
    None
}

fn free_conn(c: &mut Conn) {
    if !c.in_use {
        return;
    }
    unsafe {
        libc::close(c.fd);
    }
    c.fd = -1;
    c.in_use = false;
    c.close_after_write = false;
    c.req_len = 0;
    c.res_ptr = std::ptr::null();
    c.res_len = 0;
    c.res_sent = 0;
}

fn push_accept(ring: &mut IoUring, server_fd: i32) -> Result<(), String> {
    let entry = opcode::Accept::new(types::Fd(server_fd), std::ptr::null_mut(), std::ptr::null_mut())
        .build()
        .user_data(pack(OP_ACCEPT, 0));
    push_sqe(ring, &entry)
}

fn push_read(ring: &mut IoUring, conns: &mut [Conn], cidx: usize) -> Result<(), String> {
    let c = &mut conns[cidx];
    if c.req_len >= REQ_BUF_SIZE - 1 {
        return Ok(());
    }
    let entry = opcode::Recv::new(
        types::Fd(c.fd),
        unsafe { c.req_buf.as_mut_ptr().add(c.req_len) },
        (REQ_BUF_SIZE - 1 - c.req_len) as u32,
    )
    .build()
    .user_data(pack(OP_READ, cidx as u32));
    push_sqe(ring, &entry)
}

fn push_write(ring: &mut IoUring, conns: &mut [Conn], cidx: usize) -> Result<(), String> {
    let c = &conns[cidx];
    if c.res_sent >= c.res_len {
        return Ok(());
    }
    let entry = opcode::Send::new(
        types::Fd(c.fd),
        unsafe { c.res_ptr.add(c.res_sent) },
        (c.res_len - c.res_sent) as u32,
    )
    .build()
    .user_data(pack(OP_WRITE, cidx as u32));
    push_sqe(ring, &entry)
}

fn push_sqe(ring: &mut IoUring, entry: &io_uring::squeue::Entry) -> Result<(), String> {
    unsafe {
        if ring.submission().push(entry).is_err() {
            ring.submit().map_err(|e| format!("submit on full SQ: {e}"))?;
            ring.submission()
                .push(entry)
                .map_err(|e| format!("push after submit: {e}"))?;
        }
    }
    Ok(())
}
