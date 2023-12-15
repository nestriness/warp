use gst::glib;
use gst::glib::once_cell::sync::Lazy;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::prelude::*;
use gst_base::subclass::prelude::*;
use std::sync::Mutex;

use crate::relayurl::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::runtime::Runtime;
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

#[derive(Debug)]
struct StartedState {
    broadcast: broadcast::Publisher,
}

impl StartedState {
    pub fn new(broadcast: broadcast::Publisher) -> StartedState {
        StartedState { broadcast }
    }
}

#[derive(Default)]
enum State {
    #[default]
    Stopped,
    Completed,
    Started(StartedState),
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        DEBUG_CATEGORY,
        gst::DebugColorFlags::empty(),
        Some(ELEMENT_LONG_NAME),
    )
});

#[derive(Default)]
pub struct MoqSink {
    state: Mutex<State>,
    url: Mutex<Option<Url>>,
    settings: Mutex<Settings>,
}

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

        // // Initialize shared state and channels
        // let (sender, receiver) = mpsc::channel(32);
        // // self.sender = Some(sender);

        // // Spawn a new thread to run the Tokio runtime
        // std::thread::spawn(move || {
        //     // Create a new Tokio runtime
        //     let rt = Runtime::new().unwrap();

        //     // Block on the async code within the Tokio runtime
        //     rt.block_on(async move {
        //         // Set up your async tasks here
        //         let session_task = session.run();
        //         let media_task = media.run();

        //         tokio::select! {
        //             res = session_task => {
        //                 // Handle session result
        //                 // You might want to send a message to the GStreamer element using the channel
        //             },
        //             res = media_task => {
        //                 // Handle media result
        //                 // You might want to send a message to the GStreamer element using the channel
        //             },
        //             msg = receiver.recv() => {
        //                 // Handle messages sent from the GStreamer element to the Tokio runtime
        //                 // These might be control commands or data to process
        //             }
        //         }
        //     });
        // });

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

    type Interfaces = (gst::URIHandler,);
}

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
                    .blurb("The host of the relay server to connect tom, this can be a web url")
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
}