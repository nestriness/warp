use gst::glib;
use gst::glib::once_cell::sync::Lazy;
use gst::prelude::*;
use gst::subclass::prelude::*;

use std::sync::{Arc, Mutex};

use crate::relayurl::*;
use crate::RUNTIME;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::mpsc;

use moq_transport::cache::{broadcast, fragment, segment, track};
use url::Url;

extern crate rand;
use rand::{distributions::Alphanumeric, Rng};

pub const ELEMENT_NAME: &str = "MoqSink";
const ELEMENT_CLASS_NAME: &str = "MoqSink";
const ELEMENT_LONG_NAME: &str = "Media Over Quic Sink";
const ELEMENT_DESCRIPTION: &str =
    "This element accepts fragmented MP4 input from mp4mux and publishes them to a Moq-Relay.";
const ELEMENT_AUTHOR: &str = "Wanjohi Ryan <wanjohiryan33@gmail.com>";
const DEBUG_CATEGORY: &str = ELEMENT_NAME;

const DEFAULT_NAME_LEN: usize = 10; // Length of the random string
const DEFAULT_PORT: i32 = 4443; //default port for relay server
const DEFAULT_ADDRESS: Option<&str> = Some("localhost"); //default host is localhost

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

#[derive(Debug)]
struct Started {
    broadcast: broadcast::Publisher,

    mp4_parser: Mp4Parser,
    // Atoms in init sequence that must be repeated at each key frame.
    ftype_atom: Option<Mp4Atom>,
    moov_atom: Option<Mp4Atom>,
    // These atoms that must be buffered and pushed as a single buffer.
    moof_atom: Option<Mp4Atom>,
    // Below members that track current fragment (moof, mdat).
    /// Minimum PTS in fragment.
    fragment_pts: ClockTime,
    /// Minimum DTS in fragment.
    fragment_dts: ClockTime,
    /// Maximum PTS + duration in fragment.
    fragment_max_pts_plus_duration: ClockTime,
    /// Minimum offset in fragment.
    fragment_offset: Option<u64>,
    /// Maximum offset_end in fragment.
    fragment_offset_end: Option<u64>,
    fragment_buffer_flags: gst::BufferFlags,
}

impl Started {
    pub fn new(broadcast: broadcast::Publisher) -> Started {
        Started {
            broadcast,
            mp4_parser: Mp4Parser::new(),
            ftype_atom: None,
            moov_atom: None,
            moof_atom: None,
            fragment_pts: ClockTime::none(),
            fragment_dts: ClockTime::none(),
            fragment_max_pts_plus_duration: ClockTime::none(),
            fragment_offset: None,
            fragment_offset_end: None,
            fragment_buffer_flags: gst::BufferFlags::DELTA_UNIT,
        }
    }
}

struct Settings {
    host: Option<String>,
    port: Option<i32>,
    name: Option<String>,
}

impl Settings {
    fn to_uri(&self) -> String {
        RelayUrl {
            host: self.host.clone().unwrap(),
            port: self.port.clone().unwrap(),
            name: self.name.clone().unwrap(),
        }
        .to_string()
    }
}

impl Default for Settings {
    fn default() -> Self {
        //generate a random string on
        let random_name: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(DEFAULT_NAME_LEN)
            .map(char::from)
            .collect();

        Settings {
            host: Some("localhost".to_owned()), //localhost
            port: Some(DEFAULT_PORT),
            name: Some(random_name),
        }
    }
}

#[derive(Default)]
enum State {
    #[default]
    Stopped,
    Started(Started),
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        DEBUG_CATEGORY,
        gst::DebugColorFlags::empty(),
        Some(ELEMENT_LONG_NAME),
    )
});

#[derive(Default, object_subclass)]
pub struct MoqSink {
    state: Mutex<State>,
    url: Mutex<Option<Url>>,
    settings: Mutex<Settings>,
}

impl Default for MoqSink {
    fn default() -> Self {
        MoqSink {
            state: Mutex::new(None),
            url: Mutex::new(None),
            settings: Mutex::new(Settings::default()),
        }
    }
}

impl MoqSink {
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().map_err(|e| {
            gst::error_msg!(
                gst::ResourceError::Settings,
                ["Failed to acquire state lock: {}", e]
            )
        })?;

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

        //More complex but with error handling

        // let relay_url = self
        //     .url
        //     .lock()
        //     .map_err(|e| {
        //         gst::error_msg!(
        //             gst::ResourceError::Settings,
        //             ["Failed to acquire URL lock: {}", e]
        //         )
        //     })?
        //     .clone()
        //     .ok_or_else(|| {
        //         gst::error_msg!(
        //             gst::ResourceError::Settings,
        //             ["Cannot start without a URL being set"]
        //         )
        //     })?;

        gst::trace!(
            CAT,
            imp: self,
            "connecting to relay: url={}",
            relay_url
        );

        let (publisher, subscriber) = broadcast::new("");

