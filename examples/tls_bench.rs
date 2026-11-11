//! What the network profile costs (2026-10-24): full handshakes (SecP384r1MLKEM1024, ECDSA P-384 both
//! ways) and 1 KiB round trips over loopback. `cargo run --release --example tls_bench -- [N]`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

use cubelang::auth::{keygen, Identity, Kind, Protect};

fn ident(name: &str) -> Identity {
    let (file, _) = keygen(name, Kind::Agent, None, Protect::Plaintext).unwrap();
    Identity::load(&file, None).unwrap()
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(200);
    let (host, client) = (ident("Host"), ident("Client"));
    let host_pin = cubelang::tls::parse_pin(&cubelang::tls::pin(&host).unwrap()).unwrap();
    let client_pin = cubelang::tls::parse_pin(&cubelang::tls::pin(&client).unwrap()).unwrap();
    let scfg = cubelang::tls::server_config(&host, &[client_pin]).unwrap();
    let ccfg = cubelang::tls::client_config(&client, host_pin).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(scfg.clone()).unwrap(), s.unwrap());
            let mut buf = [0u8; 1024];
            // echo 1 KiB messages until the client goes
            while tls.read_exact(&mut buf).is_ok() {
                if tls.write_all(&buf).is_err() {
                    break;
                }
            }
        }
    });

    let t = Instant::now();
    for _ in 0..n {
        let mut s = TcpStream::connect(addr).unwrap();
        let mut c = rustls::ClientConnection::new(ccfg.clone(), cubelang::tls::server_name()).unwrap();
        while c.is_handshaking() {
            c.complete_io(&mut s).unwrap();
        }
    }
    let hs = t.elapsed().as_secs_f64() * 1e3 / n as f64;

    let s = TcpStream::connect(addr).unwrap();
    s.set_nodelay(true).unwrap();
    let mut tls = rustls::StreamOwned::new(rustls::ClientConnection::new(ccfg, cubelang::tls::server_name()).unwrap(), s);
    let msg = [7u8; 1024];
    let mut back = [0u8; 1024];
    let rounds = n * 10;
    let t = Instant::now();
    for _ in 0..rounds {
        tls.write_all(&msg).unwrap();
        tls.read_exact(&mut back).unwrap();
    }
    let rt = t.elapsed().as_secs_f64() * 1e6 / rounds as f64;
    println!("{} handshakes: {:.2} ms each (client side, both keys checked); 1 KiB round trip: {:.0} us", n, hs, rt);
}
