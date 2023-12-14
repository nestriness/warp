use crate::relayurl::*;
use glib::subclass::prelude::*;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst::ClockTime;
#[allow(unused_imports)]
use gst::{gst_debug, gst_error, gst_info, gst_log, gst_trace, gst_warning};
use moq_transport::cache::{broadcast, fragment, segment, track};
use moq_transport::VarInt;
use once_cell::sync::Lazy;
use std::convert::TryInto;
use std::sync::Mutex;
use url::Url;

pub const ELEMENT_NAME: &str = "MoqSink";
const ELEMENT_CLASS_NAME: &str = "MoqSink";
const ELEMENT_LONG_NAME: &str = "Media Over Quic Sink";
const ELEMENT_DESCRIPTION: &str =
    "This element accepts fragmented MP4 input from mp4mux and publishes them to a Moq-Relay.";
const ELEMENT_AUTHOR: &str = "Wanjohi Ryan <wanjohiryan33@gmail.com>";
const DEBUG_CATEGORY: &str = ELEMENT_NAME;

const ATOM_TYPE_FTYPE: u32 = 1718909296;
const ATOM_TYPE_MOOV: u32 = 1836019574;
const ATOM_TYPE_MOOF: u32 = 1836019558;
const ATOM_TYPE_MDAT: u32 = 1835295092;
const DEFAULT_PORT: u16 = 4443;

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

struct Settings {
    host: Option<String>,
    port: Option<u16>,
    name: Option<String>,
}

impl Settings {
    fn to_uri(&self) -> String {
        RelayUrl {
            host: self.host.clone(),
            port: self.port.clone().unwrap(),
            name: self.name.clone().unwrap(),
        }
        .to_string()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            host: None,
            port: None,
            name: None,
        }
    }
}

#[derive(Debug)]
struct StartedState {
    // // We hold on to publisher so we don't close then while media is still being published.
    // broadcast: broadcast::Publisher,
    // //parses the mp4 boxes
    // mp4_parser: Mp4Parser,
    // // Atoms in init sequence that must be repeated at each key frame.
    // ftype_atom: Option<Mp4Atom>,
    // moov_atom: Option<Mp4Atom>,
    // // These atoms that must be buffered and pushed as a single buffer.
    // moof_atom: Option<Mp4Atom>,
    // // Below members that track current fragment (moof, mdat).
    // /// Minimum PTS in fragment.
    // fragment_pts: ClockTime,
    // /// Minimum DTS in fragment.
    // fragment_dts: ClockTime,
    // /// Maximum PTS + duration in fragment.
    // fragment_max_pts_plus_duration: ClockTime,
    // /// Minimum offset in fragment.
    // fragment_offset: Option<u64>,
    // /// Maximum offset_end in fragment.
    // fragment_offset_end: Option<u64>,
    // fragment_buffer_flags: gst::BufferFlags,
    // We hold on to publisher so we don't close then while media is still being published.
    broadcast: broadcast::Publisher,
}

impl StartedState {

}

#[derive(Default)]
enum State {
    #[default]
    Stopped,
    Completed,
    Started(StartedState)
}

// impl Default for State {
//     fn default() -> State {
//         State::Started {
//             state: StartedState {
//                 mp4_parser: Mp4Parser::new(),
//                 broadcast: None,
//                 ftype_atom: None,
//                 moov_atom: None,
//                 moof_atom: None,
//                 fragment_pts: ClockTime::none(),
//                 fragment_dts: ClockTime::none(),
//                 fragment_max_pts_plus_duration: ClockTime::none(),
//                 fragment_offset: None,
//                 fragment_offset_end: None,
//                 fragment_buffer_flags: gst::BufferFlags::DELTA_UNIT,
//             },
//         }
//     }
// }

pub struct MoqSink {
    state: Mutex<State>,
    url: Mutex<Option<Url>>,
    settings: Mutex<Settings>,
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        DEBUG_CATEGORY,
        gst::DebugColorFlags::empty(),
        Some(ELEMENT_LONG_NAME),
    )
});

// impl MoqSink {
// fn sink_chain(
//     &self,
//     pad: &gst::Pad,
//     element: &super::MoqSink,
//     buffer: gst::Buffer,
// ) -> Result<gst::FlowSuccess, gst::FlowError> {
//     gst_log!(CAT, obj: pad, "Handling buffer {:?}", buffer);

//     let mut state = self.state.lock().unwrap();

//     let state = match *state {
//         State::Started { ref mut state, .. } => state,
//     };

