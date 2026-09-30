use anyhow::Result;
use log::{error, info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::target::VrchatTarget;

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
    target: VrchatTarget,
}

impl OscQueryService {
    pub fn new(
        update_sender: Sender<Option<Vec<OscParameterInfo>>>,
        change_receiver: Receiver<String>,
        target: VrchatTarget,
    ) -> Self {
        Self {
            update_sender,
            change_receiver: Some(change_receiver),
            target,
        }
    }

    pub fn start(mut self) -> Result<()> {
        let sender = self.update_sender.clone();

        let current_url = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let current_url_mdns = current_url.clone();
        let current_url_change = current_url.clone();

        let sender_mdns = sender.clone();
        let sender_change = sender.clone();
        let target = self.target.clone();
        let fetches = Fetches::default();
        let fetches_change = fetches.clone();

        // mDNS Thread
        let discover = move || {
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
                // The VRChat instance tracking goes to, by its full mDNS name.
                let mut current_name: Option<String> = None;

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

                            let addresses: Vec<IpAddr> = info
                                .get_addresses()
                                .iter()
                                .map(|ip| ip.to_ip_addr())
                                .collect();
                            // Only the VRChat at the address tracking goes
                            // to, not another on the same network.
                            if !target.matches(&addresses) {
                                info!("Ignored VRChat at another address: {}", instance_name);
                                continue;
                            }

                            // IPv4 Only: VRChat only supports IPv4 for OSC?
                            let addr = addresses.iter().find(|ip| ip.is_ipv4());

                            if let Some(ip) = addr {
                                let port = info.get_port();
                                let base = format!("http://{}:{}", ip, port);
                                let url = format!("{}/avatar", base);

                                info!("VRChat Discovered at: {}", url);
                                let osc_port = fetch_osc_port(&base);
                                if let Some(osc_port) = osc_port {
                                    info!("VRChat listens for OSC on port {}", osc_port);
                                }
                                target.found(osc_port);
                                current_name = Some(info.get_fullname().to_string());

                                {
                                    let mut lock = current_url_mdns.lock().unwrap();
                                    *lock = Some(url.clone());
                                }

                                // Initial Fetch with Retry
                                fetches.start(&url, &sender_mdns);
                            } else {
                                info!(
                                    "Ignored VRChat service with no IPv4 address: {}",
                                    instance_name
                                );
                            }
                        }
                        // Check if the removed service was VRChat
                        ServiceEvent::ServiceRemoved(_type, fullname)
                            if current_name.as_deref() == Some(fullname.as_str()) =>
                        {
                            info!(
                                "VRChat Service Removed: {}. Restarting mDNS discovery...",
                                fullname
                            );
                            {
                                let mut lock = current_url_mdns.lock().unwrap();
                                *lock = None;
                            }
                            fetches.cancel();
                            let _ = sender_mdns.send(None);
                            target.lost();

                            // Break the inner loop to restart the daemon
                            // This is important because mDNS daemons might get stuck or need re-binding
                            // if network interfaces changed (which often causes the service removal).
                            break;
                        }
                        _ => {}
                    }
                }

                // Stop this daemon's threads and sockets before making another.
                if let Err(e) = mdns.shutdown() {
                    warn!("Failed to shut down mDNS daemon: {}", e);
                }

                // If we broke out of the loop, wait a bit before restarting
                thread::sleep(Duration::from_secs(2));
            }
        };
        thread::Builder::new()
            .name("vrchat-discovery".into())
            .spawn(discover)?;

        // Change Listener Thread (with debounce)
        if let Some(change_rx) = self.change_receiver.take() {
            let listen = move || {
                info!("Starting Avatar Change Listener Thread...");
                const DEBOUNCE: Duration = Duration::from_millis(500);

                while change_rx.recv().is_ok() {
                    // Debounce: wait for changes to settle, then read the
                    // last avatar, so a quick run of switches ends on it.
                    let disconnected = loop {
                        match change_rx.recv_timeout(DEBOUNCE) {
                            Ok(_) => info!("Avatar change debounced (too rapid)."),
                            Err(RecvTimeoutError::Timeout) => break false,
                            Err(RecvTimeoutError::Disconnected) => break true,
                        }
                    };

                    info!("Avatar Change Signal Received. Re-fetching...");

                    let url_opt = {
                        let lock = current_url_change.lock().unwrap();
                        lock.clone()
                    };

                    if let Some(url) = url_opt {
                        fetches_change.start(&url, &sender_change);
                    } else {
                        warn!("Avatar change received but VRChat service not yet discovered.");
                    }
                    if disconnected {
                        break;
                    }
                }
            };
            thread::Builder::new()
                .name("avatar-change".into())
                .spawn(listen)?;
        }

        Ok(())
    }
}

