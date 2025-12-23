use anyhow::Result;
use log::{error, info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Deserialize, Debug)]
struct OscQueryNode {
    #[serde(rename = "FULL_PATH")]
    full_path: String,
    #[serde(rename = "TYPE")]
    type_: Option<String>,
    #[serde(rename = "CONTENTS")]
    contents: Option<HashMap<String, OscQueryNode>>,
}

pub struct OscQueryService {
    update_sender: Sender<Option<HashSet<String>>>,
    change_receiver: Option<Receiver<String>>,
}

impl OscQueryService {
    pub fn new(
        update_sender: Sender<Option<HashSet<String>>>,
        change_receiver: Receiver<String>,
    ) -> Self {
        Self {
            update_sender,
            change_receiver: Some(change_receiver),
        }
    }

    pub fn start(mut self) -> Result<()> {
        let sender = self.update_sender.clone();

        let current_url = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let current_url_mdns = current_url.clone();
        let current_url_change = current_url.clone();

        let sender_mdns = sender.clone();
        let sender_change = sender.clone();

        // mDNS Thread
        thread::spawn(move || {
            info!("Starting mDNS Discovery Thread...");

            loop {
                let mdns = match ServiceDaemon::new() {
                    Ok(d) => d,
                    Err(e) => {
                        error!("Failed to create mDNS daemon: {}. Retrying in 5s...", e);
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                };

                let service_type = "_oscjson._tcp.local.";
                let receiver = match mdns.browse(service_type) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to browse for service: {}. Retrying in 5s...", e);
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                };

                info!("mDNS Daemon started. Browsing for {}...", service_type);

                while let Ok(event) = receiver.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            // Name Validation: Must start with "VRChat-Client-"
                            // The fullname usually looks like "VRChat-Client-XXXX._oscjson._tcp.local."
                            // We check the instance name part.
                            let instance_name = info.get_fullname().split('.').next().unwrap_or("");
                            if !instance_name.starts_with("VRChat-Client-") {
                                info!("Ignored non-VRChat service: {}", instance_name);
                                continue;
                            }

                            // IPv4 Only: VRChat only supports IPv4 for OSC?
                            let addr = info.get_addresses().iter().find(|ip| ip.is_ipv4());

                            if let Some(ip) = addr {
                                let port = info.get_port();
                                let url = format!("http://{}:{}/avatar", ip, port);

                                info!("VRChat Discovered at: {}", url);

                                {
                                    let mut lock = current_url_mdns.lock().unwrap();
                                    *lock = Some(url.clone());
                                }

                                // Initial Fetch with Retry
                                fetch_with_retry(&url, &sender_mdns);
                            } else {
                                info!(
                                    "Ignored VRChat service with no IPv4 address: {}",
                                    instance_name
                                );
                            }
                        }
                        ServiceEvent::ServiceRemoved(_type, fullname) => {
                            // Check if the removed service was VRChat
                            if fullname.starts_with("VRChat-Client-") {
                                info!(
                                    "VRChat Service Removed: {}. Restarting mDNS discovery...",
                                    fullname
                                );
                                {
                                    let mut lock = current_url_mdns.lock().unwrap();
                                    *lock = None;
                                }
                                let _ = sender_mdns.send(None);

                                // Break the inner loop to restart the daemon
                                // This is important because mDNS daemons might get stuck or need re-binding
                                // if network interfaces changed (which often causes the service removal).
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                // If we broke out of the loop, wait a bit before restarting
                thread::sleep(Duration::from_secs(2));
            }
        });

        // Change Listener Thread
        if let Some(change_rx) = self.change_receiver.take() {
            thread::spawn(move || {
                info!("Starting Avatar Change Listener Thread...");
                while let Ok(_) = change_rx.recv() {
                    info!("Avatar Change Signal Received. Re-fetching...");

                    let url_opt = {
                        let lock = current_url_change.lock().unwrap();
                        lock.clone()
                    };

                    if let Some(url) = url_opt {
                        fetch_with_retry(&url, &sender_change);
                    } else {
                        warn!("Avatar change received but VRChat service not yet discovered.");
                    }
                }
            });
        }

        Ok(())
    }
}

fn fetch_with_retry(url: &str, sender: &Sender<Option<HashSet<String>>>) {
    let max_retries = 5;
    let retry_delay = Duration::from_secs(1);
    let url = url.to_string();
    let sender = sender.clone();

    thread::spawn(move || {
        for i in 0..max_retries {
            info!(
                "Fetching avatar info (Attempt {}/{})...",
                i + 1,
                max_retries
            );
            match fetch_avatar_parameters(&url) {
                Ok(params) => {
                    info!("Successfully fetched {} parameters.", params.len());
                    let _ = sender.send(Some(params));
                    return;
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch avatar parameters: {}. Retrying in {:?}...",
                        e, retry_delay
                    );
                    thread::sleep(retry_delay);
                }
            }
        }
        error!(
            "Failed to fetch avatar parameters after {} attempts.",
            max_retries
        );
        // Optionally reset to allow all if we can't determine parameters?
        // Or keep previous state.
        // let _ = sender.send(None);
    });
}

fn fetch_avatar_parameters(url: &str) -> Result<HashSet<String>> {
    let resp = ureq::get(url).call()?;
    let root: OscQueryNode = resp.into_json()?;

    let mut params = HashSet::new();
    flatten_node(&root, &mut params);

    Ok(params)
}

fn flatten_node(node: &OscQueryNode, params: &mut HashSet<String>) {
    // If it has a TYPE, it's a parameter (leaf or intermediate with value)
    if node.type_.is_some() {
        params.insert(node.full_path.clone());
    }

    if let Some(contents) = &node.contents {
        for child in contents.values() {
            flatten_node(child, params);
        }
    }
}
