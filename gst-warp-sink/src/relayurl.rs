use url::Url;
#[derive(Clone)]
pub struct RelayUrl {
    pub host: String,
    pub port: u16,
    pub name: String,
}

impl ToString for RelayUrl {
    fn to_string(&self) -> String {
        format!("https://{}:{}/{}", self.host, self.port, self.name)
    }
}

pub fn parse_relay_url(url_str: &str) -> Result<Url, String> {
    let url = Url::try_from(s).map_err(|e| e.to_string())?;
    
    //TODO: I know this is redundant, but this might come in handy in the future
    // Make sure the scheme is moq
    if url.scheme() != "https" {
        return Err("url scheme must be https:// for WebTransport".to_string());
    }

    Ok(url)
}
