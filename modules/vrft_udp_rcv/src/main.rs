use anyhow::Result;
use api::UnifiedTrackingData;
use std::net::UdpSocket;

fn main() -> Result<()> {
    env_logger::init();

    let port = 9000;
    let addr = format!("0.0.0.0:{}", port);
    let socket = UdpSocket::bind(&addr)?;

    println!("Listening for Face Tracking data on {}...", addr);

    let mut buf = [0u8; 65535]; // Max UDP size
    let mut last_data: Option<UnifiedTrackingData> = None;

    loop {
        match socket.recv_from(&mut buf) {
            Ok((amt, src)) => {
                // println!("Received {} bytes from {}", amt, src);
                let slice = &buf[..amt];
                
                // Try to deserialize as JSON
                match serde_json::from_slice::<UnifiedTrackingData>(slice) {
                    Ok(data) => {
                        if last_data.as_ref() != Some(&data) {
                            println!("Received Tracking Data from {}:", src);
                            println!("{:#?}", data);
                            last_data = Some(data);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to deserialize packet from {}: {}", src, e);
                        if let Ok(s) = std::str::from_utf8(slice) {
                            eprintln!("Raw data: {}", s);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error receiving data: {}", e);
            }
        }
    }
}
