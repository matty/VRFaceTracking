//! Where VRChat tracking goes, and what OSCQuery has found of VRChat. Shared
//! between the sender, mDNS discovery and the daemon's status report.
use std::net::{IpAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, RwLock};
use vrft_protocol::VrchatLink;

#[derive(Debug)]
struct State {
    host: String,
    configured_port: u16,
    found: bool,
    /// The OSC port the found VRChat reports it listens on.
    discovered_port: Option<u16>,
    /// Whether the avatar has parameters this application drives.
    avatar_face_tracking: Option<bool>,
    /// Whether VRChat reports the avatar takes native face tracking.
    avatar_native_face_tracking: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct VrchatTarget {
    state: Arc<RwLock<State>>,
}

impl VrchatTarget {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            state: Arc::new(RwLock::new(State {
                host: host.to_string(),
                configured_port: port,
                found: false,
                discovered_port: None,
                avatar_face_tracking: None,
                avatar_native_face_tracking: None,
            })),
        }
    }

    /// `host:port` to send to: the configured host, at the port VRChat
    /// reports once found, else the configured port.
    pub fn send_addr(&self) -> String {
        let state = self.state.read().unwrap();
        let port = state.discovered_port.unwrap_or(state.configured_port);
        format!("{}:{}", state.host, port)
    }

    pub fn link(&self) -> VrchatLink {
        let state = self.state.read().unwrap();
        VrchatLink {
            found: state.found,
            // Either way in counts; an avatar may have only native tracking.
            avatar_face_tracking: match state.avatar_native_face_tracking {
                Some(true) => Some(true),
                _ => state.avatar_face_tracking,
            },
            sending_to: format!(
                "{}:{}",
                state.host,
                state.discovered_port.unwrap_or(state.configured_port)
            ),
        }
    }

    /// Whether a VRChat reachable at `addresses` is the one at the configured
    /// host. A loopback host means this PC, so any of its own addresses
    /// match; VRChat advertises its network addresses, not loopback.
    pub fn matches(&self, addresses: &[IpAddr]) -> bool {
        let host = self.state.read().unwrap().host.clone();
        let wanted = resolve(&host);
        if wanted.iter().any(IpAddr::is_loopback) {
            addresses
                .iter()
                .any(|ip| ip.is_loopback() || is_own_address(*ip))
        } else {
            addresses.iter().any(|ip| wanted.contains(ip))
        }
    }

    pub fn found(&self, osc_port: Option<u16>) {
        let mut state = self.state.write().unwrap();
        state.found = true;
        state.discovered_port = osc_port.filter(|port| *port != 0);
    }

    pub fn lost(&self) {
        let mut state = self.state.write().unwrap();
        state.found = false;
        state.discovered_port = None;
        state.avatar_face_tracking = None;
        state.avatar_native_face_tracking = None;
    }

    pub fn set_avatar_face_tracking(&self, face_tracking: Option<bool>) {
        self.state.write().unwrap().avatar_face_tracking = face_tracking;
    }

    /// A new avatar: nothing is known about it until VRChat says.
    pub fn avatar_changed(&self) {
        let mut state = self.state.write().unwrap();
        state.avatar_face_tracking = None;
        state.avatar_native_face_tracking = None;
    }

    /// What VRChat pushed over OSC, which is always about the current avatar.
    pub fn set_avatar_native_face_tracking(&self, native: bool) {
        self.state.write().unwrap().avatar_native_face_tracking = Some(native);
    }

    /// What an OSCQuery read found. That covers an avatar loaded before we
    /// started, when no OSC message is coming, but the read may have been of
    /// the previous avatar, so it never overrides what OSC said.
    pub fn offer_avatar_native_face_tracking(&self, native: bool) {
        self.state
            .write()
            .unwrap()
            .avatar_native_face_tracking
            .get_or_insert(native);
    }

    /// Whether the current avatar takes `/tracking/face/v1`.
    pub fn native_face_tracking(&self) -> bool {
        self.state.read().unwrap().avatar_native_face_tracking == Some(true)
    }

    /// The address of this PC that reaches the configured host, for telling
    /// VRChat where to send back to. Loopback when VRChat is on this PC.
    pub fn reply_ip(&self) -> IpAddr {
        let host = self.state.read().unwrap().host.clone();
        let loopback = IpAddr::from([127, 0, 0, 1]);
        let Some(remote) = resolve(&host).into_iter().find(IpAddr::is_ipv4) else {
            return loopback;
        };
        if remote.is_loopback() {
            return loopback;
        }
        UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| {
                socket.connect((remote, 9))?;
                socket.local_addr()
            })
            .map(|local| local.ip())
            .unwrap_or(loopback)
    }
}

