use std::env;
use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};
use arrow::array::{
    Array, BinaryArray, FixedSizeBinaryArray, TimestampNanosecondArray, UInt32Array, UInt64Array,
};
use arrow_flight::{
    flight_service_client::FlightServiceClient, sql::client::FlightSqlServiceClient,
};
use futures::StreamExt;
use hermes_instrumentation::{info, warn};
use hermes_relay::Action;

use crate::SPACE_REGISTRY_ADDRESS_HEX;

const DEFAULT_FLIGHT_URL: &str = "http://localhost:1602";
const DEFAULT_DATASET: &str = "geo/actions";
const DEFAULT_START_BLOCK: u64 = 82655;
const DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;

#[derive(Debug, Clone)]
pub struct AmpStreamConfig {
    pub flight_url: String,
    pub dataset: String,
    pub start_block: u64,
    pub end_block: Option<u64>,
    pub actions_address: String,
    pub reconnect_delay: Duration,
}

impl AmpStreamConfig {
    pub fn from_env() -> Self {
        let flight_url =
            env::var("AMP_FLIGHT_URL").unwrap_or_else(|_| DEFAULT_FLIGHT_URL.to_string());
        let dataset = env::var("AMP_DATASET").unwrap_or_else(|_| DEFAULT_DATASET.to_string());
        let start_block = env::var("AMP_START_BLOCK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_START_BLOCK);
        let end_block = env::var("AMP_END_BLOCK").ok().and_then(|v| v.parse().ok());
        let actions_address = env::var("AMP_ACTIONS_ADDRESS")
            .unwrap_or_else(|_| SPACE_REGISTRY_ADDRESS_HEX.to_string());
        let reconnect_delay = env::var("AMP_RECONNECT_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_RECONNECT_DELAY_SECS));

        Self {
            flight_url,
            dataset,
            start_block,
            end_block,
            actions_address,
            reconnect_delay,
        }
    }
}

#[derive(Debug)]
pub struct AmpBlock {
    pub block_num: u64,
    pub timestamp_secs: u64,
    pub actions: Vec<Action>,
    pub cursor: String,
}

pub async fn stream_actions<F, Fut>(config: AmpStreamConfig, mut handle_block: F) -> Result<()>
where
    F: FnMut(AmpBlock) -> Fut + Send,
    Fut: Future<Output = Result<()>> + Send,
{
    let mut resume_from = config.start_block;

    loop {
        if let Some(end_block) = config.end_block {
            if resume_from > end_block {
                info!(
                    resume_from,
                    end_block, "Amp stream reached configured end block"
                );
                return Ok(());
            }
        }

        let mut sql = format!(
            "SELECT _block_num as block_num, timestamp, log_index, topic0, topic1, topic2, topic3, data \
             FROM \"{}\".logs \
             WHERE address = evm_encode_hex('{}') \
               AND _block_num >= {} \
               AND topic0 IS NOT NULL AND topic1 IS NOT NULL AND topic2 IS NOT NULL AND topic3 IS NOT NULL",
            config.dataset, config.actions_address, resume_from
        );

        if let Some(end_block) = config.end_block {
            sql.push_str(&format!(" AND _block_num <= {}", end_block));
        }

        sql.push_str(" SETTINGS stream = true");

        info!(
            dataset = %config.dataset,
            start_block = resume_from,
            sql = %sql,
            "Starting Amp Flight stream"
        );

        let flight_client = FlightServiceClient::connect(config.flight_url.clone())
            .await
            .context("connect flight client")?;
        let mut client = FlightSqlServiceClient::new_from_inner(flight_client);

        let mut info = client
            .execute(sql, None)
            .await
            .context("execute flight sql")?;
        let ticket = info.endpoint[0]
            .ticket
            .take()
            .context("flight query did not return a ticket")?;

        let mut stream = client.do_get(ticket).await.context("do_get")?;

        let mut current_block: Option<u64> = None;
        let mut current_timestamp_secs: u64 = 0;
        let mut current_actions: Vec<(u32, Action)> = Vec::new();
        let mut batch_count: u64 = 0;
        let mut row_count: u64 = 0;

        while let Some(batch) = stream.next().await {
            let batch = batch.context("receive flight batch")?;
            if batch.num_rows() == 0 {
                continue;
            }
            batch_count += 1;
            row_count += batch.num_rows() as u64;

            let block_nums = batch
                .column_by_name("block_num")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .context("missing block_num column")?;
            let timestamps = batch
                .column_by_name("timestamp")
                .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
                .context("missing timestamp column")?;
            let log_indexes = batch
                .column_by_name("log_index")
                .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
                .context("missing log_index column")?;
            let topic0 = batch
                .column_by_name("topic0")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
                .context("missing topic0 column")?;
            let topic1 = batch
                .column_by_name("topic1")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
                .context("missing topic1 column")?;
            let topic2 = batch
                .column_by_name("topic2")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
                .context("missing topic2 column")?;
            let topic3 = batch
                .column_by_name("topic3")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
                .context("missing topic3 column")?;
            let data = batch
                .column_by_name("data")
                .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
                .context("missing data column")?;

            for row in 0..batch.num_rows() {
                if topic0.is_null(row)
                    || topic1.is_null(row)
                    || topic2.is_null(row)
                    || topic3.is_null(row)
                {
                    continue;
                }

                let block_num = block_nums.value(row);
                let timestamp_nanos = timestamps.value(row);
                let timestamp_secs = (timestamp_nanos / 1_000_000_000) as u64;

                if current_block != Some(block_num) {
                    if let Some(prev_block) = current_block {
                        let actions = AmpBlock {
                            block_num: prev_block,
                            timestamp_secs: current_timestamp_secs,
                            actions: sort_actions_by_log_index(&mut current_actions),
                            cursor: format!("amp:{}", prev_block),
                        };
                        handle_block(actions).await?;
                        resume_from = prev_block.saturating_add(1);
                    }

                    current_block = Some(block_num);
                    current_timestamp_secs = timestamp_secs;
                }

                let log_index = log_indexes.value(row);
                let action = Action {
                    from_id: topic0.value(row)[0..16].to_vec(),
                    to_id: topic1.value(row)[0..16].to_vec(),
                    action: topic2.value(row).to_vec(),
                    topic: topic3.value(row).to_vec(),
                    data: data.value(row).to_vec(),
                };

                current_actions.push((log_index, action));
            }
        }

        if let Some(prev_block) = current_block {
            let actions = AmpBlock {
                block_num: prev_block,
                timestamp_secs: current_timestamp_secs,
                actions: sort_actions_by_log_index(&mut current_actions),
                cursor: format!("amp:{}", prev_block),
            };
            handle_block(actions).await?;
            resume_from = prev_block.saturating_add(1);
        }

        if batch_count == 0 {
            warn!(resume_from, "Amp Flight stream ended without batches");
        }
        warn!(
            resume_from,
            batch_count, row_count, "Amp Flight stream ended, reconnecting after delay"
        );
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

fn sort_actions_by_log_index(actions: &mut Vec<(u32, Action)>) -> Vec<Action> {
    actions.sort_by_key(|(log_index, _)| *log_index);
    actions.drain(..).map(|(_, action)| action).collect()
}
