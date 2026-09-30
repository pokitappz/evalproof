#![forbid(unsafe_code)]

pub mod contract;
pub mod license;
pub mod model;
pub mod report;
pub mod runner;
pub mod stats;
pub mod storage;
pub mod worker;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PROTOCOL: u32 = 1;