fn resolve(host: &str) -> Vec<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }
    (host, 0)
        .to_socket_addrs()
        .map(|addrs| addrs.map(|addr| addr.ip()).collect())
        .unwrap_or_default()
}

/// An address belongs to this PC when a socket can be bound to it.
fn is_own_address(ip: IpAddr) -> bool {
    UdpSocket::bind((ip, 0)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_to_the_configured_port_until_vrchat_reports_one() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        assert_eq!(target.send_addr(), "127.0.0.1:9000");
        target.found(Some(9010));
        assert_eq!(target.send_addr(), "127.0.0.1:9010");
        assert!(target.link().found);
        target.lost();
        assert_eq!(target.send_addr(), "127.0.0.1:9000");
        assert!(!target.link().found);
    }

    #[test]
    fn a_found_vrchat_without_a_port_keeps_the_configured_one() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.found(None);
        assert_eq!(target.send_addr(), "127.0.0.1:9000");
        assert!(target.link().found);
    }

    #[test]
    fn loopback_matches_this_pc_but_not_another() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        assert!(target.matches(&[IpAddr::from([127, 0, 0, 1])]));
        // TEST-NET-1, never assigned to a real PC.
        assert!(!target.matches(&[IpAddr::from([192, 0, 2, 10])]));
    }

    #[test]
    fn a_remote_host_matches_only_its_own_address() {
        let target = VrchatTarget::new("192.0.2.10", 9000);
        assert!(target.matches(&[IpAddr::from([192, 0, 2, 10])]));
        assert!(!target.matches(&[IpAddr::from([192, 0, 2, 11])]));
    }

    #[test]
    fn losing_vrchat_forgets_the_avatar() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.found(Some(9000));
        target.set_avatar_face_tracking(Some(true));
        target.set_avatar_native_face_tracking(true);
        target.lost();
        assert_eq!(target.link().avatar_face_tracking, None);
        assert!(!target.native_face_tracking());
    }

    #[test]
    fn a_native_only_avatar_counts_as_having_face_tracking() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.set_avatar_face_tracking(Some(false));
        target.set_avatar_native_face_tracking(true);
        assert_eq!(target.link().avatar_face_tracking, Some(true));
        assert!(target.native_face_tracking());
    }

    #[test]
    fn an_avatar_with_only_parameters_still_counts() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.set_avatar_face_tracking(Some(true));
        target.set_avatar_native_face_tracking(false);
        assert_eq!(target.link().avatar_face_tracking, Some(true));
        assert!(!target.native_face_tracking());
    }

    #[test]
    fn an_avatar_with_neither_has_no_face_tracking() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.set_avatar_face_tracking(Some(false));
        target.set_avatar_native_face_tracking(false);
        assert_eq!(target.link().avatar_face_tracking, Some(false));
    }

    #[test]
    fn a_read_fills_the_gap_but_never_overrides_what_osc_said() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.offer_avatar_native_face_tracking(true);
        assert!(target.native_face_tracking(), "nothing heard over OSC yet");

        target.avatar_changed();
        target.set_avatar_native_face_tracking(false);
        target.offer_avatar_native_face_tracking(true);
        assert!(
            !target.native_face_tracking(),
            "a stale read of the last avatar"
        );
    }

    #[test]
    fn osc_corrects_an_earlier_read() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.offer_avatar_native_face_tracking(true);
        target.set_avatar_native_face_tracking(false);
        assert!(!target.native_face_tracking());
    }

    #[test]
    fn an_avatar_change_forgets_both_kinds_of_face_tracking() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.set_avatar_face_tracking(Some(true));
        target.set_avatar_native_face_tracking(true);
        target.avatar_changed();
        assert_eq!(target.link().avatar_face_tracking, None);
        assert!(!target.native_face_tracking());
    }
}
