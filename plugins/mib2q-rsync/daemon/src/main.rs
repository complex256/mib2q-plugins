use std::path::PathBuf;
use std::process::ExitCode;

use rsyncd::config::Config;
use rsyncd::log::{self, Level};

const USAGE: &str = "usage: rsyncd [--config PATH] [--rsync PATH] [--self-test] [--version]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut config = PathBuf::from("/mnt/app/root/rsyncd/rsyncd.toml");
    let mut rsync_override: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => match args.next() {
                Some(path) => config = PathBuf::from(path),
                None => {
                    eprintln!("--config needs a path\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--rsync" => match args.next() {
                Some(path) => rsync_override = Some(PathBuf::from(path)),
                None => {
                    eprintln!("--rsync needs a path\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--self-test" => {
                return match rsyncd::app::self_test() {
                    Ok(()) => {
                        println!("SELFTEST OK");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("SELFTEST FAILED: {e}");
                        ExitCode::FAILURE
                    }
                };
            }
            "--version" => {
                println!("rsyncd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let cfg = match Config::load(&config) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("config {}: {e}", config.display());
            return ExitCode::FAILURE;
        }
    };

    let level = Level::parse(&cfg.log.level).unwrap_or(Level::Info);
    let log_path = if cfg.log.file == "-" {
        None
    } else {
        Some(PathBuf::from(&cfg.log.file))
    };
    if let Err(e) = log::init(log_path, level, cfg.log.max_bytes) {
        eprintln!("log init failed ({e}); logging to stderr");
    }

    let rsync_bin = rsync_override.unwrap_or_else(|| cfg.daemon.effective_rsync_bin());
    rsyncd::info!(
        "rsyncd {} starting (config {}, rsync {})",
        env!("CARGO_PKG_VERSION"),
        config.display(),
        rsync_bin.display()
    );

    match rsyncd::app::start(&cfg, &rsync_bin) {
        Ok(daemon) => {
            daemon.wait();
            ExitCode::SUCCESS
        }
        Err(e) => {
            rsyncd::error!("{e}");
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
