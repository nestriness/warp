use anyhow::Context;
use std::{fs, io, sync::Arc, time};
use url::{ParseError, Url};

use clap::Parser;
use moq_transport::cache::broadcast;

mod cli;
use cli::*;

mod media;
use media::*;

// TODO: clap complete
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Disable tracing so we don't get a bunch of Quinn spam.
    let tracer = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::set_global_default(tracer).unwrap();

    let config = Config::parse();

    let (publisher, subscriber) = broadcast::new("");

    // Create a list of acceptable root certificates.
    let mut roots = rustls::RootCertStore::empty();

    // Add the platform's native root certificates.
    // Add the platform's native root certificates.
    if config.tls_root.is_empty() {
        // Add the platform's native root certificates.
        for cert in
            rustls_native_certs::load_native_certs().context("could not load platform certs")?
        {
            roots
                .add(&rustls::Certificate(cert.0))
                .context("failed to add root cert")?;
        }
    } else {
        // Add the specified root certificates.
        for root in &config.tls_root {
            let root = fs::File::open(root).context("failed to open root cert file")?;
            let mut root = io::BufReader::new(root);

            let root = rustls_pemfile::certs(&mut root).context("failed to read root cert")?;
            anyhow::ensure!(root.len() == 1, "expected a single root cert");
            let root = rustls::Certificate(root[0].to_owned());

            roots.add(&root).context("failed to add root cert")?;
        }
    }

    let mut tls_config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();

    // Allow disabling TLS verification altogether.
    if config.tls_disable_verify {
        let noop = NoCertificateVerification {};
        tls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(noop));
    }

    tls_config.alpn_protocols = vec![webtransport_quinn::ALPN.to_vec()]; // this one is important

    let arc_tls_config = std::sync::Arc::new(tls_config);
    let quinn_client_config = quinn::ClientConfig::new(arc_tls_config);

    let mut endpoint = quinn::Endpoint::client(config.bind)?;
    endpoint.set_default_client_config(quinn_client_config);

    println!("connecting to relay: url={}", config.url);

    let view_url = get_view_url(&config.url.to_string())?;

    println!("watch the video at: url={}", view_url);

    let session = webtransport_quinn::connect(&endpoint, &config.url)
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

pub struct NoCertificateVerification {}

impl rustls::client::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

fn get_view_url(original_url: &str) -> Result<String, ParseError> {
    // let original_url = "https://relay.quic.video/wzV69467F2KaPgTC";
    let url = Url::parse(original_url)?;

    // Get the last path segment
    let segments = url.path_segments().ok_or_else(|| ParseError::EmptyHost)?;
    let last_segment = segments.last().ok_or_else(|| ParseError::EmptyHost)?;

    // Construct the new URL
    let new_base = "https://quic.video/watch/";
    let new_url = format!("{}{}", new_base, last_segment);

    Ok(new_url)
}