//     let map = buffer.map_readable().map_err(|_| {
//         gst::element_error!(element, gst::CoreError::Failed, ["Failed to map buffer"]);
//         gst::FlowError::Error
//     })?;
//     let input_buf = map.as_ref();

//     state.mp4_parser.add(input_buf);

//     // Update cummulative fragment variables.
//     // Buffer PTS, etc. are only valid if this buffer contains MDAT data.
//     if state.mp4_parser.have_mdat() {
//         assert!(buffer.pts().is_some());
//         if state.fragment_pts.is_none() || state.fragment_pts > buffer.pts() {
//             state.fragment_pts = buffer.pts();
//         }
//         if state.fragment_dts.is_none() || state.fragment_dts > buffer.dts() {
//             state.fragment_dts = buffer.dts();
//         }
//         let pts_plus_duration = buffer.pts() + buffer.duration();
//         if state.fragment_max_pts_plus_duration.is_none()
//             || state.fragment_max_pts_plus_duration < pts_plus_duration
//         {
//             state.fragment_max_pts_plus_duration = pts_plus_duration;
//         }
//         if buffer.offset() != gst::BUFFER_OFFSET_NONE
//             && (state.fragment_offset.is_none()
//                 || state.fragment_offset.unwrap() > buffer.offset())
//         {
//             state.fragment_offset = Some(buffer.offset());
//         }
//         if buffer.offset_end() != gst::BUFFER_OFFSET_NONE
//             && (state.fragment_offset_end.is_none()
//                 || state.fragment_offset_end.unwrap() < buffer.offset_end())
//         {
//             state.fragment_offset_end = Some(buffer.offset_end());
//         }
//         if state
//             .fragment_buffer_flags
//             .contains(gst::BufferFlags::DELTA_UNIT)
//             && !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT)
//         {
//             state
//                 .fragment_buffer_flags
//                 .remove(gst::BufferFlags::DELTA_UNIT);
//         }
//         if buffer.flags().contains(gst::BufferFlags::DISCONT) {
//             state
//                 .fragment_buffer_flags
//                 .insert(gst::BufferFlags::DISCONT);
//         }
//         gst_trace!(CAT, obj: pad, "Updated state={:?}", state);
//     }

//     loop {
//         match state.mp4_parser.pop_atom() {
//             Some(atom) => {
//                 gst_log!(CAT, obj: pad, "atom_size={}, atom_type={}", atom.len(), atom.atom_type);
//                 match atom.atom_type {
//                     ATOM_TYPE_FTYPE => {
//                         state.ftype_atom = Some(atom);
//                         gst_log!(CAT, obj: pad, "ftype_atom={:?}", state.ftype_atom);
//                     }
//                     ATOM_TYPE_MOOV => {
//                         state.moov_atom = Some(atom);
//                         gst_log!(CAT, obj: pad, "moov_atom={:?}", state.moov_atom);
//                     }
//                     ATOM_TYPE_MOOF => {
//                         state.moof_atom = Some(atom);
//                         gst_log!(CAT, obj: pad, "moof_atom={:?}", state.moof_atom);
//                     }
//                     ATOM_TYPE_MDAT => {
//                         let mdat_atom = atom;
//                         match (
//                             state.ftype_atom.as_ref(),
//                             state.moov_atom.as_ref(),
//                             state.moof_atom.as_ref(),
//                         ) {
//                             (Some(ftype_atom), Some(moov_atom), Some(moof_atom)) => {
//                                 let include_header = !state
//                                     .fragment_buffer_flags
//                                     .contains(gst::BufferFlags::DELTA_UNIT);
//                                 let header_len = if include_header {
//                                     ftype_atom.len() + moov_atom.len()
//                                 } else {
//                                     0
//                                 };
//                                 let output_buf_len =
//                                     header_len + moof_atom.len() + mdat_atom.len();

//                                 gst_log!(CAT, obj: pad, "Pushing buffer; include_header={}, ftype.len={}, moov.len={}, moof.len={}, mdat.len={}",
//                                     include_header, ftype_atom.len(), moov_atom.len(), moof_atom.len(), mdat_atom.len());