        // Spawn a new thread to run the Moq server
        RUNTIME.spawn(async move {
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

            // this one is important
            tls_config.alpn_protocols = vec![webtransport_quinn::ALPN.to_vec()];

            let arc_tls_config = std::sync::Arc::new(tls_config);
            let quinn_client_config = quinn::ClientConfig::new(arc_tls_config);

            let mut endpoint =
                quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0))?;
            endpoint.set_default_client_config(quinn_client_config);

            let session = webtransport_quinn::connect(&endpoint, &relay_url)
                .await
                .context("failed to create WebTransport session")?;

            let session = moq_transport::session::Client::publisher(session, subscriber)
                .await
                .context("failed to create MoQ Transport session")?;

            session.run().await.context("session error")?;

            Ok::<(), anyhow::Error>(())
        });

        // Update the state to indicate the element has started
        *state = State::Started(Started::new(broadcast));

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
            Ok(relay_url) => {
                *url = Some(relay_url);
                Ok(())
            }
            Err(err) => Err(glib::Error::new(gst::URIError::BadUri, &err)),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for MoqSink {
    const NAME: &'static str = ELEMENT_CLASS_NAME;
    type Type = super::MoqSink;
    type ParentType = gst_base::BaseSink;

    type Interfaces = (gst::URIHandler,);

    type Instance;

    type Class;
}

impl GstObjectImpl for WaylandDisplaySrc {}

