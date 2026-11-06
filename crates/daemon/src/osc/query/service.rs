use anyhow::Result;
use log::{error, info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::target::VrchatTarget;

/// How often to ask VRChat which avatar it's on. Its `/avatar/change` message
/// can miss us, and an avatar can load after our last read, or with OSC off.
const AVATAR_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long after a read gives up before the poll starts another.
const FAILED_READ_BACKOFF: Duration = Duration::from_secs(10);

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

/// What an OSCQuery read found of the avatar.
#[derive(Debug, Clone)]
pub struct AvatarInfo {
    /// The avatar's ID, if VRChat said.
    pub id: Option<String>,
    pub parameters: Vec<OscParameterInfo>,
    /// Whether the avatar takes native face tracking, if VRChat said.
    pub full_face_tracking: Option<bool>,
}

#[derive(Deserialize, Debug)]
struct OscQueryNode {
    #[serde(rename = "FULL_PATH")]
    full_path: String,
    #[serde(rename = "TYPE")]
    type_: Option<String>,
    #[serde(rename = "VALUE")]
    value: Option<serde_json::Value>,
    #[serde(rename = "CONTENTS")]
    contents: Option<HashMap<String, OscQueryNode>>,
}

impl OscQueryNode {
    /// The node's value as a bool. OSCQuery holds values in an array, one per
    /// argument.
    fn bool_value(&self) -> Option<bool> {
        match self.value.as_ref()? {
            serde_json::Value::Array(values) => values.first()?.as_bool(),
            value => value.as_bool(),
        }
    }

    /// The node's value as a non-empty string.
    fn string_value(&self) -> Option<&str> {
        let value = match self.value.as_ref()? {
            serde_json::Value::Array(values) => values.first()?.as_str(),
            value => value.as_str(),
        }?;
        (!value.is_empty()).then_some(value)
    }
}

pub struct OscQueryService {
    update_sender: Sender<Option<AvatarInfo>>,
    change_receiver: Option<Receiver<String>>,
    target: VrchatTarget,
}

impl OscQueryService {
    pub fn new(
        update_sender: Sender<Option<AvatarInfo>>,
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
        let current_url_poll = current_url.clone();

        let sender_mdns = sender.clone();
        let sender_change = sender.clone();
        let sender_poll = sender.clone();
        let target = self.target.clone();
        let target_poll = self.target.clone();
        let fetches = Fetches::default();
        let fetches_change = fetches.clone();
        let fetches_poll = fetches.clone();

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
                            fetches.forget();
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

        let poll = move || loop {
            thread::sleep(AVATAR_POLL_INTERVAL);
            let url = current_url_poll.lock().unwrap().clone();
            let Some(url) = url else { continue };
            // Quiet when this fails: that's VRChat loading, or its OSC being off.
            let Some(id) = fetch_avatar_id(&url) else {
                continue;
            };
            if !fetches_poll.needs_read(&id, Instant::now()) {
                continue;
            }
            info!("VRChat is on avatar {}; reading it", id);
            if fetches_poll.has_read_an_avatar() {
                target_poll.avatar_changed();
            }
            fetches_poll.start(&url, &sender_poll);
        };
        thread::Builder::new()
            .name("avatar-poll".into())
            .spawn(poll)?;

        Ok(())
    }
}

/// Reads of the avatar's parameters. Only the latest may deliver, so a slow
/// read of an earlier avatar can't overwrite a later one.
#[derive(Clone, Default)]
struct Fetches {
    state: Arc<Mutex<FetchState>>,
}

#[derive(Default)]
struct FetchState {
    latest: u64,
    /// Whether the latest read is still running.
    reading: bool,
    /// The avatar the last delivered read was of.
    read_id: Option<String>,
    /// When the latest read gave up.
    failed_at: Option<Instant>,
}

impl Fetches {
    /// Reads the parameters at `url`, retrying, in the background; any read
    /// still running stops delivering.
    fn start(&self, url: &str, sender: &Sender<Option<AvatarInfo>>) {
        let generation = self.cancel();
        self.state.lock().unwrap().reading = true;
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
                match fetch_avatar_info(&url) {
                    Ok(avatar) => {
                        info!(
                            "Successfully fetched {} parameters.",
                            avatar.parameters.len()
                        );
                        fetches.deliver(generation, &sender, avatar);
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
            fetches.gave_up(generation, Instant::now());
        };
        thread::Builder::new()
            .name("avatar-fetch".into())
            .spawn(fetch)
            .expect("couldn't start the avatar-fetch thread");
    }

    /// Stops any read running from delivering; returns the new generation.
    fn cancel(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.latest += 1;
        state.reading = false;
        state.failed_at = None;
        state.latest
    }

    fn is_current(&self, generation: u64) -> bool {
        self.state.lock().unwrap().latest == generation
    }

    /// VRChat has gone: stops any read and forgets the avatar.
    fn forget(&self) {
        self.cancel();
        self.state.lock().unwrap().read_id = None;
    }

    fn gave_up(&self, generation: u64, now: Instant) {
        let mut state = self.state.lock().unwrap();
        if state.latest == generation {
            state.reading = false;
            state.failed_at = Some(now);
        }
    }

    fn has_read_an_avatar(&self) -> bool {
        self.state.lock().unwrap().read_id.is_some()
    }

    /// Whether VRChat being on avatar `id` calls for a new read at `now`.
    fn needs_read(&self, id: &str, now: Instant) -> bool {
        let state = self.state.lock().unwrap();
        !state.reading
            && state.read_id.as_deref() != Some(id)
            && state
                .failed_at
                .is_none_or(|failed| now.duration_since(failed) >= FAILED_READ_BACKOFF)
    }

    /// Sends a read's parameters if no later read has started. The lock is
    /// held while sending so a cancel can't slip in between.
    fn deliver(&self, generation: u64, sender: &Sender<Option<AvatarInfo>>, avatar: AvatarInfo) {
        let mut state = self.state.lock().unwrap();
        if state.latest == generation {
            state.reading = false;
            state.read_id = avatar.id.clone();
            let _ = sender.send(Some(avatar));
        } else {
            info!("Dropped avatar parameters read for an earlier avatar.");
        }
    }
}

/// Asks VRChat's OSCQuery server, with a deadline: a VRChat that takes the
/// connection and then stalls, as it can while loading, mustn't hold up
/// discovery, which reads its port on the mDNS thread.
static HTTP: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into()
});

