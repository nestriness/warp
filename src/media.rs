// #![allow(dead_code)]
// #![allow(unused_imports)]
// #![allow(unused_mut)]
// #![allow(unused_assignments)]
// #![allow(unused_variables)]
use anyhow::{self, Context, Error};
use gst::prelude::*;
use gstreamer_audio::AudioCapsBuilder;
use std::sync::{Arc, Mutex};

use moq_transport::cache::{broadcast, fragment, segment, track};
use moq_transport::VarInt;
use serde_json::json;
use std::cmp::max;
use std::collections::HashMap;
use std::io::Cursor;
use std::time;
use uuid::Uuid;

use mp4::{self, ReadBox};

const ATOM_TYPE_FTYP: u32 = 1718909296;
const ATOM_TYPE_MOOV: u32 = 1836019574;
const ATOM_TYPE_MOOF: u32 = 1836019558;
const ATOM_TYPE_MDAT: u32 = 1835295092;

#[derive(Debug)]
struct Mp4Atom {
    pub atom_type: u32,
    // Includes atom size and type.
    pub atom_bytes: Vec<u8>,
}

impl Mp4Atom {
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.atom_bytes.len()
    }
}

#[derive(Debug)]
struct Mp4Parser {
    buf: Vec<u8>,
}

impl Mp4Parser {
    pub fn new() -> Mp4Parser {
        Mp4Parser { buf: Vec::new() }
    }

    pub fn add(&mut self, buf: &[u8]) {
        self.buf.extend_from_slice(buf);
    }

