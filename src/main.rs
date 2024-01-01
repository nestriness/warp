use anyhow::Context;
use url::Url;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use moq_transport::cache::broadcast;

mod media;
use media::*;

//TODO: add audio pipeline
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Disable tracing so we don't get a bunch of Quinn spam.
    let tracer = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::set_global_default(tracer).unwrap();

    let (publisher, subscriber) = broadcast::new("");

    // Create a list of acceptable root certificates.
    let mut roots = rustls::RootCertStore::empty();

    // Add the platform's native root certificates.
    // Add the platform's native root certificates.
    for cert in rustls_native_certs::load_native_certs().context("could not load platform certs")? {
        roots
            .add(&rustls::Certificate(cert.0))
            .context("failed to add root cert")?;
    }

    let mut tls_config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();

    tls_config.alpn_protocols = vec![webtransport_quinn::ALPN.to_vec()]; // this one is important

    let arc_tls_config = std::sync::Arc::new(tls_config);
    let quinn_client_config = quinn::ClientConfig::new(arc_tls_config);

    let mut endpoint =
        quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0))?;
    endpoint.set_default_client_config(quinn_client_config);

    let url = Url::try_from("https://localhost:4443").context("Could not get url")?;

    log::info!("connecting to relay: url={}", url);

    let session = webtransport_quinn::connect(&endpoint, &url)
        .await
        .context("failed to create WebTransport session")?;

    let session = moq_transport::session::Client::publisher(session, subscriber)
        .await
        .context("failed to create MoQ Transport session")?;

    tokio::select! {
        res = session.run() => res.context("session error")?,
        res = GST::run(publisher) => res.context("media error")?,
    }

    Ok(())
}
