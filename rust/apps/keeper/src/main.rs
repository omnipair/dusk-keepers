use std::{
    env,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
};

use dusk_adapter::ProtocolLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Shadow,
    Live,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "shadow" => Ok(Self::Shadow),
            "live" => Ok(Self::Live),
            _ => Err(format!("KEEPER_MODE must be shadow or live, got {value}")),
        }
    }
}

#[derive(Debug)]
struct Config {
    mode: Mode,
    profile: String,
    bind_address: SocketAddr,
    protocol_lock: PathBuf,
}

impl Config {
    fn from_environment() -> Result<Self, Box<dyn Error>> {
        let mode = Mode::parse(&env::var("KEEPER_MODE").unwrap_or_else(|_| "shadow".into()))?;
        let profile = env::var("KEEPER_PROFILE").unwrap_or_else(|_| "conformance".into());
        let port = env::var("PORT").unwrap_or_else(|_| "8080".into());
        let bind_address = format!("0.0.0.0:{port}").parse()?;
        let protocol_lock = env::var("DUSK_PROTOCOL_LOCK")
            .unwrap_or_else(|_| "protocol.lock.json".into())
            .into();
        Ok(Self {
            mode,
            profile,
            bind_address,
            protocol_lock,
        })
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::from_environment()?;
    let lock = ProtocolLock::read(&config.protocol_lock)?;

    if config.mode == Mode::Live {
        lock.assert_live_ready()?;
        return Err("live executor is intentionally not implemented in this scaffold".into());
    }

    println!(
        "keeper profile={} mode=shadow protocol_revision={}",
        config.profile, lock.revision
    );
    if env::args().any(|argument| argument == "--check") {
        return Ok(());
    }
    serve(config.bind_address, &config.profile, &lock.revision)
}

fn serve(address: SocketAddr, profile: &str, revision: &str) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(address)?;
    for stream in listener.incoming() {
        respond(stream?, profile, revision)?;
    }
    Ok(())
}

fn respond(mut stream: TcpStream, profile: &str, revision: &str) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/healthz" | "/readyz" => (
            "200 OK",
            format!(
                "{{\"status\":\"healthy\",\"mode\":\"shadow\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\"}}"
            ),
        ),
        _ => ("404 Not Found", "{\"status\":\"not_found\"}".into()),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_is_fail_closed() {
        assert_eq!(Mode::parse("shadow").unwrap(), Mode::Shadow);
        assert_eq!(Mode::parse("live").unwrap(), Mode::Live);
        assert!(Mode::parse("send").is_err());
    }
}
