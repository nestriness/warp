use anyhow::Context;
use gst::prelude::*;
use std::sync::{mpsc, Arc, Mutex};
use url::Url;

use moq_transport::cache::{broadcast, fragment, segment, track};
use moq_transport::VarInt;

struct Segment {
    start_time: gst::ClockTime,
    duration: gst::ClockTime,
}

struct State {
    // We hold on to publisher so we don't close then while media is still being published.
    broadcast: broadcast::Publisher,
    start_time: Option<gst::ClockTime>,
    end_time: Option<gst::ClockTime>,
    segments: Vec<Segment>,
    url: Option<Url>,
    bitrate: u64,
    width: u64,
    height: u64,
    wave: String,
}

fn probe_encoder(enc: gst::Element) -> String {
    let (tx, rx) = mpsc::channel();

    enc.static_pad("src").unwrap().add_probe(
        gst::PadProbeType::EVENT_DOWNSTREAM,
        move |_pad, info| {
            let Some(ev) = info.event() else {
                return gst::PadProbeReturn::Ok;
            };
            let gst::EventView::Caps(ev) = ev.view() else {
                return gst::PadProbeReturn::Ok;
            };

            match gst_pbutils::codec_utils_caps_get_mime_codec(ev.caps()) {
                Ok(mime) => {
                    let _ = tx.send(mime.to_string()); // Send the MIME string

                    return gst::PadProbeReturn::Remove; // Remove the probe after getting the MIME
                }
                Err(_) => return gst::PadProbeReturn::Drop, // Drop the event on error
            }
        },
    );

    return rx.recv().unwrap();
}

pub struct GST {}

impl GST {
    pub async fn run(url: Url, mut broadcast: broadcast::Publisher) -> anyhow::Result<()> {
        gst::init()?;

        //FIXME: Get this value from commandline argument

        gstfmp4::plugin_register_static()?;

        let pipeline = gst::Pipeline::default();

        let state = Arc::new(Mutex::new(State {
            start_time: None,
            end_time: None,
            segments: Vec::new(),
            url: Some(url),
            bitrate: 2_048_000,
            width: 1280,
            height: 720,
            wave: "sine".to_string(),
            broadcast,
        }));

        let state_lock = state.lock().unwrap();

        let video_src = gst::ElementFactory::make("videotestsrc")
            .property("is-live", true)
            .build()?;

        let raw_capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst_video::VideoCapsBuilder::new()
                    .format(gst_video::VideoFormat::I420)
                    .width(state_lock.width as i32)
                    .height(state_lock.height as i32)
                    .framerate(30.into())
                    .build(),
            )
            .build()?;

        let timeoverlay = gst::ElementFactory::make("timeoverlay").build()?;

        let video_enc = gst::ElementFactory::make("x264enc")
            .property("bframes", 0u32)
            .property("bitrate", state_lock.bitrate as u32 / 1000u32)
            .property_from_str("tune", "zerolatency")
            .build()?;

