//! Thread wiring: sources -> router -> publisher.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::discovery::DeviceInfo;
use crate::publisher::{Publisher, PublisherConfig, TcpConnector};
use crate::router::Router;
use crate::sources::exlap::{ExlapConfig, ExlapSource, Timings};
use crate::sources::ubx::{UbxConfig, UbxSource};
use crate::sources::{stop_flag, supervise, StopFlag};
use crate::status::{self, Status};
use crate::value::{SignalId, Update, Value};
use crate::{info, warn};

pub struct Pipeline {
    stop: StopFlag,
    sources: Vec<JoinHandle<()>>,
    router: JoinHandle<()>,
    publisher: JoinHandle<()>,
}

impl Pipeline {
    /// Stop sources, let router and publisher drain, publish `offline`, join all.
    pub fn shutdown(self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.sources {
            let _ = h.join();
        }
        let _ = self.router.join();
        let _ = self.publisher.join();
    }

    /// Block for the life of the daemon.
    pub fn wait(self) {
        let _ = self.publisher.join();
    }
}

pub fn publisher_config(cfg: &Config) -> PublisherConfig {
    PublisherConfig {
        topic_prefix: cfg.mqtt.topic_prefix.clone(),
        discovery_prefix: cfg.mqtt.discovery_prefix.clone(),
        device: DeviceInfo { name: cfg.device.name.clone(), model: cfg.device.model.clone() },
        vin_override: cfg.device.vin.clone(),
        client_id: cfg.mqtt.client_id.clone(),
        username: cfg.mqtt.username.clone(),
        password: cfg.mqtt.password.clone(),
        keepalive: Duration::from_secs(cfg.mqtt.keepalive_s),
    }
}

pub fn start(cfg: &Config) -> Result<Pipeline, String> {
    let rates = cfg.effective_rates();
    let stop = stop_flag();
    let (utx, urx) = mpsc::channel::<Update>();
    let (ptx, prx) = mpsc::channel::<(SignalId, Value)>();
    let status = Arc::new(Status::new());
    let mut priorities = HashMap::new();
    let mut sources = Vec::new();

    let ex = &cfg.sources.exlap;
    if ex.enabled {
        priorities.insert("exlap", ex.priority);
        let src = ExlapSource::new(
            ExlapConfig { addr: ex.addr.clone(), user: ex.user.clone(), password: ex.password.clone() },
            &rates,
            Timings::default(),
        )
        .with_status(status.clone());
        sources.push(supervise(Box::new(src), utx.clone(), stop.clone()));
    }

    let ub = &cfg.sources.ubx;
    if ub.enabled {
        priorities.insert("ubx", ub.priority);
        let src = UbxSource::new(UbxConfig { command: ub.command.clone(), args: ub.args.clone() });
        sources.push(supervise(Box::new(src), utx.clone(), stop.clone()));
    }
    drop(utx);
    if sources.is_empty() {
        return Err("no data sources are enabled".into());
    }

    let mut router = Router::new(rates, priorities);
    let router_h = thread::Builder::new()
        .name("router".into())
        .spawn(move || loop {
            match urx.recv_timeout(Duration::from_millis(250)) {
                Ok(u) => {
                    if let Some(out) = router.ingest(u, Instant::now()) {
                        if ptx.send(out).is_err() {
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            for out in router.tick(Instant::now()) {
                if ptx.send(out).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;

    let publisher = Publisher::new(publisher_config(cfg), TcpConnector { host: cfg.mqtt.host.clone(), port: cfg.mqtt.port })
        .with_status(status.clone());
    let pub_h = thread::Builder::new()
        .name("publisher".into())
        .spawn(move || publisher.run(prx))
        .map_err(|e| e.to_string())?;

    // A bind failure must not take the daemon down: log it and carry on.
    match status::spawn(&format!("127.0.0.1:{}", cfg.status.port), status, stop.clone()) {
        Ok((addr, _)) => info!("status endpoint on {addr}"),
        Err(e) => warn!("status endpoint disabled: cannot bind 127.0.0.1:{}: {e}", cfg.status.port),
    }

    Ok(Pipeline { stop, sources, router: router_h, publisher: pub_h })
}

/// On-target smoke test: crypto, formatting, threads, TCP loopback, config parsing.
pub fn self_test() -> Result<(), String> {
    use crate::sources::exlap::auth;
    if auth::b64(&auth::sha256(b"abc")) != "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=" {
        return Err("sha256 mismatch".into());
    }
    if crate::value::format_num(1234.5) != "1234.5" {
        return Err("format_num mismatch".into());
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let echo = thread::spawn(move || -> std::io::Result<()> {
        let (mut s, _) = listener.accept()?;
        let mut b = [0u8; 4];
        s.read_exact(&mut b)?;
        s.write_all(&b)
    });
    let mut c = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    c.write_all(b"ping").map_err(|e| e.to_string())?;
    let mut b = [0u8; 4];
    c.read_exact(&mut b).map_err(|e| e.to_string())?;
    if &b != b"ping" {
        return Err("loopback echo mismatch".into());
    }
    echo.join().map_err(|_| "echo thread panicked".to_string())?.map_err(|e| e.to_string())?;
    Config::parse("[mqtt]\nhost = \"x\"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn self_test_passes_on_host() {
        super::self_test().unwrap();
    }
}
