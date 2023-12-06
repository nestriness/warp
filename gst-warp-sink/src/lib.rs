mod moqsink;

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