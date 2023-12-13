use glib::subclass::prelude::*;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst::ClockTime;
#[allow(unused_imports)]
use gst::{gst_debug, gst_error, gst_info, gst_log, gst_trace, gst_warning};
use once_cell::sync::Lazy;
use std::convert::TryInto;
use std::sync::Mutex;

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
struct StartedState {
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

enum State {
    Started { state: StartedState },
}

impl Default for State {
    fn default() -> State {
        State::Started {
            state: StartedState {
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
            },
        }
    }
}

pub struct MoqSink {
    state: Mutex<State>,
    srcpad: gst::Pad,
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        DEBUG_CATEGORY,
        gst::DebugColorFlags::empty(),
        Some(ELEMENT_LONG_NAME),
    )
});

impl MoqSink {
    fn sink_chain(
        &self,
        pad: &gst::Pad,
        element: &super::MoqSink,
        buffer: gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        gst_log!(CAT, obj: pad, "Handling buffer {:?}", buffer);

        let mut state = self.state.lock().unwrap();

        let state = match *state {
            State::Started { ref mut state, .. } => state,
        };

        let map = buffer.map_readable().map_err(|_| {
            gst::element_error!(element, gst::CoreError::Failed, ["Failed to map buffer"]);
            gst::FlowError::Error
        })?;
        let input_buf = map.as_ref();

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
            let pts_plus_duration = buffer.pts() + buffer.duration();
            if state.fragment_max_pts_plus_duration.is_none()
                || state.fragment_max_pts_plus_duration < pts_plus_duration
            {
                state.fragment_max_pts_plus_duration = pts_plus_duration;
            }
            if buffer.offset() != gst::BUFFER_OFFSET_NONE
                && (state.fragment_offset.is_none()
                    || state.fragment_offset.unwrap() > buffer.offset())
            {
                state.fragment_offset = Some(buffer.offset());
            }
            if buffer.offset_end() != gst::BUFFER_OFFSET_NONE
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
            gst_trace!(CAT, obj: pad, "Updated state={:?}", state);
        }

