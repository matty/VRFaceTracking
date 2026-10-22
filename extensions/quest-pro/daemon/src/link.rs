//! Finding the headset app's camera stream on the network, and what the API
//! asks of the connection to it.
//!
//! mDNS alone missed the headset too often: Wi-Fi drops multicast, Windows
//! Firewall can block its answers, and routers that repeat it between
//! networks confuse the headset. So the daemon also takes the address the
//! desktop app reads from the headset over adb, and in the end tries the
//! stream port on every address of the local networks.
use log::{debug, info, warn};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use std::collections::HashSet;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const SERVICE_TYPE: &str = "_vrftcam._tcp.local.";
/// How often mDNS is asked again while no headset is connected. mdns-sd
/// asks after 1, 2, 4 ... s, slowing to once an hour, so a headset app that
/// started later and whose announcement this PC missed could go unseen for
/// that long.
const REBROWSE_EVERY: Duration = Duration::from_secs(15);
/// A resolution the same as the last one passed on is mDNS answering the
/// re-ask, not news, until this long has passed.
const REPEAT_IGNORED_FOR: Duration = Duration::from_secs(60);
/// How long the desktop app's word on where the headset streams holds.
const HINT_FOR: Duration = Duration::from_secs(30);
/// Connections the network sweep opens at once.
const SWEEP_PARALLEL: usize = 64;
/// The most addresses one sweep tries.
const SWEEP_MAX_HOSTS: usize = 1024;
/// A sweep connection the local network hasn't answered in this long has
/// nothing behind it.
const SWEEP_CONNECT: Duration = Duration::from_millis(800);
/// How long the headset app takes to send its status once connected: it
/// first connects to its own camera relay, which allows 3 s.
const SWEEP_ANSWER: Duration = Duration::from_secs(4);

/// What the API asks of the headset connection, and where the desktop app
/// says the headset is.
#[derive(Default)]
pub(crate) struct Link {
    retry: AtomicBool,
    hint: Mutex<Option<(SocketAddr, Instant)>>,
    /// The open connection, so Retry can end it at once.
    open: Mutex<Option<TcpStream>>,
}

impl Link {
    /// Tries the headset again now: ends the open connection, which may be
    /// one the headset has long left, and cuts any wait short.
    pub fn retry(&self) {
        self.retry.store(true, Ordering::SeqCst);
        if let Some(stream) = self.open.lock().unwrap().as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// Whether Retry was pressed since this was last asked.
    pub fn take_retry(&self) -> bool {
        self.retry.swap(false, Ordering::SeqCst)
    }

    pub fn clear_retry(&self) {
        self.retry.store(false, Ordering::SeqCst);
    }

    /// Where the desktop app sees the headset streaming.
    pub fn hint(&self, address: SocketAddr) {
        *self.hint.lock().unwrap() = Some((address, Instant::now()));
    }

    pub fn hinted(&self) -> bool {
        self.hint.lock().unwrap().is_some()
    }

    pub fn take_hint(&self) -> Option<SocketAddr> {
        self.hint
            .lock()
            .unwrap()
            .take()
            .filter(|(_, at)| at.elapsed() < HINT_FOR)
            .map(|(address, _)| address)
    }

    /// Whether to stop waiting before the next try.
    pub fn wants_attention(&self) -> bool {
        self.retry.load(Ordering::SeqCst) || self.hinted()
    }

    /// Holds `stream` until [`Opened`] drops, so Retry can end it.
    pub fn opened(&self, stream: &TcpStream) -> Opened<'_> {
        *self.open.lock().unwrap() = stream.try_clone().ok();
        Opened(self)
    }
}

/// Forgets the open connection as it drops.
pub(crate) struct Opened<'a>(&'a Link);

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        self.0.open.lock().unwrap().take();
    }
}

/// mDNS browsing for the headset app, on a thread of its own. mdns-sd hands
/// events over a short queue and stops when it's full, so they are read all
/// the time, even while connected, and passed on here.
pub(crate) struct Mdns {
    events: mpsc::Receiver<ServiceEvent>,
    searching: Arc<AtomicBool>,
}

