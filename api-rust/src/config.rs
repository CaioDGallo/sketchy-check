use std::env;

pub struct Config {
    pub uds_path: String,
    pub index_path: String,
    pub mcc_risk_path: String,
    pub ivf_nprobe: u32,
    pub iouring_qd: u32,
    pub accept_sqes: u32,
    pub backlog: i32,
    pub max_conns: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let uds_path = env::var("UDS_PATH").unwrap_or_else(|_| "/sockets/api.sock".to_string());
        let index_path = env::var("INDEX_PATH").unwrap_or_else(|_| "/app/index.bin".to_string());
        let mcc_risk_path =
            env::var("MCC_RISK_PATH").unwrap_or_else(|_| "/app/mcc_risk.json".to_string());
        let ivf_nprobe = parse_u32("IVF_NPROBE", 1)?;
        let iouring_qd = parse_u32("IOURING_QD", 4096)?;
        let accept_sqes = parse_u32("ACCEPT_SQES", 256)?;
        let backlog = parse_u32("BACKLOG", 4096)? as i32;
        let max_conns = parse_u32("MAX_CONNS", 1024)? as usize;
        Ok(Self {
            uds_path,
            index_path,
            mcc_risk_path,
            ivf_nprobe,
            iouring_qd,
            accept_sqes,
            backlog,
            max_conns,
        })
    }
}

fn parse_u32(key: &str, default: u32) -> Result<u32, String> {
    match env::var(key) {
        Ok(v) => v.parse().map_err(|e| format!("{key}={v:?}: {e}")),
        Err(_) => Ok(default),
    }
}