        loop {
            match state.mp4_parser.pop_atom() {
                Some(atom) => {
                    gst_log!(CAT, obj: pad, "atom_size={}, atom_type={}", atom.len(), atom.atom_type);
                    match atom.atom_type {
                        ATOM_TYPE_FTYPE => {
                            state.ftype_atom = Some(atom);
                            gst_log!(CAT, obj: pad, "ftype_atom={:?}", state.ftype_atom);
                        }
                        ATOM_TYPE_MOOV => {
                            state.moov_atom = Some(atom);
                            gst_log!(CAT, obj: pad, "moov_atom={:?}", state.moov_atom);
                        }
                        ATOM_TYPE_MOOF => {
                            state.moof_atom = Some(atom);
                            gst_log!(CAT, obj: pad, "moof_atom={:?}", state.moof_atom);
                        }
                        ATOM_TYPE_MDAT => {
                            let mdat_atom = atom;
                            match (
                                state.ftype_atom.as_ref(),
                                state.moov_atom.as_ref(),
                                state.moof_atom.as_ref(),
                            ) {
                                (Some(ftype_atom), Some(moov_atom), Some(moof_atom)) => {
                                    let include_header = !state
                                        .fragment_buffer_flags
                                        .contains(gst::BufferFlags::DELTA_UNIT);
                                    let header_len = if include_header {
                                        ftype_atom.len() + moov_atom.len()
                                    } else {
                                        0
                                    };
                                    let output_buf_len =
                                        header_len + moof_atom.len() + mdat_atom.len();
                                    gst_log!(CAT, obj: pad, "Pushing buffer; include_header={}, ftype.len={}, moov.len={}, moof.len={}, mdat.len={}",
                                        include_header, ftype_atom.len(), moov_atom.len(), moof_atom.len(), mdat_atom.len());
                                    let mut gst_buffer =
                                        gst::Buffer::with_size(output_buf_len).unwrap();
                                    {
                                        let buffer_ref = gst_buffer.get_mut().unwrap();
                                        buffer_ref.set_pts(state.fragment_pts);
                                        buffer_ref.set_dts(state.fragment_dts);
                                        let duration = state.fragment_max_pts_plus_duration
                                            - state.fragment_pts;
                                        buffer_ref.set_duration(duration);
                                        buffer_ref.set_offset(
                                            state
                                                .fragment_offset
                                                .unwrap_or(gst::BUFFER_OFFSET_NONE),
                                        );
                                        buffer_ref.set_offset_end(
                                            state
                                                .fragment_offset_end
                                                .unwrap_or(gst::BUFFER_OFFSET_NONE),
                                        );
                                        buffer_ref.set_flags(state.fragment_buffer_flags);
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
                                    state.fragment_pts = ClockTime::none();
                                    state.fragment_dts = ClockTime::none();
                                    state.fragment_max_pts_plus_duration = ClockTime::none();
                                    state.fragment_offset = None;
                                    state.fragment_offset_end = None;
                                    state.fragment_buffer_flags = gst::BufferFlags::DELTA_UNIT;
                                    // Push new buffer.
                                    gst_log!(CAT, obj: pad, "Pushing buffer {:?}", gst_buffer);
                                    // let _ = self.srcpad.push(gst_buffer)?;
                                    // self.send_to_network(gst_buffer)?;
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

    fn sink_event(&self, _pad: &gst::Pad, _element: &super::MoqSink, event: gst::Event) -> bool {
        self.srcpad.push_event(event)
    }

    fn sink_query(
        &self,
        _pad: &gst::Pad,
        _element: &super::MoqSink,
        query: &mut gst::QueryRef,
    ) -> bool {
        self.srcpad.peer_query(query)
    }

    fn send_to_network(&self, buffer: gst::Buffer) -> Result<(), Box<dyn std::error::Error>> {
        //Let this be our publisher
        Ok(())
    }
}

#[glib::object_subclass]
impl ObjectSubclass for MoqSink {
    const NAME: &'static str = ELEMENT_CLASS_NAME;
    type Type = super::MoqSink;
    type ParentType = gst::Element;

    fn with_class(klass: &Self::Class) -> Self {
        let templ = klass.pad_template("src").unwrap();
        let srcpad = gst::Pad::builder_with_template(&templ, Some("src"))
            .event_function(|pad, parent, event| {
                MoqSink::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_event(pad, element, event),
                )
            })
            .query_function(|pad, parent, query| {
                MoqSink::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_query(pad, element, query),
                )
            })
            .build();

        Self {
            state: Mutex::new(Default::default()),
            srcpad,
        }
    }

    type Interfaces;

    type Instance;

    type Class;
}

impl ObjectImpl for MoqSink {
    fn constructed(&self, obj: &Self::Type) {
        self.parent_constructed();
        obj.add_pad(&self.srcpad).unwrap();
    }
}

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
            let src_pad_template = gst::PadTemplate::new(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();
            vec![src_pad_template]
        });
        PAD_TEMPLATES.as_ref()
    }
}

//Moq
pub struct Publisher {
    track: track::Publisher,
}

impl Publisher {
    pub fn new(track: track::Publisher) -> Self {
        Self { track }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let start = Utc::now();
        let mut now = start;

        // Just for fun, don't start at zero.
        let mut sequence = start.minute();

        loop {
            let segment = self
                .track
                .create_segment(segment::Info {
                    sequence: VarInt::from_u32(sequence),
                    priority: 0,
                    expires: Some(time::Duration::from_secs(60)),
                })
                .context("failed to create minute segment")?;

            sequence += 1;

            tokio::spawn(async move {
                if let Err(err) = Self::send_segment(segment, now).await {
                    log::warn!("failed to send minute: {:?}", err);
                }
            });

            let next = now + chrono::Duration::minutes(1);
            let next = next.with_second(0).unwrap().with_nanosecond(0).unwrap();

            let delay = (next - now).to_std().unwrap();
            tokio::time::sleep(delay).await;

            now = next; // just assume we didn't undersleep
        }
    }

    async fn send_segment(
        mut segment: segment::Publisher,
        mut now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        // Everything but the second.
        let base = now.format("%Y-%m-%d %H:%M:").to_string();

        segment
            .fragment(VarInt::ZERO, base.len())?
            .chunk(base.clone().into())
            .context("failed to write base")?;

        loop {
            let delta = now.format("%S").to_string();
            let sequence = VarInt::from_u32(now.second() + 1);

            segment
                .fragment(sequence, delta.len())?
                .chunk(delta.clone().into())
                .context("failed to write delta")?;

            println!("{}{}", base, delta);

            let next = now + chrono::Duration::seconds(1);
            let next = next.with_nanosecond(0).unwrap();

            let delay = (next - now).to_std().unwrap();
            tokio::time::sleep(delay).await;

            // Get the current time again to check if we overslept
            let next = Utc::now();
            if next.minute() != now.minute() {
                return Ok(());
            }

            now = next;
        }
    }
}

//Sends to remote
pub struct Sender {
    track: track::Publisher,
}

impl Sender {
    pub fn new(track: track::Publisher) -> Self {
        Self { track }
    }

    pub async fn start(mut self) -> anyhow::Result<()> {
        // Disable tracing so we don't get a bunch of Quinn spam.
        let tracer = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::set_global_default(tracer).unwrap();

        let config = cli::Config::parse();

        // Create a list of acceptable root certificates.
        let mut roots = rustls::RootCertStore::empty();

        if config.tls_root.is_empty() {
            // Add the platform's native root certificates.
            for cert in
                rustls_native_certs::load_native_certs().context("could not load platform certs")?
            {
                roots
                    .add(&rustls::Certificate(cert.0))
                    .context("failed to add root cert")?;
            }
        } else {
            // Add the specified root certificates.
            for root in &config.tls_root {
                let root = fs::File::open(root).context("failed to open root cert file")?;
                let mut root = io::BufReader::new(root);

                let root = rustls_pemfile::certs(&mut root).context("failed to read root cert")?;
                anyhow::ensure!(root.len() == 1, "expected a single root cert");
                let root = rustls::Certificate(root[0].to_owned());

                roots.add(&root).context("failed to add root cert")?;
            }
        }

        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(roots)
            .with_no_client_auth();

        // Allow disabling TLS verification altogether.
        // if config.tls_disable_verify {
        //     let noop = NoCertificateVerification {};
        //     tls_config
        //         .dangerous()
        //         .set_certificate_verifier(Arc::new(noop));
        // }
        Ok(())
    }

    async fn send_segment(
        mut segment: segment::Publisher,
        mut now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        // Everything but the second.
        let base = now.format("%Y-%m-%d %H:%M:").to_string();

        segment
            .fragment(VarInt::ZERO, base.len())?
            .chunk(base.clone().into())
            .context("failed to write base")?;

        loop {
            let delta = now.format("%S").to_string();
            let sequence = VarInt::from_u32(now.second() + 1);

            segment
                .fragment(sequence, delta.len())?
                .chunk(delta.clone().into())
                .context("failed to write delta")?;

            println!("{}{}", base, delta);

            let next = now + chrono::Duration::seconds(1);
            let next = next.with_nanosecond(0).unwrap();

            let delay = (next - now).to_std().unwrap();
            tokio::time::sleep(delay).await;

            // Get the current time again to check if we overslept
            let next = Utc::now();
            if next.minute() != now.minute() {
                return Ok(());
            }

            now = next;
        }
    }
}

pub struct NoCertificateVerification {}

impl rustls::client::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
