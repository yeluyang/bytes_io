use std::{fs, path::PathBuf};

pub use std::panic;

extern crate env_logger;
use env_logger::{Builder, Env};

static INIT: std::sync::Once = std::sync::Once::new();

pub fn init() {
    INIT.call_once(|| {
        Builder::from_env(Env::default().default_filter_or("trace"))
            .is_test(true)
            .init();
    })
}

pub fn make_clean_case_dir(mod_path: &str, case_name: &str) -> PathBuf {
    trace!(
        "making cleaned case directory for case={} in mod={}",
        case_name,
        mod_path
    );
    let mut path = PathBuf::from("tmp");
    for com in mod_path[env!("CARGO_PKG_NAME").len() + 2..].split("::") {
        path = path.join(com);
    }

    let case_dir = path.join(case_name);

    if case_dir.exists() {
        fs::remove_dir_all(&case_dir).unwrap();
    }
    fs::create_dir_all(&case_dir).unwrap();

    case_dir
}
