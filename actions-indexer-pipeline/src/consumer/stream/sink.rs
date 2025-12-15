use anyhow::{anyhow, format_err, Context, Error};
use futures03::StreamExt;
use regex::Regex;
use semver::Version;
use lazy_static::lazy_static;

use actions_indexer_shared::types::{ActionRaw, ActionType, ObjectType};

use super::pb::sf::substreams::rpc::v2::{BlockScopedData, BlockUndoSignal};
use super::pb::sf::substreams::v1::Package;
use super::pb::sf::substreams::v1::module::input::{Input, Params};
use super::pb::sf::actions::v1::{Action, Actions};
use super::substreams::SubstreamsEndpoint;
use super::substreams_stream::{BlockResponse, SubstreamsStream};
use prost::Message;
use std::sync::Arc;
use crate::errors::ConsumerError;
use crate::consumer::{ConsumeActionsStream, StreamMessage, BlockDataMessage};

lazy_static! {
    static ref MODULE_NAME_REGEXP: Regex = Regex::new(r"^([a-zA-Z][a-zA-Z0-9_-]{0,63})$").unwrap();
}

const REGISTRY_URL: &str = "https://spkg.io";

pub struct SubstreamsStreamProvider {
    endpoint_url: String,
    package_file: String,
    module_name: String,
    block_range: Option<String>,
    params: Vec<Param>,
    token: Option<String>,
}

impl SubstreamsStreamProvider {
    /// Creates a new Sink instance with the provided configuration
    pub fn new(
        endpoint_url: String,
        package_file: String,
        module_name: String,
        block_range: Option<String>,
        params: Vec<Param>,
        token: Option<String>,
    ) -> Self {
        let mut endpoint_url = endpoint_url;
        if !endpoint_url.starts_with("http") {
            endpoint_url = format!("{}://{}", "https", &endpoint_url);
        }

        Self {
            endpoint_url,
            package_file,
            module_name,
            block_range,
            params,
            token,
        }
    }

    pub fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<Vec<ActionRaw>, Error> {
        let now = chrono::Utc::now();
        let block_number = data.clock.as_ref().unwrap().number;
        println!("{} - Processing block {}", now.to_rfc3339(), block_number);

        let output = data.output
            .as_ref()
            .ok_or_else(|| ConsumerError::MissingField("output".to_string()))?
            .map_output
            .as_ref()
            .ok_or_else(|| ConsumerError::MissingField("map_output".to_string()))?;
        let actions = Actions::decode(output.value.as_slice())
            .map_err(|e| ConsumerError::DecodingActions(e.to_string()))?;
        let raw_actions = actions
            .actions
            .iter()
            .map(|action| ActionRaw::try_from(action))
            .collect::<Result<Vec<ActionRaw>, ConsumerError>>()?;

        Ok(raw_actions)
    }
    
    pub fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), anyhow::Error> {
        // `BlockUndoSignal` must be treated as "delete every data that has been recorded after
        // block height specified by block in BlockUndoSignal". In the example above, this means
        // you must delete changes done by `Block #7b` and `Block #6b`. The exact details depends
        // on your own logic. If for example all your added record contain a block number, a
        // simple way is to do `delete all records where block_num > 5` which is the block num
        // received in the `BlockUndoSignal` (this is true for append only records, so when only `INSERT` are allowed).
        unimplemented!(
            "you must implement some kind of block undo handling, or request only final blocks (tweak substreams_stream.rs)"
        )
    }
    
    pub fn persist_cursor(&self, _cursor: String) -> Result<(), anyhow::Error> {
        // FIXME: Handling of the cursor is missing here. It should be saved each time
        // a full block has been correctly processed/persisted. The saving location
        // is your responsibility.
        //
        // By making it persistent, we ensure that if we crash, on startup we are
        // going to read it back from database and start back our SubstreamsStream
        // with it ensuring we are continuously streaming without ever losing a single
        // element.
        Ok(())
    }
    
    pub fn load_persisted_cursor(&self) -> Result<Option<String>, anyhow::Error> {
        // FIXME: Handling of the cursor is missing here. It should be loaded from
        // somewhere (local file, database, cloud storage) and then `SubstreamStream` will
        // be able correctly resume from the right block.
        Ok(None)
    }
    
}