    pub fn pop_atom(&mut self) -> Option<Mp4Atom> {
        if self.buf.len() >= 8 {
            let atom_size = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
            let atom_type = u32::from_be_bytes(self.buf[4..8].try_into().unwrap());
            if self.buf.len() >= atom_size {
                let atom_bytes = self.buf.drain(0..atom_size).collect();
                Some(Mp4Atom {
                    atom_type,
                    atom_bytes,
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

struct State {
    // Atoms in init sequence that must be repeated at each key frame.
    ftyp_atom: Option<Mp4Atom>,
    mp4_parser: Mp4Parser,

    // bitrate: u64,
    // width: u64,
    // height: u64,

    // We hold on to publisher so we don't close then while media is still being published.
    broadcast: broadcast::Publisher,
    catalog: Option<track::Publisher>,
    init: Option<track::Publisher>,

    // Tracks based on their track ID.
    tracks: Option<HashMap<u32, Track>>,
    current: Option<u32>,
}

struct GSTState {
    video_streams: Vec<VideoStream>,
    audio_streams: Vec<AudioStream>,
    // broadcast: broadcast::Publisher,
}
pub struct GST {}

impl GST {
    pub async fn run(broadcast: broadcast::Publisher) -> anyhow::Result<()> {
        gst::init()?;

        gstfmp4::plugin_register_static()?;

        let pipeline = gst::Pipeline::default();

        let state = Arc::new(Mutex::new(GSTState {
            video_streams: vec![VideoStream {
                // name: "video_0".to_string(),
                bitrate: 2_048_000,
                width: 1280,
                height: 720,
            }],
            audio_streams: vec![
                AudioStream {
                    // name: "audio_0".to_string(),
                    // lang: "eng".to_string(),
                    // default: true,
                    wave: "sine".to_string(),
                },
                // AudioStream {
                //     name: "audio_1".to_string(),
                //     lang: "fre".to_string(),
                //     default: false,
                //     wave: "white-noise".to_string(),
                // },
            ],
            // broadcast: broadcast.clone(),
        }));

        {
            let state_lock = state.lock().unwrap();

            for stream in &state_lock.video_streams {
                stream.setup(broadcast.clone(), &pipeline)?;
            }

            for stream in &state_lock.audio_streams {
                stream.setup(broadcast.clone(), &pipeline)?;
            }
        }

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
}

struct VideoStream {
    // name: String,
    bitrate: u64,
    width: u64,
    height: u64,
}

impl VideoStream {
    fn setup(
        &self,
        broadcast: broadcast::Publisher,
        pipeline: &gst::Pipeline,
    ) -> Result<(), Error> {
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 60000000)
            .build()?;

        let raw_capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst_video::VideoCapsBuilder::new()
                    .format(gst_video::VideoFormat::I420)
                    .width(self.width as i32)
                    .height(self.height as i32)
                    .framerate(30.into())
                    .build(),
            )
            .build()?;

        let timeoverlay = gst::ElementFactory::make("timeoverlay").build()?;

        let enc = gst::ElementFactory::make("x264enc")
            .property("bframes", 0u32)
            .property("bitrate", self.bitrate as u32 / 1000u32)
            .build()?;

        let h264_capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-h264")
                    .field("profile", "main")
                    .build(),
            )
            .build()?;

        let mux = gst::ElementFactory::make("cmafmux")
            .property("fragment-duration", 1.mseconds())
            .property_from_str("header-update-mode", "update")
            .property("write-mehd", true)
            .build()?;

        let appsink = gst_app::AppSink::builder().buffer_list(true).build();

        pipeline.add_many([
            &src,
            &raw_capsfilter,
            &timeoverlay,
            &enc,
            &h264_capsfilter,
            &mux,
            appsink.upcast_ref(),
        ])?;

        gst::Element::link_many([
            &src,
            &raw_capsfilter,
            &timeoverlay,
            &enc,
            &h264_capsfilter,
            &mux,
            appsink.upcast_ref(),
        ])?;

        setup_appsink(&appsink, broadcast, true);

        Ok(())
    }
}

struct AudioStream {
    // name: String,
    // lang: String,
    // default: bool,
    wave: String,
}

impl AudioStream {
    fn setup(
        &self,
        broadcast: broadcast::Publisher,
        pipeline: &gst::Pipeline,
    ) -> Result<(), Error> {
        let src = gst::ElementFactory::make("audiotestsrc")
            .property("num-buffers", 60000000)
            .property("samplesperbuffer", 4410)
            .property_from_str("wave", &self.wave)
            .build()?;

        let raw_capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", AudioCapsBuilder::new().rate(44100).build())
            .build()?;

        let enc = gst::ElementFactory::make("avenc_aac").build()?;
        let mux = gst::ElementFactory::make("cmafmux")
            .property("fragment-duration", 1.mseconds())
            .property_from_str("header-update-mode", "update")
            .property("write-mehd", true)
            .build()?;
        let appsink = gst_app::AppSink::builder().buffer_list(true).build();

        pipeline.add_many([&src, &raw_capsfilter, &enc, &mux, appsink.upcast_ref()])?;

        gst::Element::link_many([&src, &raw_capsfilter, &enc, &mux, appsink.upcast_ref()])?;

        setup_appsink(&appsink, broadcast, false);

        Ok(())
    }
}

fn setup_appsink(appsink: &gst_app::AppSink, mut broadcast: broadcast::Publisher, is_video: bool) {
    let state = Arc::new(Mutex::new(State {
        ftyp_atom: None,
        broadcast: broadcast.to_owned(),
        catalog: None,
        init: None,
        mp4_parser: Mp4Parser::new(),

        // Tracks based on their track ID.
        tracks: None,
        current: None,
    }));

    // Set the `emit-signals` property to `true` to receive signals
    appsink.set_property("emit-signals", &true);

    //Get the buffers as soon as they are available
    appsink.set_property("max-buffers", &1u32);
    appsink.set_property("sync", &false);

    // Set up a pad probe on the sink pad to intercept queries
    let sink_pad = appsink.static_pad("sink").unwrap();

    //FIX: https://github.com/sdroege/gst-plugin-rs/blob/95c007953c0874bc46152078775d673cf44cc255/mux/mp4/src/mp4mux/imp.rs#L1243
    sink_pad.add_probe(gst::PadProbeType::QUERY_DOWNSTREAM, move |_pad, info| {
        let Some(query) = info.query_mut() else {
            unreachable!();
        };
        match query.view_mut() {
            gst::QueryViewMut::Seeking(q) => {
                let format = q.format();
                //https://github.com/Kurento/gstreamer/blob/f2553fb153edeeecc2f4f74fca996c74dc8210df/plugins/elements/gstfilesink.c#L494
                match format {
                    gst::Format::Bytes | gst::Format::Default => {
                        q.set(
                            true,
                            0.bytes(),
                            gst::GenericFormattedValue::none_for_format(format), // No known end position, or you can specify it if known
                        );
                        gst::PadProbeReturn::Handled
                    }
                    _ => {
                        // For other formats, do not handle the seeking query
                        q.set(
                            false,
                            gst::GenericFormattedValue::none_for_format(format),
                            gst::GenericFormattedValue::none_for_format(format),
                        );
                        gst::PadProbeReturn::Handled
                    }
                }
            }
            _ => gst::PadProbeReturn::Ok,
        }
    });

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                /*FIXME: this error is happening on the last last element... so we need a way to gracefully end the stream. Find out how the muxer works
                no buffer list
                note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
                Got error from /GstPipeline:pipeline0/GstAppSink:sink: Panicked: no buffer list ()
                                     */
                let mut state = state.lock().unwrap();

                let sample = sink
                    .pull_sample()
                    .with_context(|| "Error pulling sample")
                    .map_err(|_e| gst::FlowError::Eos)?;

                // The muxer only outputs non-empty buffer lists
                let buffer_list = sample.buffer_list_owned().expect("no buffer list");

                assert!(!buffer_list.is_empty());

                let mut data = Vec::new();
                for buffer in buffer_list.iter() {
                    let map = buffer.map_readable().map_err(|e| {
                        eprintln!("{:?}", e);
                        gst::FlowError::Error
                    })?;

                    data.extend_from_slice(map.as_slice());
                }

                state.mp4_parser.add(&data);

                // The current track name
                let mut current = state.current.clone();

                loop {
                    match state.mp4_parser.pop_atom() {
                        Some(atom) => {
                            // println!("atom_size={}, atom_type={}", atom.len(), atom.atom_type);

                            // We're going to parse the moov box.
                            // We have to read the moov box header to correctly advance the cursor for the mp4 crate.
                            let mut reader = Cursor::new(&atom.atom_bytes);
                            let header = mp4::BoxHeader::read(&mut reader)
                                .expect("could not read box header");

                            match atom.atom_type {
                                ATOM_TYPE_FTYP => {
                                    // save for later
                                    // println!("ftyp_atom");
                                    state.ftyp_atom = Some(atom);
                                }
                                ATOM_TYPE_MOOV => {
                                    // println!("moov_atom");
                                    let ftyp = state.ftyp_atom.as_ref().unwrap();
                                    let mut init = ftyp.atom_bytes.clone();
                                    init.extend(&atom.atom_bytes);

                                    // Parse the moov box so we can detect the timescales for each track.
                                    let moov = mp4::MoovBox::read_box(&mut reader, header.size)
                                        .expect("could not read the moov box");

                                    let type_name = if is_video { "video"} else {  "audio"};


                                    // Create the catalog track with a single segment.
                                    if !state.init.is_some() {
                                        let init_name = format!("{}.mp4", type_name);

                                        let mut init_track = broadcast
                                            .create_track(&init_name)
                                            .expect("could not create the init broadcast track");

                                        let init_segment = init_track
                                            .create_segment(segment::Info {
                                                sequence: VarInt::ZERO,
                                                priority: 0,
                                                expires: None,
                                            })
                                            .expect("could not create init segment");

                                        // Create a single fragment, optionally setting the size
                                        let mut init_fragment = init_segment
                                            .final_fragment(VarInt::ZERO)
                                            .expect("could not create the init fragment");

                                        init_fragment.chunk(init.into()).expect("could not insert the moov+ftyp box into the init fragment");

                                        let mut tracks = HashMap::new();

                                        for trak in &moov.traks {
                                            let id = trak.tkhd.track_id;

                                            let uuid = Uuid::new_v4();

                                            let name = format!("{}.m4s", uuid);
                                            // let name = format!("{}.m4s", id);

                                            let timescale = track_timescale(&moov, id);

                                            // Store the track publisher in a map so we can update it later.
                                            let track = broadcast
                                                .create_track(&name)
                                                .expect("could not create a broadcast track");

                                            let track = Track::new(track, timescale);

                                            tracks.insert(id, track);
                                        }

                                        let catalog_name = format!(".catalog_{}", type_name);

                                        let mut catalog = broadcast
                                            .create_track(&catalog_name)
                                            .expect("could not create a catalog");

                                        // Create the catalog track
                                        serve_catalog(&mut catalog, &init_track.name, &moov)
                                            .expect("could not serve the catalog");

                                        state.broadcast = broadcast.clone();
                                        state.catalog = Some(catalog);
                                        state.init = Some(init_track);
                                        state.tracks = Some(tracks);
                                    }
                                }
                                ATOM_TYPE_MOOF => {
                                    // state.moof_atom = Some(atom);
                                    // println!("moof_atom");

                                    let moof = mp4::MoofBox::read_box(&mut reader, header.size)
                                        .expect("failed to read MP4");

                                    // Process the moof.
                                    let fragment = Fragment::new(moof)
                                        .expect("failed to create a new fragment for moof atom");

                                    // Get the track for this moof.
                                    let track = state
                                        .tracks
                                        .as_mut()
                                        .unwrap()
                                        .get_mut(&fragment.track)
                                        .expect("failed to find track");

                                    // Save the track ID for the next iteration, which must be a mdat.
                                    assert!(current.is_none());
                                    current.replace(fragment.track);

                                    // Publish the moof header, creating a new segment if it's a keyframe.
                                    track
                                        .header(atom.atom_bytes, fragment)
                                        .expect("failed to publish moof");
                                }
                                ATOM_TYPE_MDAT => {
                                    // println!("mdat_atom");
                                    // Get the track ID from the previous moof.
                                    let track = current.take().expect("missing moof");
                                    let track = state
                                        .tracks
                                        .as_mut()
                                        .unwrap()
                                        .get_mut(&track)
                                        .expect("failed to find track");

                                    // Publish the mdat atom.
                                    track.data(atom.atom_bytes).expect("failed to publish mdat");
                                }
                                _ => {
                                    //Skip unkown atoms
                                }
                            }
                        }
                        None => break,
                    }
                }

                Ok(gst::FlowSuccess::Ok)
            })
            .eos(move |_sink| {
                println!("End-of-stream reached.");

                unreachable!()
            })
            .build(),
    );
}

struct Track {
    // The track we're producing
    track: track::Publisher,