/// The OSC port VRChat reports in its OSCQuery `HOST_INFO`.
fn fetch_osc_port(base: &str) -> Option<u16> {
    #[derive(Deserialize)]
    struct HostInfo {
        #[serde(rename = "OSC_PORT")]
        osc_port: Option<u16>,
    }
    let url = format!("{}/?HOST_INFO", base);
    match HTTP
        .get(&url)
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

/// The ID of the avatar VRChat is on, asked of its OSCQuery server at `url`,
/// the `/avatar` node.
fn fetch_avatar_id(url: &str) -> Option<String> {
    let node: OscQueryNode = HTTP
        .get(format!("{}/change", url))
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    node.string_value().map(str::to_string)
}

fn fetch_avatar_info(url: &str) -> Result<AvatarInfo> {
    let mut resp = HTTP.get(url).call()?;
    let root: OscQueryNode = resp.body_mut().read_json()?;
    avatar_info(&root)
}

/// What the `/avatar` node of VRChat's OSCQuery tree says of the avatar.
fn avatar_info(root: &OscQueryNode) -> Result<AvatarInfo> {
    let contents = root.contents.as_ref();
    // Every avatar has some, so a tree without them is VRChat still loading
    // it: an error, so the read is tried again, rather than an avatar with
    // nothing to drive.
    let parameters_node = contents
        .and_then(|contents| contents.get("parameters"))
        .ok_or_else(|| anyhow::anyhow!("VRChat's avatar has no parameters yet"))?;
    let mut parameters = Vec::new();
    flatten_node(parameters_node, &mut parameters);

    let full_face_tracking = contents
        .and_then(|contents| contents.get("avatarfullfacetracking"))
        .and_then(OscQueryNode::bool_value);

    let id = contents
        .and_then(|contents| contents.get("change"))
        .and_then(OscQueryNode::string_value)
        .map(str::to_string);

    Ok(AvatarInfo {
        id,
        parameters,
        full_face_tracking,
    })
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

    fn params() -> AvatarInfo {
        AvatarInfo {
            id: Some("avtr_a".into()),
            parameters: vec![OscParameterInfo {
                address: "/avatar/parameters/FT/v2/JawOpen".into(),
                param_type: OscParamType::Float,
            }],
            full_face_tracking: None,
        }
    }

    fn node(json: serde_json::Value) -> OscQueryNode {
        serde_json::from_value(json).unwrap()
    }

    fn avatar_tree(extra: serde_json::Value) -> OscQueryNode {
        let mut contents = serde_json::json!({
            "parameters": {
                "FULL_PATH": "/avatar/parameters",
                "CONTENTS": {
                    "JawOpen": {
                        "FULL_PATH": "/avatar/parameters/JawOpen",
                        "TYPE": "f",
                        "VALUE": [0.5],
                    }
                }
            }
        });
        contents
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        node(serde_json::json!({ "FULL_PATH": "/avatar", "CONTENTS": contents }))
    }

    #[test]
    fn the_read_reports_full_face_tracking() {
        for value in [true, false] {
            let tree = avatar_tree(serde_json::json!({
                "avatarfullfacetracking": {
                    "FULL_PATH": "/avatar/avatarfullfacetracking",
                    "TYPE": "T",
                    "VALUE": [value],
                }
            }));
            let avatar = avatar_info(&tree).unwrap();
            assert_eq!(avatar.full_face_tracking, Some(value));
            assert_eq!(avatar.parameters.len(), 1);
        }
    }

    #[test]
    fn a_tree_without_the_flag_says_nothing_about_it() {
        let tree = avatar_tree(serde_json::json!({}));
        assert_eq!(avatar_info(&tree).unwrap().full_face_tracking, None);
    }

    /// The node can exist with no value yet, or one we can't read. That isn't
    /// the same as VRChat saying no.
    #[test]
    fn a_flag_without_a_bool_says_nothing_about_it() {
        for value in [
            serde_json::json!([]),
            serde_json::json!(["yes"]),
            serde_json::Value::Null,
        ] {
            let tree = avatar_tree(serde_json::json!({
                "avatarfullfacetracking": {
                    "FULL_PATH": "/avatar/avatarfullfacetracking",
                    "VALUE": value,
                }
            }));
            assert_eq!(avatar_info(&tree).unwrap().full_face_tracking, None);
        }
    }

    #[test]
    fn a_tree_without_parameters_is_still_loading() {
        let tree = node(serde_json::json!({ "FULL_PATH": "/avatar", "CONTENTS": {} }));
        assert!(avatar_info(&tree).is_err());
    }

    #[test]
    fn the_read_reports_the_avatar_id() {
        let tree = avatar_tree(serde_json::json!({
            "change": {
                "FULL_PATH": "/avatar/change",
                "TYPE": "s",
                "VALUE": ["avtr_a"],
            }
        }));
        assert_eq!(avatar_info(&tree).unwrap().id.as_deref(), Some("avtr_a"));
    }

    #[test]
    fn an_empty_avatar_id_is_no_avatar() {
        let change = node(serde_json::json!({
            "FULL_PATH": "/avatar/change",
            "VALUE": [""],
        }));
        assert_eq!(change.string_value(), None);
    }

    /// The case a missed `/avatar/change` leaves: nothing else would read it.
    #[test]
    fn an_unread_avatar_needs_a_read() {
        let fetches = Fetches::default();
        assert!(fetches.needs_read("avtr_a", Instant::now()));
        assert!(!fetches.has_read_an_avatar());
    }

    #[test]
    fn the_avatar_just_read_needs_no_other() {
        let fetches = Fetches::default();
        let (tx, _rx) = channel();
        let generation = fetches.cancel();
        fetches.deliver(generation, &tx, params());
        assert!(fetches.has_read_an_avatar());
        assert!(!fetches.needs_read("avtr_a", Instant::now()));
    }

    #[test]
    fn a_different_avatar_needs_a_read() {
        let fetches = Fetches::default();
        let (tx, _rx) = channel();
        let generation = fetches.cancel();
        fetches.deliver(generation, &tx, params());
        assert!(fetches.needs_read("avtr_b", Instant::now()));
    }

    #[test]
    fn a_read_in_progress_is_left_to_finish() {
        let fetches = Fetches::default();
        fetches.cancel();
        fetches.state.lock().unwrap().reading = true;
        assert!(!fetches.needs_read("avtr_a", Instant::now()));
    }

    #[test]
    fn a_read_that_gave_up_is_tried_again_after_the_backoff() {
        let fetches = Fetches::default();
        let generation = fetches.cancel();
        let failed = Instant::now();
        fetches.gave_up(generation, failed);
        assert!(!fetches.needs_read("avtr_a", failed));
        assert!(fetches.needs_read("avtr_a", failed + FAILED_READ_BACKOFF));
    }

    #[test]
    fn vrchat_going_forgets_the_avatar() {
        let fetches = Fetches::default();
        let (tx, _rx) = channel();
        let generation = fetches.cancel();
        fetches.deliver(generation, &tx, params());
        fetches.forget();
        assert!(!fetches.has_read_an_avatar());
        assert!(fetches.needs_read("avtr_a", Instant::now()));
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
