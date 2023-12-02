use anyhow::Error;
use gst::prelude::*;

fn main() -> Result<(), Error> {
    gst::init()?;
    gstfmp4::plugin_register_static().expect("Failed to register fmp4 plugin");

    let pipeline = gst::parse_launch("videotestsrc num-buffers=2500 ! timecodestamper ! video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! timeoverlay ! x264enc bframes=0 bitrate=2048 ! video/x-h264,profile=main ! cmafmux fragment-duration=10000000000 header-update-mode=update write-mehd=true ! testsink").unwrap().downcast::<gst::Pipeline>().unwrap();

    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline
        .bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                println!("EOS");
                break;
            }
            MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null)?;
                eprintln!(
                    "Got error from {}: {} ({})",
                    msg.src()
                        .map(|s| String::from(s.path_string()))
                        .unwrap_or_else(|| "None".into()),
                    err.error(),
                    err.debug().unwrap_or_else(|| "".into()),
                );
                break;
            }
            _ => (),
        }
    }

    pipeline.set_state(gst::State::Null)?;

    Ok(())
}
