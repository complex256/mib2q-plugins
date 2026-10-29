use std::path::PathBuf;
use std::process::ExitCode;

use carmqtt::config::Config;
use carmqtt::log::{self, Level};

const USAGE: &str = "usage: carmqtt [--config PATH] [--self-test] [--version]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut config = PathBuf::from("/mnt/app/root/carmqtt/carmqtt.toml");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => match args.next() {
                Some(p) => config = PathBuf::from(p),
                None => {
                    eprintln!("--config needs a path\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--self-test" => {
                return match carmqtt::app::self_test() {
                    Ok(()) => {
                        println!("SELFTEST OK");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("SELFTEST FAILED: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            "--version" => {
                println!("carmqtt {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let cfg = match Config::load(&config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config {}: {e}", config.display());
            return ExitCode::FAILURE;
        }
    };
    let level = Level::parse(&cfg.log.level).unwrap_or(Level::Info);
    let path = if cfg.log.file == "-" { None } else { Some(PathBuf::from(&cfg.log.file)) };
    if let Err(e) = log::init(path, level, cfg.log.max_bytes) {
        eprintln!("log init failed ({e}); logging to stderr");
    }
    carmqtt::info!("carmqtt {} starting", env!("CARGO_PKG_VERSION"));
    match carmqtt::app::start(&cfg) {
        Ok(p) => {
            p.wait();
            ExitCode::SUCCESS
        }
        Err(e) => {
            carmqtt::error!("{e}");
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
