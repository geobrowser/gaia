use std::env;
use std::io::{self, Read, Write};

use arrow::json::writer::ArrayWriter;
use arrow_flight::{
    flight_service_client::FlightServiceClient, sql::client::FlightSqlServiceClient,
};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = env::var("AMP_FLIGHT_URL").unwrap_or_else(|_| "http://localhost:1602".to_string());

    let sql = match env::var("AMP_SQL") {
        Ok(value) => value,
        Err(_) => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if input.trim().is_empty() {
                return Err("No SQL provided. Set AMP_SQL or pipe SQL via stdin.".into());
            }
            input
        }
    };

    let flight_client = FlightServiceClient::connect(url).await?;
    let mut client = FlightSqlServiceClient::new_from_inner(flight_client);

    let mut info = client.execute(sql, None).await?;
    let ticket = info.endpoint[0]
        .ticket
        .take()
        .ok_or("Flight query did not return a ticket")?;

    let mut batches = client.do_get(ticket).await?;

    let mut stdout = io::stdout();
    while let Some(batch) = batches.next().await {
        let batch = batch?;
        let mut buf = Vec::new();
        let mut writer = ArrayWriter::new(&mut buf);
        writer.write(&batch)?;
        writer.finish()?;
        stdout.write_all(&buf)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}
