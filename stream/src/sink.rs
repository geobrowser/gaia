use anyhow::{Context, Error, format_err};
use futures03::StreamExt;
use lazy_static::lazy_static;
use prost::Message;
use regex::Regex;
use semver::Version;

use std::{env, process::exit, sync::Arc};

use crate::{
    pb::sf::substreams::{
        rpc::v2::{BlockScopedData, BlockUndoSignal},
        v1::Package,
    },
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse, SubstreamsStream},
};

pub trait PreprocessedSink<P: Send>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn preprocess_block_scoped_data(
        &self,
        block_data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<P, Self::Error>> + Send;

    fn process_block_scoped_data(
        &self,
        block_data: &BlockScopedData,
        decoded_data: P,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
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

    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        // FIXME: Handling of the cursor is missing here. It should be saved each time
        // a full block has been correctly processed/persisted. The saving location
        // is your responsibility.
        //
        // By making it persistent, we ensure that if we crash, on startup we are
        // going to read it back from database and start back our SubstreamsStream
        // with it ensuring we are continuously streaming without ever losing a single
        // element.
        async { Ok(()) }
    }

    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        // FIXME: Handling of the cursor is missing here. It should be loaded from
        // somewhere (local file, database, cloud storage) and then `SubstreamStream` will
        // be able correctly resume from the right block.
        async { Ok(None) }
    }

    fn run(
        &self,
        endpoint_url: &str,
        spkg_file: &str,
        module_name: &str,
        start_block: i64,
        end_block: u64,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        async move {
            let token_env = env::var("SUBSTREAMS_API_TOKEN").unwrap_or("".to_string());
            let mut token: Option<String> = None;
            if !token_env.is_empty() {
                token = Some(token_env);
            }

            let cursor: Option<String> = self.load_persisted_cursor().await?;

            println!("Processing block {}", spkg_file);

            let package = read_package(spkg_file).await.unwrap();

            let endpoint = Arc::new(SubstreamsEndpoint::new(&endpoint_url, token).await?);

            let mut stream = SubstreamsStream::new(
                endpoint.clone(),
                cursor,
                package.modules.clone(),
                module_name.to_string(),
                start_block,
                end_block,
            );

            loop {
                match stream.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        let decoded_data = self.preprocess_block_scoped_data(&data).await?;
                        self.process_block_scoped_data(&data, decoded_data).await?;
                        self.persist_cursor(data.cursor, data.clock.unwrap().number)
                            .await?;
                    }
                    Some(Ok(BlockResponse::Undo(undo_signal))) => {
                        self.process_block_undo_signal(&undo_signal)?;
                        self.persist_cursor(
                            undo_signal.last_valid_cursor,
                            undo_signal.last_valid_block.unwrap().number,
                        )
                        .await?;
                    }
                    Some(Err(err)) => {
                        println!();
                        println!("Stream terminated with error");
                        println!("{:?}", err);
                        exit(1);
                    }
                }
            }

            Ok(())
        }
    }
}

pub trait Sink<T: Send>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn process_block_scoped_data(
        &self,
        block_data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
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

    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        // FIXME: Handling of the cursor is missing here. It should be saved each time
        // a full block has been correctly processed/persisted. The saving location
        // is your responsibility.
        //
        // By making it persistent, we ensure that if we crash, on startup we are
        // going to read it back from database and start back our SubstreamsStream
        // with it ensuring we are continuously streaming without ever losing a single
        // element.
        async { Ok(()) }
    }

    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        // FIXME: Handling of the cursor is missing here. It should be loaded from
        // somewhere (local file, database, cloud storage) and then `SubstreamStream` will
        // be able correctly resume from the right block.
        async { Ok(None) }
    }

    fn run(
        &self,
        endpoint_url: &str,
        spkg_file: &str,
        module_name: &str,
        start_block: i64,
        end_block: u64,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        async move {
            let token_env = env::var("SUBSTREAMS_API_TOKEN").unwrap_or("".to_string());
            let mut token: Option<String> = None;
            if !token_env.is_empty() {
                token = Some(token_env);
            }

            let cursor: Option<String> = self.load_persisted_cursor().await?;

            println!("Processing block {}", spkg_file);

            let package = read_package(spkg_file).await.unwrap();

            let endpoint = Arc::new(SubstreamsEndpoint::new(&endpoint_url, token).await?);

            let mut stream = SubstreamsStream::new(
                endpoint.clone(),
                cursor,
                package.modules.clone(),
                module_name.to_string(),
                start_block,
                end_block,
            );

            loop {
                match stream.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        self.process_block_scoped_data(&data).await?;
                        self.persist_cursor(data.cursor, data.clock.unwrap().number)
                            .await?;
                    }
                    Some(Ok(BlockResponse::Undo(undo_signal))) => {
                        self.process_block_undo_signal(&undo_signal)?;
                        self.persist_cursor(
                            undo_signal.last_valid_cursor,
                            undo_signal.last_valid_block.unwrap().number,
                        )
                        .await?;
                    }
                    Some(Err(err)) => {
                        println!();
                        println!("Stream terminated with error");
                        println!("{:?}", err);
                        exit(1);
                    }
                }
            }

            Ok(())
        }
    }
}

lazy_static! {
    static ref MODULE_NAME_REGEXP: Regex = Regex::new(r"^([a-zA-Z][a-zA-Z0-9_-]{0,63})$").unwrap();
}

const REGISTRY_URL: &str = "https://spkg.io";

pub async fn read_package(input: &str) -> Result<Package, anyhow::Error> {
    let mut mutable_input = input.to_string();

    if let Ok(package_and_version) = parse_standard_package_and_version(input) {
        mutable_input = format!(
            "{}/v1/packages/{}/{}",
            REGISTRY_URL, package_and_version.0, package_and_version.1
        );
    }

    if mutable_input.starts_with("http") {
        return read_http_package(&mutable_input).await;
    }

    // Assume it's a local file
    let content = std::fs::read(&mutable_input)
        .context(format_err!("read package from file '{}'", mutable_input))?;
    Package::decode(content.as_ref()).context("decode command")
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

    if parts.len() == 1 || parts.get(1).is_none_or(|v| v.is_empty() || *v == "latest") {
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
