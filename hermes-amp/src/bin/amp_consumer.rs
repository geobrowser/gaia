use std::env;
use std::io::{self, Read, Write};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = env::var("AMP_JSONL_URL").unwrap_or_else(|_| "http://localhost:1603".to_string());

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

    let stream = matches!(
        env::var("AMP_STREAM").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(if stream {
            None
        } else {
            Some(Duration::from_secs(30))
        })
        .build()?;

    let mut response = client.post(url).body(sql).send()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text()?;
        return Err(format!("Amp returned {status}: {body}").into());
    }

    if stream {
        let mut stdout = io::stdout();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            stdout.write_all(&buffer[..read])?;
            stdout.flush()?;
        }
        return Ok(());
    }

    let body = response.text()?;
    print!("{body}");
    Ok(())
}
