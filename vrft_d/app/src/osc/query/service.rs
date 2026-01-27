use anyhow::Result;
use log::{error, info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

/// OSC type tag to Rust type mapping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscParamType {
    Float,
    Bool,
    Int,
    Unknown,
}

impl OscParamType {
    /// Parse OSC type tag string (e.g., "f", "i", "T", "F", "s")
    pub fn from_osc_type_tag(tag: &str) -> Self {
        match tag {
            "f" | "d" => OscParamType::Float, // float or double
            "i" | "h" => OscParamType::Int,   // int32 or int64
            "T" | "F" => OscParamType::Bool,  // True or False constants
            _ => OscParamType::Unknown,
        }
    }
}

/// Parameter info returned from OSC Query
#[derive(Debug, Clone)]
pub struct OscParameterInfo {
    pub address: String,
    pub param_type: OscParamType,
}

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
    update_sender: Sender<Option<Vec<OscParameterInfo>>>,
    change_receiver: Option<Receiver<String>>,
}

impl OscQueryService {
    pub fn new(
        update_sender: Sender<Option<Vec<OscParameterInfo>>>,
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

        // Change Listener Thread (with debounce)
        if let Some(change_rx) = self.change_receiver.take() {
            thread::spawn(move || {
                info!("Starting Avatar Change Listener Thread...");
                let mut last_fetch_time: Option<std::time::Instant> = None;
                const DEBOUNCE_MS: u64 = 500;

                while change_rx.recv().is_ok() {
                    // Debounce: skip if last fetch was less than DEBOUNCE_MS ago
                    if let Some(last) = last_fetch_time {
                        if last.elapsed().as_millis() < DEBOUNCE_MS as u128 {
                            info!("Avatar change debounced (too rapid).");
                            continue;
                        }
                    }

                    info!("Avatar Change Signal Received. Re-fetching...");
                    last_fetch_time = Some(std::time::Instant::now());

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

fn fetch_with_retry(url: &str, sender: &Sender<Option<Vec<OscParameterInfo>>>) {
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
    });
}

fn fetch_avatar_parameters(url: &str) -> Result<Vec<OscParameterInfo>> {
    let mut resp = ureq::get(url).call()?;
    let root: OscQueryNode = resp.body_mut().read_json()?;

    let mut params = Vec::new();

    // Navigate to /avatar/parameters
    if let Some(contents) = &root.contents {
        if let Some(parameters_node) = contents.get("parameters") {
            flatten_node(parameters_node, &mut params);
        }
    }

    Ok(params)
}

fn flatten_node(node: &OscQueryNode, params: &mut Vec<OscParameterInfo>) {
    // First, recurse into any child contents
    if let Some(contents) = &node.contents {
        for child in contents.values() {
            flatten_node(child, params);
        }
    }

    // Then, add this node as a parameter if it has a TYPE tag.
    // A node can have both contents AND a type, so check type_ independently.
    if let Some(type_tag) = &node.type_ {
        if !type_tag.is_empty() {
            params.push(OscParameterInfo {
                address: node.full_path.clone(),
                param_type: OscParamType::from_osc_type_tag(type_tag),
            });
        }
    }
}