impl Mdns {
    pub fn start(running: Arc<AtomicBool>) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let receiver = daemon.browse(SERVICE_TYPE)?;
        let (sender, events) = mpsc::channel();
        let searching = Arc::new(AtomicBool::new(true));
        let watching = searching.clone();
        thread::Builder::new()
            .name("quest-pro-mdns".into())
            .spawn(move || watch(daemon, receiver, sender, &watching, &running))
            .map_err(|error| mdns_sd::Error::Msg(error.to_string()))?;
        Ok(Self { events, searching })
    }

    /// Whether the headset is being looked for, which keeps asking mDNS.
    pub fn set_searching(&self, searching: bool) {
        self.searching.store(searching, Ordering::SeqCst);
    }

    /// The next service found or removed, waiting at most `timeout`.
    pub fn next(&self, timeout: Duration) -> Option<ServiceEvent> {
        self.events.recv_timeout(timeout).ok()
    }
}

fn watch(
    daemon: ServiceDaemon,
    mut receiver: mdns_sd::Receiver<ServiceEvent>,
    sender: mpsc::Sender<ServiceEvent>,
    searching: &AtomicBool,
    running: &AtomicBool,
) {
    let mut browsed_at = Instant::now();
    // The last resolution passed on, and when.
    let mut last: Option<(Resolution, Instant)> = None;
    while running.load(Ordering::SeqCst) {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let resolution = Resolution::of(&info);
                let repeat = last.as_ref().is_some_and(|(seen, at)| {
                    *seen == resolution && at.elapsed() < REPEAT_IGNORED_FOR
                });
                if !repeat {
                    last = Some((resolution, Instant::now()));
                    if sender.send(ServiceEvent::ServiceResolved(info)).is_err() {
                        break;
                    }
                }
            }
            Ok(event @ ServiceEvent::ServiceRemoved(..)) => {
                // Coming back after this is news, even unchanged.
                last = None;
                if sender.send(event).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) if receiver.is_disconnected() => {
                thread::sleep(Duration::from_millis(500));
                // Browse again straight away.
                browsed_at = Instant::now() - REBROWSE_EVERY;
            }
            Err(_) => {}
        }
        if searching.load(Ordering::SeqCst) && browsed_at.elapsed() >= REBROWSE_EVERY {
            // Stopping first drops the old query's slowed schedule and its
            // cached records, so what comes back is current.
            let _ = daemon.stop_browse(SERVICE_TYPE);
            match daemon.browse(SERVICE_TYPE) {
                Ok(fresh) => receiver = fresh,
                Err(error) => debug!("Quest Pro camera: couldn't ask mDNS again ({error})"),
            }
            browsed_at = Instant::now();
        }
    }
    let _ = daemon.shutdown();
}

/// What a resolution of the headset app says: where it is and which version.
/// Re-asking mDNS brings the same answer back every [`REBROWSE_EVERY`];
/// passing each on would cut every wait between retries short and keep the
/// backoff from growing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolution {
    addresses: Vec<IpAddr>,
    port: u16,
    properties: [Option<String>; 3],
}

impl Resolution {
    fn of(info: &ResolvedService) -> Self {
        let mut addresses: Vec<IpAddr> = info
            .get_addresses()
            .iter()
            .map(|ip| ip.to_ip_addr())
            .collect();
        addresses.sort();
        addresses.dedup();
        let property = |key: &str| info.get_property_val_str(key).map(str::to_owned);
        Self {
            addresses,
            port: info.get_port(),
            properties: [
                property("protocol"),
                property("version"),
                property("apk_version"),
            ],
        }
    }
}

/// A local IPv4 network this PC is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Network {
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

impl Network {
    fn mask(prefix: u8) -> u32 {
        u32::MAX
            .checked_shl(32 - u32::from(prefix.min(32)))
            .unwrap_or(0)
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = Self::mask(self.prefix);
        u32::from(self.ip) & mask == u32::from(ip) & mask
    }

