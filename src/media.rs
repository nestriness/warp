use anyhow::{self,Context};
use gst::prelude::*;
use gst::ClockTime;

use std::sync::{Arc, Mutex};
use url::Url;

use moq_transport::cache::{broadcast, fragment, segment, track};
use moq_transport::VarInt;
use std::collections::HashMap;
use std::io::Cursor;
use serde_json::json;
use std::cmp::max;
use std::time;

use mp4::{self, ReadBox};

const ATOM_TYPE_FTYPE: u32 = 1718909296;
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

    // Returns true if all or part of an MDAT body has been added.
    pub fn have_mdat(&self) -> bool {
        if self.buf.len() > 8 {
            let atom_type = u32::from_be_bytes(self.buf[4..8].try_into().unwrap());
            atom_type == ATOM_TYPE_MDAT
        } else {
            false
        }
    }

    pub fn pop_atom(&mut self) -> Option<Mp4Atom> {
        if self.buf.len() >= 8 {
            let atom_size = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
            let atom_type = u32::from_be_bytes(self.buf[4..8].try_into().unwrap());
            if self.buf.len() >= atom_size {
                let mut atom_bytes = Vec::with_capacity(atom_size);
                // TODO: Swap vectors?
                atom_bytes.extend_from_slice(&self.buf[0..atom_size]);
                assert_eq!(self.buf.len(), atom_size);
                self.buf.clear();
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
    ftype_atom: Option<Mp4Atom>,
    moov_atom: Option<Mp4Atom>,
    // Below members that track current fragment (moof, mdat).
    /// Minimum PTS in fragment.
    fragment_pts: Option<ClockTime>,
    /// Minimum DTS in fragment.
    fragment_dts: Option<ClockTime>,
    /// Maximum PTS + duration in fragment.
    fragment_max_pts_plus_duration: Option<ClockTime>,
    /// Minimum offset in fragment.
    fragment_offset: Option<u64>,
    /// Maximum offset_end in fragment.
    fragment_offset_end: Option<u64>,
    fragment_buffer_flags: gst::BufferFlags,

    bitrate: u64,
    width: u64,
    height: u64,
    // wave: String,
    mp4_parser: Mp4Parser,

    // We hold on to publisher so we don't close then while media is still being published.
    broadcast: broadcast::Publisher,
    catalog: Option<track::Publisher>,
	init: Option<track::Publisher>,

	// Tracks based on their track ID.
	tracks: Option<HashMap<u32, Track>>,
    current:  Option<u32>,
}

pub struct GST {}

impl GST {
    pub async fn run(mut broadcast: broadcast::Publisher) -> anyhow::Result<()> {
        gst::init()?;

        //FIXME: Get this value from commandline argument

        gstfmp4::plugin_register_static()?;

        let pipeline = gst::Pipeline::default();

        let state = Arc::new(Mutex::new(State {
            ftype_atom: None,
            moov_atom: None,
            fragment_pts: None,
            fragment_dts: None,
            fragment_max_pts_plus_duration: None,
            fragment_offset: None,
            fragment_offset_end: None,
            fragment_buffer_flags: gst::BufferFlags::DELTA_UNIT,
            bitrate: 2_048_000,
            width: 1280,
            height: 720,
            broadcast: broadcast.to_owned(),
            mp4_parser: Mp4Parser::new(),
            // wave: "sine".to_string(),
            catalog: None,
	        init: None,

	        // Tracks based on their track ID.
	        tracks: None,
            current: None,
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

        //drop the choke hold here
        drop(state_lock);

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    // The muxer only outputs non-empty buffer lists
                    let buffer_list = sample.buffer_list_owned().expect("no buffer list");
                    assert!(!buffer_list.is_empty());

                    let first = buffer_list.get(0).unwrap();

                    // Each list contains a full segment, i.e. does not start with a DELTA_UNIT
                    assert!(!first.flags().contains(gst::BufferFlags::DELTA_UNIT));

                    for buffer in &*buffer_list {
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        let input_buf = map.as_ref();

                        let mut state = state.lock().unwrap();

                        state.mp4_parser.add(input_buf);

                        // Update cummulative fragment variables.
                        // Buffer PTS, etc. are only valid if this buffer contains MDAT data.
                        if state.mp4_parser.have_mdat() {
                            assert!(buffer.pts().is_some());
                            if state.fragment_pts.is_none() || state.fragment_pts > buffer.pts() {
                                state.fragment_pts = buffer.pts();
                            }
                            if state.fragment_dts.is_none() || state.fragment_dts > buffer.dts() {
                                state.fragment_dts = buffer.dts();
                            }
                            let pts = buffer.pts();
                            let duration = buffer.duration();

                            let pts_plus_duration = match (pts, duration) {
                                (Some(pts), Some(duration)) => Some(pts + duration),
                                // Handle the case where one or both values are `None`
                                _ => None,
                            };
                            if state.fragment_max_pts_plus_duration.is_none()
                                || state.fragment_max_pts_plus_duration < pts_plus_duration
                            {
                                state.fragment_max_pts_plus_duration = pts_plus_duration;
                            }
                            if buffer.offset() != gst_sys::GST_BUFFER_OFFSET_NONE
                                && (state.fragment_offset.is_none()
                                    || state.fragment_offset.unwrap() > buffer.offset())
                            {
                                state.fragment_offset = Some(buffer.offset());
                            }
                            if buffer.offset_end() != gst_sys::GST_BUFFER_OFFSET_NONE
                                && (state.fragment_offset_end.is_none()
                                    || state.fragment_offset_end.unwrap() < buffer.offset_end())
                            {
                                state.fragment_offset_end = Some(buffer.offset_end());
                            }
                            if state
                                .fragment_buffer_flags
                                .contains(gst::BufferFlags::DELTA_UNIT)
                                && !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT)
                            {
                                state
                                    .fragment_buffer_flags
                                    .remove(gst::BufferFlags::DELTA_UNIT);
                            }
                            if buffer.flags().contains(gst::BufferFlags::DISCONT) {
                                state
                                    .fragment_buffer_flags
                                    .insert(gst::BufferFlags::DISCONT);
                            }
                            log::info!("finished updating fragment time stamps");
                        }
                    }

                    loop {
                        let mut state = state.lock().unwrap();
                        match state.mp4_parser.pop_atom() {
                            Some(atom) => {
                                log::info!("atom_size={}, atom_type={}", atom.len(), atom.atom_type);
                                match atom.atom_type {
                                    ATOM_TYPE_FTYPE => {
                                        state.ftype_atom = Some(atom);
                                        log::info!("ftype_atom={:?}", state.ftype_atom);
                                    },
                                    ATOM_TYPE_MOOV => {
                                        state.moov_atom = Some(atom);

                                        log::info!("moov_atom={:?}", state.moov_atom);

                                        match (state.ftype_atom.as_ref(), state.moov_atom.as_ref()) {
                                            (Some(ftype_atom), Some(moov_atom)) => {
                                                let output_buf_len = ftype_atom.len() + moov_atom.len();
                                                let mut gst_buffer = gst::Buffer::with_size(output_buf_len).unwrap();
                                                {
                                                    let buffer_ref = gst_buffer.get_mut().unwrap();
                                                    buffer_ref.set_pts(state.fragment_pts);
                                                    buffer_ref.set_dts(state.fragment_dts);

                                                    let pts_plus_duration =state.fragment_max_pts_plus_duration.clone();
                                                    let fragment_pts = state.fragment_pts.clone();

                                                    let duration = match (pts_plus_duration, fragment_pts) {
                                                        (Some(pts_plus_duration), Some(fragment_pts)) => {
                                                            Some(pts_plus_duration - fragment_pts)
                                                        }
                                                        // Handle the case where one or both values are `None`
                                                        _ => None,
                                                    };                                                    
                                                    buffer_ref.set_duration(duration);
                                                    buffer_ref.set_offset(state.fragment_offset.unwrap_or(gst_sys::GST_BUFFER_OFFSET_NONE));
                                                    buffer_ref.set_offset_end(state.fragment_offset_end.unwrap_or(gst_sys::GST_BUFFER_OFFSET_NONE));
                                                    //this is a header
                                                    buffer_ref.set_flags(gst::BufferFlags::HEADER);
                                                    let mut buffer_map = buffer_ref.map_writable().unwrap();
                                                    let slice = buffer_map.as_mut_slice();
                                                    let mut pos = 0;
                                                    slice[pos..pos+ftype_atom.len()].copy_from_slice(&ftype_atom.atom_bytes);
                                                    pos += ftype_atom.len();
                                                    slice[pos..pos+moov_atom.len()].copy_from_slice(&moov_atom.atom_bytes);
                                                    pos += moov_atom.len();
                                                    log::info!("real size of the header atom={:?}", pos);
                                                    log::info!("expected size of the header atom={:?}", output_buf_len);
                                                    assert_eq!(pos, output_buf_len);
                                                };
                                                log::info!("pushing the header atom={:?}", gst_buffer);

                                                // Create the catalog track with a single segment.
		                                        let mut init_track = state.broadcast.clone().create_track("0.mp4").map_err(|_| gst::FlowError::Error)?;
		                                        let init_segment = init_track.create_segment(segment::Info {
		                                        	sequence: VarInt::ZERO,
		                                        	priority: 0,
		                                        	expires: None,
		                                        }).map_err(|_| gst::FlowError::Error)?;

                                                // Create a single fragment, optionally setting the size
		                                        let mut init_fragment = init_segment.final_fragment(VarInt::ZERO).map_err(|_| gst::FlowError::Error)?;

                                                let buffer_map = gst_buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                                                // Create a a Vec<u8> object from the data slice
                                                let bytes = buffer_map.as_slice().to_vec();

                                                init_fragment.chunk(bytes.into()).map_err(|_| gst::FlowError::Error)?;

                                                // We're going to parse the moov box.
		                                        // We have to read the moov box header to correctly advance the cursor for the mp4 crate.
		                                        let mut moov_reader = Cursor::new(moov_atom.atom_bytes.clone());
		                                        let moov_header = mp4::BoxHeader::read(&mut moov_reader).map_err(|_| gst::FlowError::Error)?;

                                                // Parse the moov box so we can detect the timescales for each track.
		                                        let moov = mp4::MoovBox::read_box(&mut moov_reader, moov_header.size).map_err(|_| gst::FlowError::Error)?;

                                                let mut tracks = HashMap::new();

                                                for trak in &moov.traks {
                                                    let id = trak.tkhd.track_id;
                                                    let name = format!("{}.m4s", id);
                                        
                                                    let timescale = track_timescale(&moov, id);
                                        
                                                    // Store the track publisher in a map so we can update it later.
                                                    let track = broadcast.create_track(&name).map_err(|_| gst::FlowError::Error)?;
                                                    let track = Track::new(track, timescale);
                                                    tracks.insert(id, track);
                                                }

                                                let mut catalog = broadcast.create_track(".catalog").map_err(|_| gst::FlowError::Error)?;

		                                        // Create the catalog track
		                                        Self::serve_catalog(&mut catalog, &init_track.name, &moov).map_err(|_| gst::FlowError::Error)?;

                                               state.tracks = Some(tracks);
                                               state.init = Some(init_track);
                                               state.catalog = Some(catalog);
                                            }
                                            _ => {
                                                log::warn!("Received moov without ftype");
                                            },
                                        }
                                    },
                                    ATOM_TYPE_MOOF => {
                                        log::info!("pushing the moof_atom={:?}", atom);

                                        let mut current = state.current;

                                        let tracks = if let Some(tracks) = &mut state.tracks {
                                            tracks
                                        } else {
                                            log::warn!("Tracks are not set up yet");
                                            return Err(gst::FlowError::Error);
                                        };

                                        let mut reader = Cursor::new(atom.atom_bytes.clone());
                                        let header = mp4::BoxHeader::read(&mut reader).map_err(|_| gst::FlowError::Error)?;
                                        let moof = mp4::MoofBox::read_box(&mut reader, header.size).map_err(|_| gst::FlowError::Error)?;

                                        // Process the moof.
                                        let fragment = Fragment::new(moof).map_err(|_| gst::FlowError::Error)?;
                                                        
                                        // Get the track for this moof.
                                        let track = tracks.get_mut(&fragment.track).context("failed to find track").map_err(|_| gst::FlowError::Error)?;
                                                        
                                        // Save the track ID for the next iteration, which must be a mdat.
                                        if current.clone().is_none(){
                                            log::info!("multiple moof atoms")
                                        }

                                        current.replace(fragment.track);

                                        // Publish the moof header, creating a new segment if it's a keyframe.
					                    track.header(atom.atom_bytes.clone(), fragment).context("failed to publish moof").map_err(|_| gst::FlowError::Error)?;
                                    
                                    },
                                    ATOM_TYPE_MDAT => {
                                        log::info!("pushing the mdat_atom={:?}", atom);

                                        // Get the track ID from the previous moof.
					                    let track = state.current.take().context("missing moof").map_err(|_| gst::FlowError::Error)?;
                                        
                                        let tracks = if let Some(tracks) = &mut state.tracks {
                                            tracks
                                        } else {
                                            log::warn!("Tracks are not set up yet");
                                            return Err(gst::FlowError::Error);
                                        };

					                    let track = tracks.get_mut(&track).context("failed to find track").map_err(|_| gst::FlowError::Error)?;

					                    // Publish the mdat atom.
					                    track.data(atom.atom_bytes.clone()).context("failed to publish mdat").map_err(|_| gst::FlowError::Error)?;
                                    },
                                    _ => {
                                        log::warn!("Unknown atom type {:?}", atom);
                                    },
                                }
                            },
                            None => break,
                        }
                    };

                    Ok(gst::FlowSuccess::Ok)
                })
                .eos(move |_sink| {
                    unreachable!();
                })
                .build(),
        );

        Ok(())
    }

    //Copied from https://github.com/kixelated/moq-rs/blob/f64c2e894e4f09873aeef4620dbf20aaabe5d12e/moq-pub/src/media.rs#L127
    fn serve_catalog(
		track: &mut track::Publisher,
		init_track_name: &str,
		moov: &mp4::MoovBox,
	) -> Result<(), anyhow::Error> {
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

				let codec_str = format!("mp4a.{:02x}.{}", desc.object_type_indication, desc.dec_specific.profile);

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
				let codec_str = format!("vp09.0.{:02x}.{:02x}.{:02x}", vpcc.profile, vpcc.level, vpcc.bit_depth);

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
			priority: u32::MAX.checked_sub(timestamp).context("priority too large")?,

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