#[async_trait::async_trait]
impl ConsumeActionsStream for SubstreamsStreamProvider {
    async fn stream_events(&self, sender: tokio::sync::mpsc::Sender<StreamMessage>, cursor: Option<String>) -> Result<(), ConsumerError> {
        let package = read_package(&self.package_file, self.params.clone()).await.map_err(|e| ConsumerError::ReadingPackage(e.to_string()))?;
        let block_range = read_block_range(&package, &self.module_name, self.block_range.clone()).map_err(|e| ConsumerError::ReadingBlockRange(e.to_string()))?;

        let endpoint =
            Arc::new(SubstreamsEndpoint::new(&self.endpoint_url, self.token.clone()).await.map_err(|e| ConsumerError::ReadingEndpoint(e.to_string()))?);

        let mut stream = SubstreamsStream::new(
            endpoint,
            cursor,
            package.modules,
            self.module_name.clone(),
            block_range.0,
            block_range.1
        );

        loop {
            match stream.next().await {
                None => {
                    sender.send(StreamMessage::StreamEnd).await.map_err(|e| ConsumerError::ChannelSend(e.to_string()))?;
                    break;
                }
                Some(Ok(BlockResponse::New(data))) => {
                    let actions = self.process_block_scoped_data(&data).map_err(|e| ConsumerError::ProcessingBlockScopedData(e.to_string()))?;
                    sender.send(StreamMessage::BlockData(BlockDataMessage {
                        actions,
                        cursor: data.cursor,
                        block_number: data.clock.unwrap().number as i64,
                    })).await.map_err(|e| ConsumerError::ChannelSend(e.to_string()))?;
                }
                Some(Ok(BlockResponse::Undo(undo_signal))) => {
                    sender.send(StreamMessage::UndoSignal(undo_signal)).await.map_err(|e| ConsumerError::ChannelSend(e.to_string()))?;
                }
                Some(Err(err)) => {
                    println!();
                    println!("Stream terminated with error");
                    println!("{:?}", err);
                    sender.send(StreamMessage::Error(ConsumerError::StreamingError(err.to_string()))).await.map_err(|e| ConsumerError::ChannelSend(e.to_string()))?;
                    break;
                }
            }
        }

        Ok(())
    }
}


fn read_block_range(
    pkg: &Package,
    module_name: &str,
    block_range: Option<String>,
) -> Result<(i64, u64), anyhow::Error> {
    let module = pkg
        .modules
        .as_ref()
        .unwrap()
        .modules
        .iter()
        .find(|m| m.name == module_name)
        .ok_or_else(|| format_err!("module '{}' not found in package", module_name))?;

    let mut input: String = "".to_string();
    if let Some(range) = block_range {
        input = range;
    };

    let (prefix, suffix) = match input.split_once(":") {
        Some((prefix, suffix)) => (prefix.to_string(), suffix.to_string()),
        None => ("".to_string(), input),
    };

    let start: i64 = match prefix.as_str() {
        "" => module.initial_block as i64,
        x if x.starts_with("+") => {
            let block_count = x
                .trim_start_matches("+")
                .parse::<u64>()
                .context("argument <stop> is not a valid integer")?;

            (module.initial_block + block_count) as i64
        }
        x => x
            .parse::<i64>()
            .context("argument <start> is not a valid integer")?,
    };

    let stop: u64 = match suffix.as_str() {
        "" => 0,
        "-" => 0,
        x if x.starts_with("+") => {
            let block_count = x
                .trim_start_matches("+")
                .parse::<u64>()
                .context("argument <stop> is not a valid integer")?;

            start as u64 + block_count
        }
        x => x
            .parse::<u64>()
            .context("argument <stop> is not a valid integer")?,
    };

    return Ok((start, stop));
}

async fn read_package(input: &str, params: Vec<Param>) -> Result<Package, Error> {
    let mut mutable_input = input.to_string();

    let val = parse_standard_package_and_version(input);
    if val.is_ok() {
        let package_and_version = val?;
        mutable_input = format!(
            "{}/v1/packages/{}/{}",
            REGISTRY_URL, package_and_version.0, package_and_version.1
        );
    }

    let mut package = if mutable_input.starts_with("http") {
        read_http_package(&mutable_input).await
    } else {
        // Assume it's a local file
        let content = std::fs::read(&mutable_input)
            .context(format_err!("read package from file '{}'", mutable_input))?;
        Package::decode(content.as_ref()).context("decode command")
    }?;

    if params.len() > 0 {
        // Find the module by name and apply the block filter
        if let Some(modules) = &mut package.modules {
            for param in params {
                if let Some(module) = modules
                    .modules
                    .iter_mut()
                    .find(|m| m.name == param.module_name)
                {
                    module.inputs[0].input = Some(Input::Params(Params {
                        value: param.expression,
                    }));
                }
            }
        }
        Ok(package)
    } else {
        Ok(package)
    }
}

