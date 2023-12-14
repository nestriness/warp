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