//                                 let mut gst_buffer =
//                                     gst::Buffer::with_size(output_buf_len).unwrap();
//                                 {
//                                     let buffer_ref = gst_buffer.get_mut().unwrap();
//                                     buffer_ref.set_pts(state.fragment_pts);
//                                     buffer_ref.set_dts(state.fragment_dts);
//                                     let duration = state.fragment_max_pts_plus_duration
//                                         - state.fragment_pts;
//                                     buffer_ref.set_duration(duration);
//                                     buffer_ref.set_offset(
//                                         state
//                                             .fragment_offset
//                                             .unwrap_or(gst::BUFFER_OFFSET_NONE),
//                                     );
//                                     buffer_ref.set_offset_end(
//                                         state
//                                             .fragment_offset_end
//                                             .unwrap_or(gst::BUFFER_OFFSET_NONE),
//                                     );
//                                     buffer_ref.set_flags(state.fragment_buffer_flags);
//                                     let mut buffer_map = buffer_ref.map_writable().unwrap();
//                                     let slice = buffer_map.as_mut_slice();
//                                     let mut pos = 0;
//                                     if include_header {
//                                         slice[pos..pos + ftype_atom.len()]
//                                             .copy_from_slice(&ftype_atom.atom_bytes);
//                                         pos += ftype_atom.len();
//                                         slice[pos..pos + moov_atom.len()]
//                                             .copy_from_slice(&moov_atom.atom_bytes);
//                                         pos += moov_atom.len();
//                                     }
//                                     slice[pos..pos + moof_atom.len()]
//                                         .copy_from_slice(&moof_atom.atom_bytes);
//                                     pos += moof_atom.len();
//                                     slice[pos..pos + mdat_atom.len()]
//                                         .copy_from_slice(&mdat_atom.atom_bytes);
//                                     pos += mdat_atom.len();
//                                     assert_eq!(pos, output_buf_len);

//                                     //FIXME: Work on the Json here, instead of redoing it in a new method.
//                                 }
//                                 // Clear fragment variables.
//                                 state.fragment_pts = ClockTime::none();
//                                 state.fragment_dts = ClockTime::none();
//                                 state.fragment_max_pts_plus_duration = ClockTime::none();
//                                 state.fragment_offset = None;
//                                 state.fragment_offset_end = None;
//                                 state.fragment_buffer_flags = gst::BufferFlags::DELTA_UNIT;
//                                 // Push new buffer.
//                                 gst_log!(CAT, obj: pad, "Pushing buffer {:?}", gst_buffer);
//                                 // let _ = self.srcpad.push(gst_buffer)?;
//                                 // self.send_to_network(gst_buffer)?;
//                             }
//                             _ => {
//                                 gst_warning!(CAT, obj: pad, "Received mdat without ftype, moov, or moof");
//                             }
//                         }
//                     }
//                     _ => {
//                         gst_warning!(CAT, obj: pad, "Unknown atom type {:?}", atom);
//                     }
//                 }
//             }
//             None => break,
//         }
//     }
//     gst_trace!(CAT, obj: element, "sink_chain: END: state={:?}", state);
//     Ok(gst::FlowSuccess::Ok)
// }

// fn sink_event(&self, _pad: &gst::Pad, _element: &super::MoqSink, event: gst::Event) -> bool {
//     self.srcpad.push_event(event)
// }

// fn sink_query(
//     &self,
//     _pad: &gst::Pad,
//     _element: &super::MoqSink,
//     query: &mut gst::QueryRef,
// ) -> bool {
//     self.srcpad.peer_query(query)
// }

// fn send_to_network(&self, buffer: gst::Buffer) -> Result<(), Box<dyn std::error::Error>> {
//     //Let this be our publisher
//     Ok(())
// }
// }
impl MoqSink {
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        let settings = self.settings.lock().unwrap();

        if let State::Started { .. } = *state {
            unreachable!("Element already started");
        }

        let relay_url = {
            let url = self.url.lock().unwrap();
            match *url {
                Some(ref url) => url.clone(),
                None => {
                    return Err(gst::error_msg!(
                        gst::ResourceError::Settings,
                        ["Cannot start without a URL being set"]
                    ));
                }
            }
        };

        // Disable tracing so we don't get a bunch of Quinn spam.
        let tracer = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::set_global_default(tracer).unwrap();

        // Create a list of acceptable root certificates.
        let mut roots = rustls::RootCertStore::empty();

        // Add the platform's native root certificates.
        for cert in
            rustls_native_certs::load_native_certs().context("could not load platform certs")?
        {
            roots
                .add(&rustls::Certificate(cert.0))
                .context("failed to add root cert")?;
        }

        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(roots)
            .with_no_client_auth();

        tls_config.alpn_protocols = vec![webtransport_quinn::ALPN.to_vec()]; // this one is important

        let arc_tls_config = std::sync::Arc::new(tls_config);
        let quinn_client_config = quinn::ClientConfig::new(arc_tls_config);