impl ObjectImpl for MoqSink {
    fn constructed(&self) {
        self.parent_constructed();
        self.obj().set_sync(false);
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::builder("host")
                    .nick("Host")
                    .blurb("The host of the relay server to connect to, this can be a web url")
                    .default_value(DEFAULT_ADDRESS)
                    .build(),
                glib::ParamSpecInt::builder("port")
                    .nick("Port")
                    .blurb("The port of the relay server to connect to, most probably this is a 4443")
                    .minimum(0)
                    .maximum(u16::MAX as i32)
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
                settings.host = value
                    .get::<Option<String>>()
                    .expect("type checked upstream");
                if settings.port.is_some() && settings.name.is_some() {
                    let _ = self.set_uri(Some(&settings.to_uri()));
                }
            }
            "port" => {
                let port = value.get::<i32>().expect("type checked upstream");
                settings.port = Some(port);
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
            "port" => {
                // Handle the Option<i32> for the port
                match settings.port {
                    Some(port) => port.to_value(),
                    None => glib::Value::from_type(glib::Type::I32), // Create a "None" Value for i32
                }
            }

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

impl BaseSinkImpl for MoqSink {
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        self.start()
    }
    fn render(
        &self,
        element: &Self::Type,
        buffer: &gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        if let State::Stopped = *self.state.lock().unwrap() {
            gst::element_imp_error!(self, gst::CoreError::Failed, ["Not started yet"]);
            return Err(gst::FlowError::Error);
        }

        gst::trace!(CAT, imp: self, "Rendering {:?}", buffer);

        let mut state = self.state.lock().unwrap();

        let started_state = match *state {
            State::Started(ref mut started_state) => started_state,
            State::Stopped => {
                unreachable!("Element should be started already");
            }
        };

        let map = buffer.map_readable().map_err(|_| {
            gst::element_imp_error!(self, gst::CoreError::Failed, ["Failed to map buffer"]);
            gst::FlowError::Error
        })?;

        let input_buf = map.as_ref();

        started_state.mp4_parser.add(input_buf);

        // Update cummulative fragment variables.
        // Buffer PTS, etc. are only valid if this buffer contains MDAT data.
        if started_state.mp4_parser.have_mdat() {
            assert!(buffer.pts().is_some());
            if started_state.fragment_pts.is_none() || started_state.fragment_pts > buffer.pts() {
                started_state.fragment_pts = buffer.pts();
            }
            if started_state.fragment_dts.is_none() || started_state.fragment_dts > buffer.dts() {
                started_state.fragment_dts = buffer.dts();
            }
            let pts_plus_duration = buffer.pts() + buffer.duration();
            if started_state.fragment_max_pts_plus_duration.is_none()
                || started_state.fragment_max_pts_plus_duration < pts_plus_duration
            {
                started_state.fragment_max_pts_plus_duration = pts_plus_duration;
            }
            if buffer.offset() != gst::BUFFER_OFFSET_NONE
                && (started_state.fragment_offset.is_none()
                    || started_state.fragment_offset.unwrap() > buffer.offset())
            {
                started_state.fragment_offset = Some(buffer.offset());
            }
            if buffer.offset_end() != gst::BUFFER_OFFSET_NONE
                && (started_state.fragment_offset_end.is_none()
                    || started_state.fragment_offset_end.unwrap() < buffer.offset_end())
            {
                started_state.fragment_offset_end = Some(buffer.offset_end());
            }
            if started_state
                .fragment_buffer_flags
                .contains(gst::BufferFlags::DELTA_UNIT)
                && !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT)
            {
                started_state
                    .fragment_buffer_flags
                    .remove(gst::BufferFlags::DELTA_UNIT);
            }
            if buffer.flags().contains(gst::BufferFlags::DISCONT) {
                started_state
                    .fragment_buffer_flags
                    .insert(gst::BufferFlags::DISCONT);
            }
            gst::trace!(CAT, imp: self, "Updated state={:?}", started_state);
        }

        loop {
            match started_state.mp4_parser.pop_atom() {
                Some(atom) => {
                    gst::log!(CAT, imp: self, "atom_size={}, atom_type={}", atom.len(), atom.atom_type);
                    match atom.atom_type {
                        ATOM_TYPE_FTYPE => {
                            started_state.ftype_atom = Some(atom);
                            gst::log!(CAT, imp: self, "ftype_atom={:?}", started_state.ftype_atom);
                        }
                        ATOM_TYPE_MOOV => {
                            started_state.moov_atom = Some(atom);
                            gst::log!(CAT, imp: self, "moov_atom={:?}", started_state.moov_atom);
                        }
                        ATOM_TYPE_MOOF => {
                            started_state.moof_atom = Some(atom);
                            gst::log!(CAT, imp: self, "moof_atom={:?}", started_state.moof_atom);
                        }
                        ATOM_TYPE_MDAT => {
                            let mdat_atom = atom;
                            match (
                                started_state.ftype_atom.as_ref(),
                                started_state.moov_atom.as_ref(),
                                started_state.moof_atom.as_ref(),
                            ) {
                                (Some(ftype_atom), Some(moov_atom), Some(moof_atom)) => {
                                    let include_header = !started_state
                                        .fragment_buffer_flags
                                        .contains(gst::BufferFlags::DELTA_UNIT);
                                    let header_len = if include_header {
                                        ftype_atom.len() + moov_atom.len()
                                    } else {
                                        0
                                    };
                                    let output_buf_len =
                                        header_len + moof_atom.len() + mdat_atom.len();

                                    gst::log!(CAT, imp:self, "Pushing buffer; include_header={}, ftype.len={}, moov.len={}, moof.len={}, mdat.len={}",
                                    include_header, ftype_atom.len(), moov_atom.len(), moof_atom.len(), mdat_atom.len());

                                    let mut gst_buffer =
                                        gst::Buffer::with_size(output_buf_len).unwrap();
                                    {
                                        let buffer_ref = gst_buffer.get_mut().unwrap();
                                        buffer_ref.set_pts(started_state.fragment_pts);
                                        buffer_ref.set_dts(started_state.fragment_dts);
                                        let duration = started_state.fragment_max_pts_plus_duration
                                            - started_state.fragment_pts;
                                        buffer_ref.set_duration(duration);
                                        buffer_ref.set_offset(
                                            started_state
                                                .fragment_offset
                                                .unwrap_or(gst::BUFFER_OFFSET_NONE),
                                        );
                                        buffer_ref.set_offset_end(
                                            started_state
                                                .fragment_offset_end
                                                .unwrap_or(gst::BUFFER_OFFSET_NONE),
                                        );
                                        buffer_ref.set_flags(started_state.fragment_buffer_flags);
                                        let mut buffer_map = buffer_ref.map_writable().unwrap();
                                        let slice = buffer_map.as_mut_slice();
                                        let mut pos = 0;
                                        if include_header {
                                            slice[pos..pos + ftype_atom.len()]
                                                .copy_from_slice(&ftype_atom.atom_bytes);
                                            pos += ftype_atom.len();
                                            slice[pos..pos + moov_atom.len()]
                                                .copy_from_slice(&moov_atom.atom_bytes);
                                            pos += moov_atom.len();
                                        }
                                        slice[pos..pos + moof_atom.len()]
                                            .copy_from_slice(&moof_atom.atom_bytes);
                                        pos += moof_atom.len();
                                        slice[pos..pos + mdat_atom.len()]
                                            .copy_from_slice(&mdat_atom.atom_bytes);
                                        pos += mdat_atom.len();
                                        assert_eq!(pos, output_buf_len);

                                        //FIXME: Work on the Json here, instead of redoing it in a new method.
                                    }
                                    // Clear fragment variables.
                                    started_state.fragment_pts = ClockTime::none();
                                    started_state.fragment_dts = ClockTime::none();
                                    started_state.fragment_max_pts_plus_duration = ClockTime::none();
                                    started_state.fragment_offset = None;
                                    started_state.fragment_offset_end = None;
                                    started_state.fragment_buffer_flags = gst::BufferFlags::DELTA_UNIT;
                                    // Push new buffer.
                                    gst::log!(CAT, imp: self, "Pushing buffer {:?}", gst_buffer);
                                }
                                _ => {
                                    gst_warning!(CAT, obj: pad, "Received mdat without ftype, moov, or moof");
                                }
                            }
                        }
                        _ => {
                            gst_warning!(CAT, obj: pad, "Unknown atom type {:?}", atom);
                        }
                    }
                }
                None => break,
            }
        }
        gst_trace!(CAT, obj: element, "sink_chain: END: state={:?}", state);

        Ok(gst::FlowSuccess::Ok)
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();

        if let State::Started(ref mut state) = *state {
            gst::warning!(CAT, imp: self, "Stopping all threads");

            //TODO: Gracefully kill all running servers here
        }

        *state = State::Stopped;
        gst::info!(CAT, imp: self, "Stopped");

        Ok(())
    }
}
