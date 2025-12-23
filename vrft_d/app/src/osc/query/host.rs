use axum::Router;
use log::info;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub use crate::osc::query::extensions::CalibrationStatus;

pub struct OscQueryHost;

impl OscQueryHost {
    pub async fn start(requested_port: u16, app_router: Router) -> anyhow::Result<()> {
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

        // Run Server
        axum::serve(listener, app_router).await?;

        Ok(())
    }
}