    // The current segment
    current: Option<fragment::Publisher>,

    // The number of units per second.
    timescale: u64,

    // The number of segments produced.
    sequence: u64,
}

impl Track {
    fn new(track: track::Publisher, timescale: u64) -> Self {
        Self {
            track,
            sequence: 0,
            current: None,
            timescale,
        }
    }

    pub fn header(&mut self, raw: Vec<u8>, fragment: Fragment) -> anyhow::Result<()> {
        if let Some(current) = self.current.as_mut() {
            if !fragment.keyframe {
                // Use the existing segment
                current.chunk(raw.into())?;
                return Ok(());
            }
        }

        // Otherwise make a new segment

        // Compute the timestamp in milliseconds.
        // Overflows after 583 million years, so we're fine.
        let timestamp: u32 = fragment
            .timestamp(self.timescale)
            .as_millis()
            .try_into()
            .context("timestamp too large")?;

        // Create a new segment.
        let segment = self.track.create_segment(segment::Info {
            sequence: VarInt::try_from(self.sequence).context("sequence too large")?,

            // Newer segments are higher priority
            priority: u32::MAX
                .checked_sub(timestamp)
                .context("priority too large")?,

            // Delete segments after 10s.
            expires: Some(time::Duration::from_secs(10)),
        })?;

        // Create a single fragment for the segment that we will keep appending.
        let mut fragment = segment.final_fragment(VarInt::ZERO)?;

        self.sequence += 1;

        // Insert the raw atom into the segment.
        fragment.chunk(raw.into())?;

        // Save for the next iteration
        self.current = Some(fragment);

        Ok(())
    }

