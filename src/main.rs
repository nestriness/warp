use anyhow::Error;
use gst::prelude::*;

fn main() -> Result<(), Error> {
    gst::init()?;
    gstfmp4::plugin_register_static().expect("Failed to register fmp4 plugin");

    let pipeline = gst::parse_launch("videotestsrc num-buffers=2500 ! timecodestamper ! video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! videoconvert ! queue ! x264enc tune=zerolatency key-int-max=30 ! mp4mux streamable=true fragment-duration=1 ! ! appsink name=sink ").unwrap().downcast::<gst::Pipeline>().unwrap();

    let sink = pipeline
        .by_name("sink")
        .unwrap()
        .dynamic_cast::<gst_app::AppSink>()
        .unwrap();
    sink.set_buffer_list(true);

    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                // let mut state = state.lock().unwrap();

                // // The muxer only outputs non-empty buffer lists

                //TODO: figure out how to iterate over all the buffers and then do this

                let mut buffer_list = sample.buffer_list_owned().expect("no buffer list");
                let map = buffer.map_readable().map_err(|_| {
                    gst::element_error!(element, gst::CoreError::Failed, ["Failed to map buffer"]);
                    gst::FlowError::Error
                })?;
                let input_buf = map.as_ref();
                // assert!(!buffer_list.is_empty());

                // let mut first = buffer_list.get(0).unwrap();

                // // Each list contains a full segment, i.e. does not start with a DELTA_UNIT
                // assert!(!first.flags().contains(gst::BufferFlags::DELTA_UNIT));

                // // If the buffer has the DISCONT and HEADER flag set then it contains the media
                // // header, i.e. the `ftyp`, `moov` and other media boxes.
                // //
                // // This might be the initial header or the updated header at the end of the stream.
                // if first
                //     .flags()
                //     .contains(gst::BufferFlags::DISCONT | gst::BufferFlags::HEADER)
                // {
                //     let mut path = state.path.clone();
                //     std::fs::create_dir_all(&path).expect("failed to create directory");
                //     path.push("init.cmfi");

                //     println!("writing header to {}", path.display());
                //     let map = first.map_readable().unwrap();
                //     std::fs::write(path, &map).expect("failed to write header");
                //     drop(map);

                //     // Remove the header from the buffer list
                //     buffer_list.make_mut().remove(0, 1);

                //     // If the list is now empty then it only contained the media header and nothing
                //     // else.
                //     if buffer_list.is_empty() {
                //         return Ok(gst::FlowSuccess::Ok);
                //     }

                //     // Otherwise get the next buffer and continue working with that.
                //     first = buffer_list.get(0).unwrap();
                // }

                // // If the buffer only has the HEADER flag set then this is a segment header that is
                // // followed by one or more actual media buffers.
                // assert!(first.flags().contains(gst::BufferFlags::HEADER));

                // let segment = sample
                //     .segment()
                //     .expect("no segment")
                //     .downcast_ref::<gst::ClockTime>()
                //     .expect("no time segment");

                // // Initialize the start time with the first PTS we observed. This will be used
                // // later for calculating the duration of the whole media for the DASH manifest.
                // //
                // // The PTS of the segment header is equivalent to the earliest PTS of the whole
                // // segment.
                // let pts = segment
                //     .to_running_time(first.pts().unwrap())
                //     .expect("can't get running time");
                // if state.start_time.is_none() {
                //     state.start_time = Some(pts);
                // }

                // // The metadata of the first media buffer is duplicated to the segment header.
                // // Based on this we can know the timecode of the first frame in this segment.
                // let meta = first
                //     .meta::<gst_video::VideoTimeCodeMeta>()
                //     .expect("no timecode meta");

                // let mut path = state.path.clone();
                // path.push(format!("segment_{}.cmfv", state.segments.len() + 1));
                // println!(
                //     "writing segment with timecode {} to {}",
                //     meta.tc(),
                //     path.display()
                // );

                // // Calculate the end time at this point. The duration of the segment header is set
                // // to the whole duration of this segment.
                // let duration = first.duration().unwrap();
                // let end_time = first.pts().unwrap() + first.duration().unwrap();
                // state.end_time = Some(
                //     segment
                //         .to_running_time(end_time)
                //         .expect("can't get running time"),
                // );

                // let mut file = std::fs::File::create(path).expect("failed to open fragment");
                // for buffer in &*buffer_list {
                //     use std::io::prelude::*;

                //     let map = buffer.map_readable().unwrap();
                //     file.write_all(&map).expect("failed to write fragment");
                // }

                // state.segments.push(Segment {
                //     start_time: pts,
                //     duration,
                // });

                Ok(gst::FlowSuccess::Ok)
            })
            .eos(move |_sink| {
//                 let state = state_clone.lock().unwrap();

//                 // Now write the manifest
//                 let mut path = state.path.clone();
//                 path.push("manifest.mpd");

//                 println!("writing manifest to {}", path.display());

//                 let duration = state
//                     .end_time
//                     .opt_checked_sub(state.start_time)
//                     .ok()
//                     .flatten()
//                     .unwrap()
//                     .mseconds();

//                 // Write the whole segment timeline out here, compressing multiple segments with
//                 // the same duration to a repeated segment.
//                 let mut segments = vec![];
//                 let mut write_segment =
//                     |start: gst::ClockTime, duration: gst::ClockTime, repeat: usize| {
//                         let mut s = dash_mpd::S {
//                             t: Some(start.mseconds() as i64),
//                             d: duration.mseconds() as i64,
//                             ..Default::default()
//                         };
//                         if repeat > 0 {
//                             s.r = Some(repeat as i64);
//                         }

//                         segments.push(s);
//                     };

//                 let mut start = None;
//                 let mut num_segments = 0;
//                 let mut last_duration = None;
//                 for segment in &state.segments {
//                     if start.is_none() {
//                         start = Some(segment.start_time);
//                     }
//                     if last_duration.is_none() {
//                         last_duration = Some(segment.duration);
//                     }

//                     // If the duration of this segment is different from the previous one then we
//                     // have to write out the segment now.
//                     if last_duration != Some(segment.duration) {
//                         write_segment(start.unwrap(), last_duration.unwrap(), num_segments - 1);
//                         start = Some(segment.start_time);
//                         last_duration = Some(segment.duration);
//                         num_segments = 1;
//                     } else {
//                         num_segments += 1;
//                     }
//                 }

//                 // Write the last segment if any
//                 if num_segments > 0 {
//                     write_segment(start.unwrap(), last_duration.unwrap(), num_segments - 1);
//                 }

//                 let segment_template = dash_mpd::SegmentTemplate {
//                     timescale: Some(1000),
//                     initialization: Some("init.cmfi".to_string()),
//                     media: Some("segment_$Number$.cmfv".to_string()),
//                     SegmentTimeline: Some(dash_mpd::SegmentTimeline { segments }),
//                     ..Default::default()
//                 };

//                 let rep = dash_mpd::Representation {
//                     id: Some("A".to_string()),
//                     width: Some(1280),
//                     height: Some(720),
//                     bandwidth: Some(2048000),
//                     SegmentTemplate: Some(segment_template),
//                     ..Default::default()
//                 };

//                 let adapt = dash_mpd::AdaptationSet {
//                     contentType: Some("video".to_string()),
//                     mimeType: Some("video/mp4".to_string()),
//                     codecs: Some("avc1.4d0228".to_string()),
//                     frameRate: Some("30/1".to_string()),
//                     segmentAlignment: Some(true),
//                     subsegmentStartsWithSAP: Some(1),
//                     representations: vec![rep],
//                     ..Default::default()
//                 };

//                 let period = dash_mpd::Period {
//                     adaptations: vec![adapt],
//                     ..Default::default()
//                 };

//                 let mpd = dash_mpd::MPD {
//                     mpdtype: Some("static".to_string()),
//                     xmlns: Some("urn:mpeg:dash:schema:mpd:2011".to_string()),
//                     schemaLocation: Some("urn:mpeg:dash:schema:mpd:2011 DASH-MPD.xsd".to_string()),
//                     profiles: Some("urn:mpeg:dash:profile:isoff-on-demand:2011".to_string()),
//                     periods: vec![period],
//                     mediaPresentationDuration: Some(std::time::Duration::from_millis(duration)),
//                     minBufferTime: Some(std::time::Duration::from_secs(1)),
//                     ..Default::default()
//                 };

//                 use serde::ser::Serialize;

//                 let mut xml = String::new();
//                 let mut ser = quick_xml::se::Serializer::new(&mut xml);
//                 ser.indent(' ', 4);
//                 mpd.serialize(ser).unwrap();

//                 let manifest = format!(
//                     r###"<?xml version="1.0" encoding="UTF-8"?>
// {xml}
// "###
//                 );

//                 std::fs::write(path, manifest).expect("failed to write manifest");
            })
            .build(),
    );

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