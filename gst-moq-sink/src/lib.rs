use gst::glib;
use gst::glib::once_cell::sync::Lazy;
use tokio::runtime;

mod moqsink;
mod relayurl;

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    moqsink::register(plugin)?;
    Ok(())
}

gst::plugin_define!(
    moqsink,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("COMMIT_ID")),
    "unknown",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    env!("BUILD_REL_DATE")
);

//TODO: https://github.com/sdroege/gst-plugin-rs/blob/1faef49b51746a82c5b99a09a1034220cf92af9d/net/webrtc/src/lib.rs#L44C1-L50C4
pub static RUNTIME: Lazy<runtime::Runtime> = Lazy::new(|| {
    runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap()
});