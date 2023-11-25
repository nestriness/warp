To be used as follows:

```rust
// Create the parsebin element
let parsebin = gst::ElementFactory::make("parsebin", None).expect("Failed to create parsebin element");

// Create the h264parse element
let h264parse = gst::ElementFactory::make("h264parse", None).expect("Failed to create h264parse element");

// Create the mp4mux element for fmp4
let mp4mux = gst::ElementFactory::make("mp4mux", None).expect("Failed to create mp4mux element");
mp4mux.set_property("fragment-duration", &2000u32).unwrap(); // Fragment duration in milliseconds
mp4mux.set_property_from_str("streamable", "true"); // For streaming output

// Add elements to the pipeline
pipeline.add_many(&[&waylandsrc, &parsebin, &h264parse, &mp4mux, &moqsink])?;

// Link the elements together
waylandsrc.link(&parsebin)?;
parsebin.link(&h264parse)?;
h264parse.link(&mp4mux)?;
mp4mux.link(&moqsink)?;
```