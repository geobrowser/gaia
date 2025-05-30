use grc20::pb::grc20::Edit;
use prost::Message;
use reqwest::Client as ReqwestClient;

#[derive(Debug, thiserror::Error)]
pub enum IpfsError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("prost error: {0}")]
    Prost(#[from] prost::DecodeError),
    #[error("cid error: {0}")]
    CidError(String),
}

type Result<T> = std::result::Result<T, IpfsError>;

pub fn deserialize(buf: &[u8]) -> std::result::Result<Edit, prost::DecodeError> {
    Edit::decode(buf)
}

pub struct IpfsClient {
    url: String,
    client: ReqwestClient,
}

impl IpfsClient {
    pub fn new(url: &str) -> Self {
        IpfsClient {
            url: url.to_string(),
            client: ReqwestClient::new(),
        }
    }

    pub async fn get(&self, hash: &str) -> Result<Edit> {
        // @TODO: Error handle
        let cid = if let Some((_, maybe_cid)) = hash.split_once("://") {
            maybe_cid
        } else {
            ""
        };

        // @TODO: Should retry this fetch
        let bytes = self.get_bytes(cid).await?;

        let data = deserialize(&bytes)?;
        return Ok(data);
    }

    pub async fn get_bytes(&self, hash: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.url, hash);
        let res = self.client.get(&url).send().await?;
        let bytes = res.bytes().await?;
        Ok(bytes.to_vec())
    }
}
