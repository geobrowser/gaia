use std::env;

use arrow::array::{
    Array, BinaryArray, FixedSizeBinaryArray, TimestampNanosecondArray, UInt64Array,
};
use arrow_flight::{
    flight_service_client::FlightServiceClient, sql::client::FlightSqlServiceClient,
};
use futures::StreamExt;

use hermes_relay::{Action, Actions};

use crate::{Pipeline, PipelineError};

const DEFAULT_ACTIONS_ADDRESS: &str = "0xb01683b2f0d38d43fcd4d9aab980166988924132";

pub async fn run_amp_stream(pipeline: &Pipeline) -> Result<(), PipelineError> {
    let url = env::var("AMP_FLIGHT_URL").unwrap_or_else(|_| "http://localhost:1602".to_string());
    let dataset = env::var("AMP_DATASET").unwrap_or_else(|_| "geo/actions".to_string());
    let start_block: u64 = env::var("AMP_START_BLOCK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(82655);
    let address =
        env::var("AMP_ACTIONS_ADDRESS").unwrap_or_else(|_| DEFAULT_ACTIONS_ADDRESS.to_string());

    let sql = format!(
        "SELECT block_num, timestamp, log_index, topic0, topic1, topic2, topic3, data \
         FROM \"{}\".logs \
         WHERE address = evm_encode_hex('{}') \
           AND _block_num >= {} \
           AND topic0 IS NOT NULL AND topic1 IS NOT NULL AND topic2 IS NOT NULL AND topic3 IS NOT NULL \
         ORDER BY _block_num, log_index \
         SETTINGS stream = true",
        dataset, address, start_block
    );

    let flight_client =
        FlightServiceClient::connect(url).await.map_err(anyhow::Error::from)?;
    let mut client = FlightSqlServiceClient::new_from_inner(flight_client);

    let mut info = client.execute(sql, None).await.map_err(anyhow::Error::from)?;
    let ticket = info.endpoint[0]
        .ticket
        .take()
        .ok_or_else(|| anyhow::anyhow!("Flight query did not return a ticket"))?;

    let mut stream = client.do_get(ticket).await.map_err(anyhow::Error::from)?;

    let mut current_block: Option<u64> = None;
    let mut current_timestamp_secs: u64 = 0;
    let mut current_actions: Vec<Action> = Vec::new();

    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(anyhow::Error::from)?;
        if batch.num_rows() == 0 {
            continue;
        }

        let block_nums = batch
            .column_by_name("block_num")
            .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
            .ok_or_else(|| anyhow::anyhow!("missing block_num column"))?;
        let timestamps = batch
            .column_by_name("timestamp")
            .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
            .ok_or_else(|| anyhow::anyhow!("missing timestamp column"))?;
        let topic0 = batch
            .column_by_name("topic0")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("missing topic0 column"))?;
        let topic1 = batch
            .column_by_name("topic1")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("missing topic1 column"))?;
        let topic2 = batch
            .column_by_name("topic2")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("missing topic2 column"))?;
        let topic3 = batch
            .column_by_name("topic3")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("missing topic3 column"))?;
        let data = batch
            .column_by_name("data")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("missing data column"))?;

        for row in 0..batch.num_rows() {
            if topic0.is_null(row) || topic1.is_null(row) || topic2.is_null(row) || topic3.is_null(row) {
                continue;
            }

            let block_num = block_nums.value(row);
            let timestamp_nanos = timestamps.value(row);
            let timestamp_secs = (timestamp_nanos / 1_000_000_000) as u64;

            if current_block != Some(block_num) {
                if let Some(prev_block) = current_block {
                    let actions = Actions {
                        actions: std::mem::take(&mut current_actions),
                    };
                    let cursor = format!("amp:{}", prev_block);
                    pipeline
                        .process_actions_block(actions, prev_block, current_timestamp_secs, cursor)
                        .await?;
                }

                current_block = Some(block_num);
                current_timestamp_secs = timestamp_secs;
            }

            let t0 = topic0.value(row);
            let t1 = topic1.value(row);
            let t2 = topic2.value(row);
            let t3 = topic3.value(row);
            let payload = data.value(row);

            let action = Action {
                from_id: t0[0..16].to_vec(),
                to_id: t1[0..16].to_vec(),
                action: t2.to_vec(),
                topic: t3.to_vec(),
                data: payload.to_vec(),
            };

            current_actions.push(action);
        }
    }

    if let Some(prev_block) = current_block {
        let actions = Actions {
            actions: std::mem::take(&mut current_actions),
        };
        let cursor = format!("amp:{}", prev_block);
        pipeline
            .process_actions_block(actions, prev_block, current_timestamp_secs, cursor)
            .await?;
    }

    Ok(())
}
