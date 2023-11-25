use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;
use gst_base::BaseSink;
use once_cell::sync::Lazy;
use std::sync::Mutex;

// Define the plugin's instance structure with a URL property.
pub struct MoqSink {
    sink: gst_base::BaseSink,
    url: Mutex<Option<String>>, // Store the URL property value
    // Add additional fields for moq_transport as needed.
}

// Implement the GObject subclass for our sink.
#[glib::object_subclass]
impl ObjectSubclass for MoqSink {
    const NAME: &'static str = "MoqSink";
    type Type = super::MoqSink;
    type ParentType = BaseSink;
}

// Implement the GstObject trait for our sink to set up the metadata.
impl GstObjectImpl for MoqSink {}

// Implement the Element trait for our sink. This is where we can add pads and set up
// the element's functionality.
impl ElementImpl for MoqSink {
    // Define metadata for the element.
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "MoQ Sink",
                "Sink/Network",
                "Send data over QUIC using the MoQ protocol",
                "Wanjohi Ryan <elviswanjohi47@gmail.com>",
            )
        });
        Some(&*ELEMENT_METADATA)
    }
}

// Implement the properties and their behavior for the sink.
impl ObjectImpl for MoqSink {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::new(
                    "url",
                    "URL",
                    "URL to send data to",
                    None, // Default value
                    glib::ParamFlags::READWRITE,
                ),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "url" => {
                let mut url = self.url.lock().unwrap();
                *url = value.get().expect("Type checked upstream");
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "url" => {
                let url = self.url.lock().unwrap();
                url.as_ref().to_value()
            }
            _ => unimplemented!(),
        }
    }
}

// Implement the BaseSink trait for our sink. This is where most of the sink logic is implemented.
impl BaseSinkImpl for MoqSink {
    fn start(&self, element: &Self::Type) -> Result<(), gst::ErrorMessage> {
        // Initialize moq_transport here.
        Ok(())
    }

    fn stop(&self, element: &Self::Type) -> Result<(), gst::ErrorMessage> {
        // Clean up moq_transport here.
        Ok(())
    }

    fn render(&self, element: &Self::Type, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
        // Send buffer data over QUIC using moq_transport.
        // Extract data from the buffer
        let data = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

        // Assuming that the upstream element is producing MP4 atoms, we need to distinguish
        // between 'moof' and 'mdat' atoms. We will also assume that each buffer contains a
        // complete atom for simplicity. In a real-world scenario, you might need to handle
        // partial atoms and reassemble them here.
        //
        // The first 4 bytes of the buffer contain the size of the atom, and the next 4 bytes
        // contain the atom type. For 'moof' and 'mdat', we would send them as separate messages
        // or bundle them together, depending on the protocol requirements.

        // Check if the buffer is large enough to contain the size and type of the atom.
        if data.len() < 8 {
            gst::element_error!(
                element,
                gst::CoreError::Failed,
                ("Buffer is too small to contain an MP4 atom")
            );
            return Err(gst::FlowError::Error);
        }

        // Read atom size and type
        let size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        let atom_type = &data[4..8];

        // Ensure the buffer contains the complete atom
        if size > data.len() {
            gst::element_error!(
                element,
                gst::CoreError::Failed,
                ("Buffer does not contain the complete MP4 atom")
            );
            return Err(gst::FlowError::Error);
        }

        // Handle 'moof' and 'mdat' atoms
        match atom_type {
            b"moof" => {
                // Handle 'moof' atom
                // Send the 'moof' atom over the moq_transport protocol
                // self.moq_transport.send_moof(data.as_slice());
            },
            b"mdat" => {
                // Handle 'mdat' atom
                // Send the 'mdat' atom over the moq_transport protocol
                // self.moq_transport.send_mdat(data.as_slice());
            },
            _ => {
                // Handle other atoms or ignore them
            }
        }
        Ok(gst::FlowSuccess::Ok)
    }
}