        let h264_capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-h264")
                    .field("profile", "main")
                    .build(),
            )
            .build()?;

        // let audio_src = gst::ElementFactory::make("audiotestsrc")
        //     .property("is-live", true)
        //     .property_from_str("wave", &state.wave)
        //     .build()?;

        // let audio_enc = gst::ElementFactory::make("avenc_aac").build()?;

        let mux = gst::ElementFactory::make("cmafmux")
            .property_from_str("header-update-mode", "update")
            .property("write-mehd", true)
            .property("fragment-duration", 1.mseconds())
            .build()?;

        //drop the choke hold here
        drop(state_lock);

        let appsink = gst_app::AppSink::builder().buffer_list(true).build();

        pipeline.add_many([
            &video_src,
            &raw_capsfilter,
            &timeoverlay,
            &video_enc,
            &h264_capsfilter,
            // &audio_src,
            // &audio_enc,
            &mux,
            appsink.upcast_ref(),
        ])?;

        gst::Element::link_many([
            &video_src,
            &raw_capsfilter,
            &timeoverlay,
            &video_enc,
            &h264_capsfilter,
            // &audio_src,
            // &audio_enc,
            &mux,
            appsink.upcast_ref(),
        ])?;

        let video_encoder = probe_encoder(video_enc);

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let mut state = state.lock().unwrap();

                    // The muxer only outputs non-empty buffer lists
                    let mut buffer_list = sample.buffer_list_owned().expect("no buffer list");
                    assert!(!buffer_list.is_empty());

                    let mut first = buffer_list.get(0).unwrap();

                    // Each list contains a full segment, i.e. does not start with a DELTA_UNIT
                    assert!(!first.flags().contains(gst::BufferFlags::DELTA_UNIT));

                    // If the buffer has the DISCONT and HEADER flag set then it contains the media
                    // header, i.e. the `ftyp`, `moov` and other media boxes.
                    //
                    // This might be the initial header or the updated header at the end of the stream.
                    if first
                        .flags()
                        .contains(gst::BufferFlags::DISCONT | gst::BufferFlags::HEADER)
                    {
                        // Create the catalog track with a single segment.

                        // println!("writing header to {}", path.display());
                        let map = first.map_readable().unwrap();
                        // std::fs::write(path, &map).expect("failed to write header");
                        drop(map);

                        // Remove the header from the buffer list
                        buffer_list.make_mut().remove(0, 1);

                        // If the list is now empty then it only contained the media header and nothing
                        // else.
                        if buffer_list.is_empty() {
                            return Ok(gst::FlowSuccess::Ok);
                        }

                        // Otherwise get the next buffer and continue working with that.
                        first = buffer_list.get(0).unwrap();
                    }

                    // If the buffer only has the HEADER flag set then this is a segment header that is
                    // followed by one or more actual media buffers.
                    assert!(first.flags().contains(gst::BufferFlags::HEADER));

                    let segment = sample
                        .segment()
                        .expect("no segment")
                        .downcast_ref::<gst::ClockTime>()
                        .expect("no time segment");

                    // Initialize the start time with the first PTS we observed. This will be used
                    // later for calculating the duration of the whole media for the DASH manifest.
                    //
                    // The PTS of the segment header is equivalent to the earliest PTS of the whole
                    // segment.
                    let pts = segment
                        .to_running_time(first.pts().unwrap())
                        .expect("can't get running time");
                    if state.start_time.is_none() {
                        state.start_time = Some(pts);
                    }

                    // The metadata of the first media buffer is duplicated to the segment header.
                    // Based on this we can know the timecode of the first frame in this segment.
                    let meta = first
                        .meta::<gst_video::VideoTimeCodeMeta>()
                        .expect("no timecode meta");

                    // let mut path = state.path.clone();
                    // path.push(format!("segment_{}.cmfv", state.segments.len() + 1));
                    // println!(
                    //     "writing segment with timecode {} to {}",
                    //     meta.tc(),
                    //     path.display()
                    // );

                    // Calculate the end time at this point. The duration of the segment header is set
                    // to the whole duration of this segment.
                    let duration = first.duration().unwrap();
                    let end_time = first.pts().unwrap() + first.duration().unwrap();
                    state.end_time = Some(
                        segment
                            .to_running_time(end_time)
                            .expect("can't get running time"),
                    );

                    // let mut file = std::fs::File::create(path).expect("failed to open fragment");
                    for buffer in &*buffer_list {
                        use std::io::prelude::*;

                        let map = buffer.map_readable().unwrap();
                        // file.write_all(&map).expect("failed to write fragment");
                    }

                    state.segments.push(Segment {
                        start_time: pts,
                        duration,
                    });

                    Ok(gst::FlowSuccess::Ok)
                })
                .eos(move |_sink| {
                    unreachable!();
                })
                .build(),
        );

        Ok(())
    }

    //TODO: turn this into a Segment Struct 

    async fn create_segment(state: Arc<Mutex<State>>) -> anyhow::Result<()> {
        let mut state = state.lock().unwrap();

        let mut init_track = state.broadcast.create_track("0.mp4")?;

        //TODO:
        // let init_segment = {
        //     let r = init_track.create_segment(segment::Info {
        //         sequence: VarInt::ZERO,
        //         priority: 0,
        //         expires: None,
        //     });
            
        //     match r {
        //         Ok(x)=>x,
        //         Err(e)=>log::error!("Cache error", e)
        //     };
        // };

        // // Create a single fragment, optionally setting the size
		// let mut init_fragment = init_segment.final_fragment(VarInt::ZERO)?;

		// init_fragment.chunk(init.into())?;

        Ok(())
    }
}
