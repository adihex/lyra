//! Reference client: pair or reconnect against a running Lyra instance.
//!   remote_client pair 127.0.0.1:4777 "qa-phone" 123456
//!   remote_client connect 127.0.0.1:4777 toggle|next|seek:30|vol:0.5
//!   remote_client verify 127.0.0.1:4777 <host_pub_hex> toggle

use lyra_core::PlayerCommand;
use lyra_remote::Client;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).expect("pair|connect|verify");
    let addr: SocketAddr = std::env::args().nth(2).unwrap().parse().unwrap();
    let client = Client::new();

    let mut session = match mode.as_str() {
        "pair" => {
            let name = std::env::args().nth(3).unwrap();
            let code = std::env::args().nth(4).unwrap();
            client.pair(addr, &name, &code).await.expect("pair failed")
        }
        "connect" => client.connect(addr).await.expect("connect failed"),
        "verify" => {
            let hex = std::env::args().nth(3).unwrap();
            let mut pk = [0u8; 32];
            for (i, b) in pk.iter_mut().enumerate() {
                *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
            }
            client.connect_verify(addr, &pk).await.expect("verify failed")
        }
        _ => panic!("bad mode"),
    };

    println!("host: {}", session.host_static().map(|k| k.iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap_or_default());
    println!("hello: {}", session.hello().await.unwrap());

    for arg in std::env::args().skip(if mode == "pair" { 5 } else if mode == "verify" { 4 } else { 3 }) {
        let cmd = match arg.as_str() {
            "toggle" => PlayerCommand::Toggle,
            "next" => PlayerCommand::Next,
            "prev" => PlayerCommand::Prev,
            "stop" => PlayerCommand::StopAfterCurrent,
            a if a.starts_with("seek:") => {
                PlayerCommand::Seek { position_secs: a[5..].parse().unwrap() }
            }
            a if a.starts_with("vol:") => {
                PlayerCommand::Volume { value: a[4..].parse().unwrap() }
            }
            _ => continue,
        };
        let resp = session.send(&cmd).await.unwrap();
        println!("{arg} → {resp}");
    }
}
