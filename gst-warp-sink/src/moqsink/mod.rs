use glib::prelude::*;

mod imp;

glib::wrapper! {
    pub struct MoqSink(ObjectSubclass<imp::MoqSink>) @extends gst::Element, gst::Object;
}

unsafe impl Send for MoqSink {}
unsafe impl Sync for MoqSink {}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        imp::ELEMENT_NAME,
        gst::Rank::None,
        MoqSink::static_type(),
    )
}