/// Reads the module name and filter from the input string.
///
/// Example input would be `filtered_events:(type:transfer)`
fn read_params_flag(input: &str) -> anyhow::Result<Vec<Param>> {
    let mut params = vec![];

    let value = input
        .trim_start_matches("--params")
        .trim()
        .trim_start_matches("=")
        .trim();
    if value.is_empty() {
        return Err(anyhow!(
            "wrong --params input value '{}': empty string",
            value
        ));
    }

    for param in value.split(",") {
        match param.split_once(":") {
            Some((module_name, expression)) => params.push(Param {
                module_name: module_name.trim().to_string(),
                expression: expression.trim().to_string(),
            }),
            None => {
                return Err(anyhow!(
                    "wrong --params value for '{}': missing ':' delimiter",
                    param
                ));
            }
        }
    }

    Ok(params)
}

async fn read_http_package(input: &str) -> Result<Package, anyhow::Error> {
    let body = reqwest::get(input).await?.bytes().await?;

    Package::decode(body).context("decode command")
}

fn parse_standard_package_and_version(input: &str) -> Result<(String, String), Error> {
    let parts: Vec<&str> = input.split('@').collect();
    if parts.len() > 2 {
        return Err(format_err!(
            "package name: {} does not follow the convention of <package>@<version>",
            input
        ));
    }

    let package_name = parts[0].to_string();
    if !MODULE_NAME_REGEXP.is_match(&package_name) {
        return Err(format_err!(
            "package name {} does not match regexp {}",
            package_name,
            MODULE_NAME_REGEXP.as_str()
        ));
    }

    if parts.len() == 1
        || parts
            .get(1)
            .map_or(true, |v| v.is_empty() || *v == "latest")
    {
        return Ok((package_name, "latest".to_string()));
    }

    let version = parts[1];
    if !is_valid_version(&version.replace("v", "")) {
        return Err(format_err!(
            "version '{}' is not valid Semver format",
            version
        ));
    }

    Ok((package_name, version.to_string()))
}

fn is_valid_version(version: &str) -> bool {
    Version::parse(version).is_ok()
}

#[derive(Clone)]
pub struct Param {
    pub module_name: String,
    pub expression: String,
}

impl TryFrom<&Action> for ActionRaw {
    type Error = ConsumerError;

    fn try_from(action: &Action) -> Result<Self, Self::Error> {
        Ok(ActionRaw {
            sender: action.sender.parse()
                .map_err(|e| ConsumerError::InvalidAddress(format!("sender: {}", e)))?,
            action_type: match action.action_type {
                0 => ActionType::Vote,
                _ => return Err(ConsumerError::InvalidActionType(format!("action_type: {}", action.action_type))),
            },
            action_version: action.action_version,
            space_pov: action.space_pov.parse()
                .map_err(|e| ConsumerError::InvalidAddress(format!("space_pov: {}", e)))?,
            object_id: action.object_id.parse()
                .map_err(|e| ConsumerError::InvalidUuid(format!("entity: {}", e)))?,
            group_id: if action.group_id.is_some() {
                Some(action.group_id.as_ref().unwrap().parse()
                    .map_err(|e| ConsumerError::InvalidUuid(format!("group_id: {}", e)))?)
            } else {
                None
            },
            metadata: action.metadata.as_ref().map(|metadata| metadata.to_vec().into()),
            block_number: action.block_number.into(),
            block_timestamp: action.block_timestamp.into(),
            tx_hash: action.tx_hash.parse()
                .map_err(|e| ConsumerError::InvalidTxHash(format!("tx_hash: {}", e)))?,
            object_type: match action.object_type {
                0 => ObjectType::Entity,
                1 => ObjectType::Relation,
                _ => return Err(ConsumerError::InvalidObjectType(format!("object_type: {}", action.object_type))),
            },
        })
    }
}