    /// The addresses a sweep tries: every host of the network but this PC,
    /// within the /24 around it when the network is larger.
    pub fn sweep_hosts(&self) -> impl Iterator<Item = Ipv4Addr> {
        let prefix = self.prefix.max(24);
        let mask = Self::mask(prefix);
        let network = u32::from(self.ip) & mask;
        let broadcast = network | !mask;
        let own = self.ip;
        // A /31 or /32 has no hosts to look for.
        let (first, last) = if prefix >= 31 {
            (1, 0)
        } else {
            (network + 1, broadcast - 1)
        };
        (first..=last)
            .map(Ipv4Addr::from)
            .filter(move |ip| *ip != own)
    }
}

/// The private IPv4 networks of this PC's working interfaces.
pub(crate) fn local_networks() -> Vec<Network> {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_else(|error| {
        debug!("Quest Pro camera: couldn't list the network interfaces ({error})");
        Vec::new()
    });
    let mut networks = Vec::new();
    for interface in interfaces {
        let if_addrs::IfAddr::V4(v4) = &interface.addr else {
            continue;
        };
        if !interface.is_oper_up() || !v4.ip.is_private() {
            continue;
        }
        let network = Network {
            ip: v4.ip,
            prefix: v4.prefixlen,
        };
        if !networks.contains(&network) {
            networks.push(network);
        }
    }
    networks
}

