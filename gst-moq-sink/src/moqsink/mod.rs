use gst::glib;
use gst::prelude::*;

mod imp;

glib::wrapper! {
    pub struct MoqSink(ObjectSubclass<imp::MoqSink>) @extends gst_base::BaseSink, gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        imp::ELEMENT_NAME,
        gst::Rank::PRIMARY,
        MoqSink::static_type(),
    )
}