        let mut endpoint = quinn::Endpoint::client("[::]:0")?;
        endpoint.set_default_client_config(quinn_client_config);

        gst::debug!(CAT,imp: self,"connecting to relay: url={:?}",relay_url);

        let session = webtransport_quinn::connect(&endpoint, &relay_url)
            .await
            .context("failed to create WebTransport session")?;

        let (mut publisher, subscriber) = broadcast::new("");

        let session = moq_transport::session::Client::publisher(session, subscriber)
            .await
            .context("failed to create MoQ Transport session")?;

        // let publisher = publisher
        //     .create_track(&config.track)
        //     .context("failed to create clock track")?;

        Ok(())
    }

    fn set_uri(self: &MoqSink, url_str: Option<&str>) -> Result<(), glib::Error> {
        let state = self.state.lock().unwrap();

        if let State::Started { .. } = *state {
            return Err(glib::Error::new(
                gst::URIError::BadState,
                "Cannot set URI on a started MoqSink",
            ));
        }

        let mut url = self.url.lock().unwrap();

        if url_str.is_none() {
            *url = None;
            return Ok(());
        }

        gst::debug!(CAT, imp: self, "Setting uri to {:?}", url_str);

        let url_str = url_str.unwrap();
        match parse_relay_url(url_str) {
            Ok(relayUrl) => {
                *url = Some(relayUrl);
                Ok(())
            }
            Err(_) => Err(glib::Error::new(
                gst::URIError::BadUri,
                "Could not parse URI",
            )),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for MoqSink {
    const NAME: &'static str = ELEMENT_CLASS_NAME;
    type Type = super::MoqSink;
    type ParentType = gst_base::BaseSink;

    type Interfaces = (gst::URIHandler);

    type Instance;

    type Class;
}

impl ObjectImpl for MoqSink {
    fn constructed(&self, obj: &Self::Type) {
        self.parent_constructed();
        self.obj().set_sync(false);
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::builder("host")
                    .nick("Url Host")
                    .blurb("The host of the relay server to connect to, e.g. example.com")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecString::builder("port")
                    .nick("Url Port")
                    .blurb("The port of the relay server to connect to, most probably this is a 4443")
                    .default_value(DEFAULT_PORT)
                    .build(),
                glib::ParamSpecString::builder("name")
                    .nick("Url Name")
                    .blurb("This is a very long random string to identify your stream on the relay server")
                    .mutable_ready()
                    .build(),
            ]
        });
        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        let mut settings = self.settings.lock().unwrap();

        gst::debug!(
            CAT,
            imp: self,
            "Setting property '{}' to '{:?}'",
            pspec.name(),
            value
        );

        match pspec.name() {
            "host" => {
                settings.host = value.get::<String>().expect("type checked upstream");
                if settings.port.is_some() && settings.name.is_some() {
                    let _ = self.set_uri(Some(&settings.to_uri()));
                }
            }
            "port" => {
                settings.port = value.get::<Option<u16>>().expect("type checked upstream");
                if settings.host.is_some() && settings.name.is_some() {
                    let _ = self.set_uri(Some(&settings.to_uri()));
                }
            }
            "name" => {
                settings.name = value
                    .get::<Option<String>>()
                    .expect("type checked upstream");
                if settings.host.is_some() && settings.port.is_some() {
                    let _ = self.set_uri(Some(&settings.to_uri()));
                }
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let settings = self.settings.lock().unwrap();

        match pspec.name() {
            "host" => settings.host.to_value(),
            "name" => settings.name.to_value(),
            "port" => settings.port.to_value(),
            _ => unimplemented!(),
        }
    }
}

impl GstObjectImpl for MoqSink {}

impl ElementImpl for MoqSink {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                ELEMENT_LONG_NAME,
                "Generic",
                ELEMENT_DESCRIPTION,
                ELEMENT_AUTHOR,
            )
        });
        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let caps = gst::Caps::new_any();
            let sink_pad_template = gst::PadTemplate::new(
                "sink",
                gst::PadDirection::Sink,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();
            vec![sink_pad_template]
        });
        PAD_TEMPLATES.as_ref()
    }
}

impl URIHandlerImpl for MoqSink {
    const URI_TYPE: gst::URIType = gst::URIType::Sink;

    fn protocols() -> &'static [&'static str] {
        &["https"]
    }

    fn uri(&self) -> Option<String> {
        self.url.lock().unwrap().as_ref().map(|s| s.to_string())
    }

    fn set_uri(&self, uri: &str) -> Result<(), glib::Error> {
        self.set_uri(Some(uri))
    }
}