    pub fn data(&mut self, raw: Vec<u8>) -> anyhow::Result<()> {
        let fragment = self.current.as_mut().context("missing current fragment")?;
        fragment.chunk(raw.into())?;

        Ok(())
    }
}

struct Fragment {
    // The track for this fragment.
    track: u32,

    // The timestamp of the first sample in this fragment, in timescale units.
    timestamp: u64,

    // True if this fragment is a keyframe.
    keyframe: bool,
}

impl Fragment {
    fn new(moof: mp4::MoofBox) -> anyhow::Result<Self> {
        // We can't split the mdat atom, so this is impossible to support
        anyhow::ensure!(moof.trafs.len() == 1, "multiple tracks per moof atom");
        let track = moof.trafs[0].tfhd.track_id;

        // Parse the moof to get some timing information to sleep.
        let timestamp = sample_timestamp(&moof).expect("couldn't find timestamp");

        // Detect if we should start a new segment.
        let keyframe = sample_keyframe(&moof);

        Ok(Self {
            track,
            timestamp,
            keyframe,
        })
    }

    // Convert from timescale units to a duration.
    fn timestamp(&self, timescale: u64) -> time::Duration {
        time::Duration::from_millis(1000 * self.timestamp / timescale)
    }
}

fn sample_timestamp(moof: &mp4::MoofBox) -> Option<u64> {
    Some(moof.trafs.first()?.tfdt.as_ref()?.base_media_decode_time)
}

fn sample_keyframe(moof: &mp4::MoofBox) -> bool {
    for traf in &moof.trafs {
        // TODO trak default flags if this is None
        let default_flags = traf.tfhd.default_sample_flags.unwrap_or_default();
        let trun = match &traf.trun {
            Some(t) => t,
            None => return false,
        };

        for i in 0..trun.sample_count {
            let mut flags = match trun.sample_flags.get(i as usize) {
                Some(f) => *f,
                None => default_flags,
            };

            if i == 0 && trun.first_sample_flags.is_some() {
                flags = trun.first_sample_flags.unwrap();
            }

            // https://chromium.googlesource.com/chromium/src/media/+/master/formats/mp4/track_run_iterator.cc#177
            let keyframe = (flags >> 24) & 0x3 == 0x2; // kSampleDependsOnNoOther
            let non_sync = (flags >> 16) & 0x1 == 0x1; // kSampleIsNonSyncSample

            if keyframe && !non_sync {
                return true;
            }
        }
    }

    false
}

// Find the timescale for the given track.
fn track_timescale(moov: &mp4::MoovBox, track_id: u32) -> u64 {
    let trak = moov
        .traks
        .iter()
        .find(|trak| trak.tkhd.track_id == track_id)
        .expect("failed to find trak");

    trak.mdia.mdhd.timescale as u64
}

//Copied from https://github.com/kixelated/moq-rs/blob/f64c2e894e4f09873aeef4620dbf20aaabe5d12e/moq-pub/src/media.rs#L127
fn serve_catalog(
    track: &mut track::Publisher,
    init_track_name: &str,
    moov: &mp4::MoovBox,
) -> Result<(), anyhow::Error> {
    // println!("serving the catalog");

    let segment = track.create_segment(segment::Info {
        sequence: VarInt::ZERO,
        priority: 0,
        expires: None,
    })?;

    let mut tracks = Vec::new();

    for trak in &moov.traks {
        let mut track = json!({
            "container": "mp4",
            "init_track": init_track_name,
            "data_track": format!("{}.m4s", trak.tkhd.track_id),
        });

        let stsd = &trak.mdia.minf.stbl.stsd;
        if let Some(avc1) = &stsd.avc1 {
            // avc1[.PPCCLL]
            //
            // let profile = 0x64;
            // let constraints = 0x00;
            // let level = 0x1f;
            let profile = avc1.avcc.avc_profile_indication;
            let constraints = avc1.avcc.profile_compatibility; // Not 100% certain here, but it's 0x00 on my current test video
            let level = avc1.avcc.avc_level_indication;

            let width = avc1.width;
            let height = avc1.height;

            let codec = rfc6381_codec::Codec::avc1(profile, constraints, level);
            let codec_str = codec.to_string();

            track["kind"] = json!("video");
            track["codec"] = json!(codec_str);
            track["width"] = json!(width);
            track["height"] = json!(height);
        } else if let Some(_hev1) = &stsd.hev1 {
            // TODO https://github.com/gpac/mp4box.js/blob/325741b592d910297bf609bc7c400fc76101077b/src/box-codecs.js#L106
            anyhow::bail!("HEVC not yet supported")
        } else if let Some(mp4a) = &stsd.mp4a {
            let desc = &mp4a
                .esds
                .as_ref()
                .context("missing esds box for MP4a")?
                .es_desc
                .dec_config;

            let codec_str = format!(
                "mp4a.{:02x}.{}",
                desc.object_type_indication, desc.dec_specific.profile
            );

            track["kind"] = json!("audio");
            track["codec"] = json!(codec_str);
            track["channel_count"] = json!(mp4a.channelcount);
            track["sample_rate"] = json!(mp4a.samplerate.value());
            track["sample_size"] = json!(mp4a.samplesize);

            let bitrate = max(desc.max_bitrate, desc.avg_bitrate);
            if bitrate > 0 {
                track["bit_rate"] = json!(bitrate);
            }
        } else if let Some(vp09) = &stsd.vp09 {
            // https://github.com/gpac/mp4box.js/blob/325741b592d910297bf609bc7c400fc76101077b/src/box-codecs.js#L238
            let vpcc = &vp09.vpcc;
            let codec_str = format!(
                "vp09.0.{:02x}.{:02x}.{:02x}",
                vpcc.profile, vpcc.level, vpcc.bit_depth
            );

            track["kind"] = json!("video");
            track["codec"] = json!(codec_str);
            track["width"] = json!(vp09.width); // no idea if this needs to be multiplied
            track["height"] = json!(vp09.height); // no idea if this needs to be multiplied

            // TODO Test if this actually works; I'm just guessing based on mp4box.js
            anyhow::bail!("VP9 not yet supported")
        } else {
            // TODO add av01 support: https://github.com/gpac/mp4box.js/blob/325741b592d910297bf609bc7c400fc76101077b/src/box-codecs.js#L251
            anyhow::bail!("unknown codec for track: {}", trak.tkhd.track_id);
        }

        tracks.push(track);
    }

    let catalog = json!({
        "tracks": tracks
    });

    let catalog_str = serde_json::to_string_pretty(&catalog)?;
    log::info!("catalog: {}", catalog_str);

    // Create a single fragment for the segment.
    let mut fragment = segment.final_fragment(VarInt::ZERO)?;

    // Add the segment and add the fragment.
    fragment.chunk(catalog_str.into())?;

    Ok(())
}