/// `ips` at `port`, in the order to try them: those on a network this PC is
/// on first, then other usable ones. An advert can list several, including
/// one the headset had before.
pub(crate) fn order_addresses(
    ips: impl IntoIterator<Item = Ipv4Addr>,
    port: u16,
    networks: &[Network],
) -> Vec<SocketAddr> {
    let mut ips: Vec<Ipv4Addr> = ips
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
        .collect();
    ips.dedup();
    ips.sort_by_key(|ip| {
        let local = networks.iter().any(|network| network.contains(*ip));
        (!local, ip.is_link_local())
    });
    let mut addresses: Vec<SocketAddr> = Vec::new();
    for ip in ips {
        let address = SocketAddr::new(IpAddr::V4(ip), port);
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// Looks for the headset app on the local networks by connecting to `port`
/// on every address and keeping the first that answers as the headset app
/// does, with its status. Takes a few seconds. Only outgoing connections, so
/// a firewall that blocks mDNS doesn't block this.
pub(crate) fn sweep(port: u16, running: &AtomicBool) -> Option<SocketAddr> {
    let networks = local_networks();
    let mut hosts: Vec<SocketAddr> = Vec::new();
    for network in &networks {
        for ip in network.sweep_hosts() {
            let address = SocketAddr::new(IpAddr::V4(ip), port);
            if hosts.len() < SWEEP_MAX_HOSTS && !hosts.contains(&address) {
                hosts.push(address);
            }
        }
    }
    if hosts.is_empty() {
        debug!("Quest Pro camera: no local network to look for the headset on");
        return None;
    }
    info!(
        "Quest Pro camera: looking for the headset app at port {port} on {} addresses of {}",
        hosts.len(),
        networks
            .iter()
            .map(|network| format!("{}/{}", network.ip, network.prefix))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let found = Mutex::new(None);
    for chunk in hosts.chunks(SWEEP_PARALLEL) {
        if !running.load(Ordering::SeqCst) || found.lock().unwrap().is_some() {
            break;
        }
        thread::scope(|scope| {
            for &address in chunk {
                let found = &found;
                scope.spawn(move || {
                    if answers_as_headset_app(address) {
                        found.lock().unwrap().get_or_insert(address);
                    }
                });
            }
        });
    }
    found.into_inner().unwrap()
}

/// Whether the headset app's stream is at `address`: it sends its status
/// first on every connection.
pub(crate) fn answers_as_headset_app(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, SWEEP_CONNECT) else {
        return false;
    };
    if stream.set_read_timeout(Some(SWEEP_ANSWER)).is_err() {
        return false;
    }
    let mut magic = [0u8; 8];
    let answered = stream.read_exact(&mut magic).is_ok() && &magic == b"QPSTAT1\0";
    if !answered {
        // Sweeps repeat every half minute; say so once per address.
        static WARNED: Mutex<Option<HashSet<SocketAddr>>> = Mutex::new(None);
        let first = WARNED
            .lock()
            .unwrap()
            .get_or_insert_with(HashSet::new)
            .insert(address);
        if first {
            warn!("Quest Pro camera: {address} takes connections on the headset app's port but didn't answer as the headset app");
        } else {
            debug!("Quest Pro camera: {address} again didn't answer as the headset app");
        }
    }
    answered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn sweeps_cover_the_network_without_this_pc() {
        let network = Network {
            ip: Ipv4Addr::new(192, 168, 1, 20),
            prefix: 24,
        };
        let hosts: Vec<_> = network.sweep_hosts().collect();
        assert_eq!(hosts.len(), 253);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(hosts[252], Ipv4Addr::new(192, 168, 1, 254));
        assert!(!hosts.contains(&network.ip));
    }

    #[test]
    fn large_networks_are_swept_around_this_pc() {
        let network = Network {
            ip: Ipv4Addr::new(10, 1, 7, 9),
            prefix: 8,
        };
        let hosts: Vec<_> = network.sweep_hosts().collect();
        assert_eq!(hosts.len(), 253);
        assert!(hosts.iter().all(|ip| ip.octets()[..3] == [10, 1, 7]));
        assert!(network.contains(Ipv4Addr::new(10, 200, 0, 1)));
        assert!(!network.contains(Ipv4Addr::new(11, 0, 0, 1)));
        let point_to_point = Network {
            ip: Ipv4Addr::new(10, 0, 0, 1),
            prefix: 32,
        };
        assert_eq!(point_to_point.sweep_hosts().count(), 0);
    }

    #[test]
    fn addresses_on_this_pcs_networks_come_first() {
        let networks = [Network {
            ip: Ipv4Addr::new(192, 168, 1, 5),
            prefix: 24,
        }];
        let ordered = order_addresses(
            [
                Ipv4Addr::new(169, 254, 3, 4),
                Ipv4Addr::new(10, 0, 0, 7),
                Ipv4Addr::new(192, 168, 1, 20),
                Ipv4Addr::LOCALHOST,
            ],
            27274,
            &networks,
        );
        let ordered: Vec<String> = ordered.iter().map(ToString::to_string).collect();
        assert_eq!(
            ordered,
            ["192.168.1.20:27274", "10.0.0.7:27274", "169.254.3.4:27274"]
        );
    }

    #[test]
    fn only_the_headset_apps_status_counts_as_an_answer() {
        let headset = TcpListener::bind("127.0.0.1:0").unwrap();
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let (headset_at, other_at) = (headset.local_addr().unwrap(), other.local_addr().unwrap());
        let serve = |listener: TcpListener, reply: &'static [u8]| {
            thread::spawn(move || {
                let (mut client, _) = listener.accept().unwrap();
                client.write_all(reply).unwrap();
            })
        };
        let a = serve(headset, b"QPSTAT1\0\x01\0\0\0\x02\0\0\0{}");
        let b = serve(other, b"HTTP/1.1 400 Bad Request\r\n");
        assert!(answers_as_headset_app(headset_at));
        assert!(!answers_as_headset_app(other_at));
        a.join().unwrap();
        b.join().unwrap();
    }

    #[test]
    fn retry_ends_the_open_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let _held = listener.accept().unwrap();
        let link = Link::default();
        let opened = link.opened(&stream);
        link.retry();
        assert_eq!(stream.read(&mut [0u8; 1]).unwrap_or(0), 0);
        assert!(link.take_retry());
        drop(opened);
        assert!(link.open.lock().unwrap().is_none());
    }

    #[test]
    fn hints_wake_the_wait_once() {
        let link = Link::default();
        assert!(!link.wants_attention());
        let address: SocketAddr = "192.168.1.20:27274".parse().unwrap();
        link.hint(address);
        assert!(link.wants_attention());
        assert_eq!(link.take_hint(), Some(address));
        assert_eq!(link.take_hint(), None);
        assert!(!link.wants_attention());
    }
}
