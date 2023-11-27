use gst::prelude::*;

fn main() {
    // Initialize GStreamer
    gst::init().unwrap();

    // Create the elements
    let source = gst::ElementFactory::make("waylanddisplaysrc", Some("source")).unwrap();
    let encoder = gst::ElementFactory::make("x264enc", Some("encoder")).unwrap();
    let muxer = gst::ElementFactory::make("fmp4mux", Some("muxer")).unwrap();
    // let sink = gst::ElementFactory::make("warpsink", Some("sink")).unwrap();
    let sink = gst::ElementFactory::make("autovideosink", Some("sink")).unwrap();

    // Create an empty pipeline
    let pipeline = gst::Pipeline::new(Some("pipeline"));

    // Build the pipeline
    pipeline.add_many(&[&source, &encoder, &muxer, &sink]).unwrap();
    gst::Element::link_many(&[&source, &encoder, &muxer, &sink]).unwrap();

    // Start playing
    pipeline.set_state(gst::State::Playing).unwrap();

    // Wait until error or EOS
    let bus = pipeline.bus().unwrap();
    for msg in bus.iter_timed(gst::CLOCK_TIME_NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Error(err) => {
                eprintln!(
                    "Error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                break;
            }
            MessageView::Eos(..) => break,
            _ => (),
        }
    }

    // Clean up
    pipeline.set_state(gst::State::Null).unwrap();
}