/// Reads of the avatar's parameters. Only the latest may deliver, so a slow
/// read of an earlier avatar can't overwrite a later one.
#[derive(Clone, Default)]
struct Fetches {
    latest: Arc<Mutex<u64>>,
}

impl Fetches {
    /// Reads the parameters at `url`, retrying, in the background; any read
    /// still running stops delivering.
    fn start(&self, url: &str, sender: &Sender<Option<Vec<OscParameterInfo>>>) {
        let generation = self.cancel();
        let fetches = self.clone();
        let max_retries = 5;
        let retry_delay = Duration::from_secs(1);
        let url = url.to_string();
        let sender = sender.clone();
        let fetch = move || {
            for i in 0..max_retries {
                if !fetches.is_current(generation) {
                    return;
                }
                info!(
                    "Fetching avatar info (Attempt {}/{})...",
                    i + 1,
                    max_retries
                );
                match fetch_avatar_parameters(&url) {
                    Ok(params) => {
                        info!("Successfully fetched {} parameters.", params.len());
                        fetches.deliver(generation, &sender, params);
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
        };
        thread::Builder::new()
            .name("avatar-fetch".into())
            .spawn(fetch)
            .expect("couldn't start the avatar-fetch thread");
    }

    /// Stops any read running from delivering; returns the new generation.
    fn cancel(&self) -> u64 {
        let mut latest = self.latest.lock().unwrap();
        *latest += 1;
        *latest
    }

    fn is_current(&self, generation: u64) -> bool {
        *self.latest.lock().unwrap() == generation
    }

    /// Sends a read's parameters if no later read has started. The lock is
    /// held while sending so a cancel can't slip in between.
    fn deliver(
        &self,
        generation: u64,
        sender: &Sender<Option<Vec<OscParameterInfo>>>,
        params: Vec<OscParameterInfo>,
    ) {
        let latest = self.latest.lock().unwrap();
        if *latest == generation {
            let _ = sender.send(Some(params));
        } else {
            info!("Dropped avatar parameters read for an earlier avatar.");
        }
    }
}

/// The OSC port VRChat reports in its OSCQuery `HOST_INFO`.
fn fetch_osc_port(base: &str) -> Option<u16> {
    #[derive(Deserialize)]
    struct HostInfo {
        #[serde(rename = "OSC_PORT")]
        osc_port: Option<u16>,
    }
    let url = format!("{}/?HOST_INFO", base);
    match ureq::get(&url)
        .call()
        .and_then(|mut resp| resp.body_mut().read_json::<HostInfo>())
    {
        Ok(info) => info.osc_port,
        Err(e) => {
            warn!("Failed to read VRChat HOST_INFO: {}", e);
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn params() -> Vec<OscParameterInfo> {
        vec![OscParameterInfo {
            address: "/avatar/parameters/FT/v2/JawOpen".into(),
            param_type: OscParamType::Float,
        }]
    }

    #[test]
    fn the_latest_read_delivers() {
        let fetches = Fetches::default();
        let (tx, rx) = channel();
        let generation = fetches.cancel();
        fetches.deliver(generation, &tx, params());
        assert!(rx.try_recv().unwrap().is_some());
    }

    #[test]
    fn an_earlier_read_finishing_late_is_dropped() {
        let fetches = Fetches::default();
        let (tx, rx) = channel();
        let earlier = fetches.cancel();
        let later = fetches.cancel();
        fetches.deliver(earlier, &tx, params());
        assert!(rx.try_recv().is_err());
        assert!(!fetches.is_current(earlier));
        assert!(fetches.is_current(later));
    }
}
