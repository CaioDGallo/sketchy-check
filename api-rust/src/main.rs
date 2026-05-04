// sketchy-rust — Rinha de Backend 2026 fraud-scoring API.
//
// Single-threaded io_uring event loop per process. Loads the IVF6 index +
// MCC risk table at startup, listens on a Unix Domain Socket, and serves
// pre-built HTTP/1.1 responses byte-for-byte compatible with the Go version
// in ../api/.

mod config;
mod index;
mod kernel;
mod mcc;
mod responses;
mod server;
mod vectorize;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cfg = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!(
        "sketchy-rust starting: uds={} index={} mcc={} nprobe={}",
        cfg.uds_path, cfg.index_path, cfg.mcc_risk_path, cfg.ivf_nprobe
    );

    let idx: &'static index::Index = match index::Index::load(&cfg.index_path) {
        Ok(i) => Box::leak(Box::new(i)),
        Err(e) => {
            eprintln!("index load failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("loaded index: N={} K={} D={}", idx.n, index::K, index::D);

    let mcc_table: &'static mcc::Table = match mcc::Table::load(&cfg.mcc_risk_path) {
        Ok(t) => Box::leak(Box::new(t)),
        Err(e) => {
            eprintln!("mcc load failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("loaded mcc risk table");

    responses::init();

    if let Err(e) = server::run(&cfg, idx, mcc_table) {
        eprintln!("server failed: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
