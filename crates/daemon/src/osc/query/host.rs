use axum::Router;
use log::info;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub struct OscQueryHost;

impl OscQueryHost {
    /// Serves `app_router` and advertises it. `osc_port` is the UDP port OSC
    /// arrives on, advertised too so VRChat sends its output there, whichever
    /// other apps are listening for it.
    pub async fn start(
        requested_port: u16,
        app_router: Router,
        osc_port: Option<u16>,
    ) -> anyhow::Result<()> {
        // Bind to Port (0 for dynamic)
        let addr = SocketAddr::from(([0, 0, 0, 0], requested_port));
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let actual_port = local_addr.port();

        info!("OSC Query Host listening on http://{}", local_addr);

        // Advertise via mDNS
        let mdns = ServiceDaemon::new()?;
        let service_type = "_oscjson._tcp.local.";
        let instance_name = "VRFT";
        let host_name = format!("vrft_rs_{}.local.", actual_port);

        let properties = [("txtvers", "1")];

        let service_info = ServiceInfo::new(
            service_type,
            instance_name,
            &host_name,
            "",
            actual_port,
            &properties[..],
        )?
        .enable_addr_auto();

        mdns.register(service_info)?;
        info!(
            "Advertised OSC Query Service via mDNS: {} on port {}",
            instance_name, actual_port
        );

        if let Some(osc_port) = osc_port.filter(|port| *port != 0) {
            let osc_service = ServiceInfo::new(
                "_osc._udp.local.",
                instance_name,
                &host_name,
                "",
                osc_port,
                &properties[..],
            )?
            .enable_addr_auto();
            mdns.register(osc_service)?;
            info!(
                "Advertised OSC Service via mDNS: {} on port {}",
                instance_name, osc_port
            );
        }

        // Run Server
        axum::serve(listener, app_router).await?;

        Ok(())
    }
}
