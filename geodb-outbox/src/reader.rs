//! The delivery side of the shadow: claim mirrored units from geo-db's work class and
//! acknowledge them, over the same held `geodb-client work pipe` the mirror writes through.
//!
//! This only reads and acknowledges geo-db's own copy. It never delivers anything and holds no
//! Postgres state: the caller compares each claimed unit with the Postgres row it delivered.
//!
//! A claim row is `<delivery key hex>\t<attempts>\t<epoch>\t<payload hex>`; the delivery key is
//! the ledger key (32 bytes) then the webhook id (16). `complete` is not epoch-fenced (a stale
//! epoch still completes), so an acknowledgement after a lease lapsed is harmless.

use crate::{Config, ENTER_ANSWER_MAX, MirrorError, PAYLOAD_MAX, Worker};

/// The longest delivery key: 32 ledger bytes and a 16 byte webhook id.
pub const KEY_BYTES: usize = 48;

/// The most units one claim asks for. A row is at most `2 * PAYLOAD_MAX` hex digits and a head.
pub const CLAIM_MAX: u16 = 16;

/// The longest acknowledgement body: a `-code=<Token> ...` refusal can run to the enter answer's
/// bound, so an ack is held to the same one.
const ACK_ANSWER_MAX: usize = ENTER_ANSWER_MAX;

const ROW_MAX: usize = 2 * KEY_BYTES + 2 * PAYLOAD_MAX + 64;

/// One claimed unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub key: [u8; KEY_BYTES],
    pub attempts: u32,
    pub epoch: u64,
    /// The payload the indexer entered: the outbox row's JSON text.
    pub payload: Vec<u8>,
}

impl Claimed {
    /// The ledger key, 64 lowercase hex digits: the Postgres `idempotency_key`.
    pub fn ledger_hex(&self) -> String {
        hex(&self.key[..32])
    }

    /// The webhook id, the Postgres `notification_deliveries.webhook_id`.
    pub fn webhook(&self) -> [u8; 16] {
        let mut w = [0u8; 16];
        w.copy_from_slice(&self.key[32..]);
        w
    }
}

/// Fill byte of a start marker's ledger prefix and webhook id. A real ledger key is a SHA-256, so
/// sixteen of these bytes at the front are not a delivery key.
pub const MARK: u8 = 0xFF;

/// True for the key of a start marker entered by [`OutboxReader::enter_marker`] (this process's
/// or a past one's).
pub fn is_marker(key: &[u8; KEY_BYTES]) -> bool {
    key[..16] == [MARK; 16] && key[32..] == [MARK; 16]
}

/// What an acknowledgement was answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Ack {
    Ok,
    AlreadyTerminal,
    Unknown,
    LeaseLost,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The `claim` request line.
pub fn claim_line(limit: u16, lease_ms: u64) -> String {
    format!("claim\t{}\t{lease_ms}\n", limit.clamp(1, CLAIM_MAX))
}

/// The `complete` request line for a claimed unit.
pub fn complete_line(key: &[u8; KEY_BYTES], epoch: u64) -> String {
    format!("complete\t{}\t{epoch}\n", hex(key))
}

/// The `retry` request line: back to `pending` after `delay_ms`, `attempts + 1`. Epoch-fenced.
pub fn retry_line(key: &[u8; KEY_BYTES], epoch: u64, delay_ms: u64) -> String {
    format!("retry\t{}\t{epoch}\t{delay_ms}\n", hex(key))
}

/// The `fail` request line: `failed` for good, `attempts + 1`. Epoch-fenced.
pub fn fail_line(key: &[u8; KEY_BYTES], epoch: u64) -> String {
    format!("fail\t{}\t{epoch}\n", hex(key))
}

/// The rows of a claim's answer. An empty body is no units.
pub fn parse_claim(body: &str) -> Result<Vec<Claimed>, MirrorError> {
    let bad = |l: &str| MirrorError::BadFrame(format!("claim row {l:?}"));
    let mut out = Vec::new();
    for line in body.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 4 {
            return Err(bad(line));
        }
        let key: [u8; KEY_BYTES] = unhex(f[0])
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| bad(line))?;
        out.push(Claimed {
            key,
            attempts: f[1].parse().map_err(|_| bad(line))?,
            epoch: f[2].parse().map_err(|_| bad(line))?,
            payload: unhex(f[3]).ok_or_else(|| bad(line))?,
        });
    }
    Ok(out)
}

pub fn parse_ack(body: &str) -> Result<Ack, MirrorError> {
    match body.trim_end() {
        "ack: ok" => Ok(Ack::Ok),
        "ack: already_terminal" => Ok(Ack::AlreadyTerminal),
        "ack: unknown" => Ok(Ack::Unknown),
        "ack: lease_lost" => Ok(Ack::LeaseLost),
        other => Err(MirrorError::BadFrame(other.to_string())),
    }
}

/// Claims and completes units. Used from one task; every failure is returned for the caller to
/// count, and a failure cools the pipe down exactly as the mirror's does.
pub struct OutboxReader {
    worker: Worker,
}

impl OutboxReader {
    pub fn new(cfg: Config) -> Self {
        Self {
            worker: Worker::new(cfg),
        }
    }

    /// `Some` only when `GEODB_OUTBOX_SHADOW_READ` is on and configured.
    pub fn from_env() -> Option<Self> {
        Config::from_env_flag("GEODB_OUTBOX_SHADOW_READ").map(Self::new)
    }

    /// Enters a start marker: a unit with a unique key (see [`is_marker`]) that sorts, in claim
    /// order, after every unit entered before it. A reader that claims the marker knows every
    /// unit it saw before it is older than this call. Returns the marker's key. An enter is
    /// put-if-absent, so a replay is harmless.
    pub async fn enter_marker(&mut self) -> Result<[u8; KEY_BYTES], MirrorError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut key = [MARK; KEY_BYTES];
        key[16..32].copy_from_slice(&nanos.to_be_bytes());
        let unit = crate::Unit {
            ledger_key_hex: hex(&key[..32]),
            event_type: "comment".into(),
            payload: "{}".into(),
            webhooks: vec![[MARK; 16]],
        };
        self.worker.enter(&unit).await?;
        Ok(key)
    }

    /// Claims up to `limit` units under a lease of `lease_ms`. A pipe that was already open and
    /// died (geo-db ends a pipe at its grant TTL) is reopened and the claim replayed once, with
    /// no cool-down. That is safe: a claim the dead pipe refused leased nothing, and one whose
    /// answer was lost leaves its units leased until the lease lapses, so they come back later
    /// (delayed, never counted twice: the matcher keeps one entry a key).
    pub async fn claim(&mut self, limit: u16, lease_ms: u64) -> Result<Vec<Claimed>, MirrorError> {
        let max = usize::from(CLAIM_MAX) * (ROW_MAX + 1);
        let body = self
            .worker
            .call_replaying(&claim_line(limit, lease_ms), max)
            .await?;
        parse_claim(&body)
    }

    /// Completes a claimed unit.
    pub async fn complete(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
        let body = self
            .worker
            .call_replaying(&complete_line(&c.key, c.epoch), ACK_ANSWER_MAX)
            .await?;
        parse_ack(&body)
    }

    /// Returns a claimed unit to `pending`, due after `delay_ms`, counting one failed try. A
    /// lapsed lease answers [`Ack::LeaseLost`] and changes nothing (plan 10.16 W10).
    pub async fn retry(&mut self, c: &Claimed, delay_ms: u64) -> Result<Ack, MirrorError> {
        let body = self
            .worker
            .call_replaying(&retry_line(&c.key, c.epoch, delay_ms), ACK_ANSWER_MAX)
            .await?;
        parse_ack(&body)
    }

    /// Ends a claimed unit as `failed`, counting one failed try. A lapsed lease answers
    /// [`Ack::LeaseLost`] and changes nothing.
    pub async fn fail(&mut self, c: &Claimed) -> Result<Ack, MirrorError> {
        let body = self
            .worker
            .call_replaying(&fail_line(&c.key, c.epoch), ACK_ANSWER_MAX)
            .await?;
        parse_ack(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const LEDGER: &str = "0707070707070707070707070707070707070707070707070707070707070707";

    fn key() -> [u8; KEY_BYTES] {
        let mut k = [7u8; KEY_BYTES];
        k[32..].copy_from_slice(&[9u8; 16]);
        k
    }

    fn reader(script: &str) -> OutboxReader {
        OutboxReader::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout: Duration::from_millis(1500),
            cooldown: Duration::ZERO,
        })
    }

    #[test]
    fn a_marker_key_is_told_from_a_delivery_key() {
        assert!(!is_marker(&key()));
        let mut m = [MARK; KEY_BYTES];
        m[16..32].copy_from_slice(&5u128.to_be_bytes());
        assert!(is_marker(&m));
    }

    #[tokio::test]
    async fn enter_marker_sends_one_valid_enter_and_returns_its_key() {
        let mut r = reader("echo ready; read -r a; printf '+7\\nentered'");
        let k = r.enter_marker().await.unwrap();
        assert!(is_marker(&k));
    }

    #[test]
    fn lines_have_the_wire_shape_and_clamp() {
        assert_eq!(claim_line(4, 60000), "claim\t4\t60000\n");
        assert_eq!(claim_line(0, 1), "claim\t1\t1\n");
        assert_eq!(claim_line(500, 1), format!("claim\t{CLAIM_MAX}\t1\n"));
        assert_eq!(
            complete_line(&key(), 12),
            format!("complete\t{}{}\t12\n", LEDGER, "09".repeat(16))
        );
    }

    #[test]
    fn a_claim_body_parses_and_splits_its_key() {
        let row = format!("{}{}\t2\t31\t7b2261223a317d", LEDGER, "09".repeat(16));
        let got = parse_claim(&format!("{row}\n{row}\n")).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].attempts, 2);
        assert_eq!(got[0].epoch, 31);
        assert_eq!(got[0].payload, b"{\"a\":1}");
        assert_eq!(got[0].ledger_hex(), LEDGER);
        assert_eq!(got[0].webhook(), [9u8; 16]);
        assert!(parse_claim("").unwrap().is_empty());
        for bad in ["zz\t0\t0\t", "ab\t0\t0\t", &format!("{LEDGER}\t0\t0\t00")] {
            assert!(parse_claim(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn acks_parse_by_name() {
        assert_eq!(parse_ack("ack: ok\n").unwrap(), Ack::Ok);
        assert_eq!(
            parse_ack("ack: already_terminal\n").unwrap(),
            Ack::AlreadyTerminal
        );
        assert_eq!(parse_ack("ack: unknown").unwrap(), Ack::Unknown);
        assert_eq!(parse_ack("ack: lease_lost").unwrap(), Ack::LeaseLost);
        assert!(parse_ack("nope").is_err());
    }

    #[tokio::test]
    async fn claims_then_completes_over_one_held_child() {
        let row = format!("{}{}\t0\t5\t7b7d", LEDGER, "09".repeat(16));
        let script = format!(
            "echo ready; read -r a; printf '+{}\\n%s\\n' '{row}'; read -r b; \
             case \"$b\" in complete*5) printf '+8\\nack: ok\\n';; *) printf '+13\\nack: unknown\\n';; esac",
            row.len() + 1
        );
        let mut r = reader(&script);
        let got = r.claim(4, 60_000).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].epoch, 5);
        assert_eq!(r.complete(&got[0]).await.unwrap(), Ack::Ok);
    }

    #[tokio::test]
    async fn a_reused_pipe_that_died_is_reopened_for_claim_and_complete_with_no_cooldown() {
        // Each child answers one request and exits, as a pipe past its grant TTL does.
        let mut r = OutboxReader::new(Config {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "echo ready; read -r a; case \"$a\" in claim*) printf '+0\\n';; \
                 *) printf '+8\\nack: ok\\n';; esac"
                    .into(),
            ],
            timeout: Duration::from_millis(1500),
            cooldown: Duration::from_secs(3600),
        });
        assert!(r.claim(4, 1).await.unwrap().is_empty());
        // The held pipe is dead now: the claim goes over a fresh one instead of failing.
        assert!(r.claim(4, 1).await.unwrap().is_empty());
        let c = Claimed {
            key: key(),
            attempts: 0,
            epoch: 1,
            payload: vec![],
        };
        assert_eq!(r.complete(&c).await.unwrap(), Ack::Ok);
    }

    #[test]
    fn retry_and_fail_lines_have_the_wire_shape() {
        let k = format!("{}{}", LEDGER, "09".repeat(16));
        assert_eq!(
            retry_line(&key(), 3, 30_000),
            format!("retry\t{k}\t3\t30000\n")
        );
        assert_eq!(fail_line(&key(), 3), format!("fail\t{k}\t3\n"));
    }

    #[tokio::test]
    async fn retry_and_fail_send_their_verbs_and_parse_the_ack() {
        let script = "echo ready; read -r a; case \"$a\" in retry*3*30000) printf '+8\\nack: ok\\n';; \
                      *) printf '+13\\nack: unknown\\n';; esac; read -r b; \
                      case \"$b\" in fail*3) printf '+15\\nack: lease_lost\\n';; \
                      *) printf '+13\\nack: unknown\\n';; esac";
        let mut r = reader(script);
        let c = Claimed {
            key: key(),
            attempts: 0,
            epoch: 3,
            payload: vec![],
        };
        assert_eq!(r.retry(&c, 30_000).await.unwrap(), Ack::Ok);
        assert_eq!(r.fail(&c).await.unwrap(), Ack::LeaseLost);
    }

    #[tokio::test]
    async fn a_long_coded_refusal_of_an_ack_is_a_refusal_and_keeps_the_child() {
        let body = format!("code=Busy {}", "y".repeat(200));
        let mut r = reader(&format!(
            "echo ready; while IFS= read -r a; do printf -- '-{}\\n%s' '{body}'; done",
            body.len()
        ));
        let c = Claimed {
            key: key(),
            attempts: 0,
            epoch: 1,
            payload: Vec::new(),
        };
        for _ in 0..2 {
            assert!(matches!(r.complete(&c).await, Err(MirrorError::Refused(_))));
        }
    }

    #[tokio::test]
    async fn an_expired_grant_on_a_reused_pipe_is_replayed_for_an_ack() {
        let body = "code=CapabilityTokenExpired refused: DefiniteRefusal CapabilityTokenExpired";
        // Each child acks its first line and refuses its second as session-over.
        let script = format!(
            "echo ready; n=0; while IFS= read -r l; do n=$((n+1)); \
             if [ $n -eq 2 ]; then printf -- '-{}\\n%s' '{body}'; else printf '+8\\nack: ok\\n'; fi; done",
            body.len()
        );
        let mut r = OutboxReader::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout: Duration::from_millis(1500),
            cooldown: Duration::from_secs(3600),
        });
        let c = Claimed {
            key: key(),
            attempts: 0,
            epoch: 1,
            payload: Vec::new(),
        };
        assert_eq!(r.complete(&c).await.unwrap(), Ack::Ok);
        // The second is refused on the reused pipe, then replayed on a new session: no loss.
        assert_eq!(r.complete(&c).await.unwrap(), Ack::Ok);
    }

    #[tokio::test]
    async fn an_empty_claim_is_no_units() {
        let mut r = reader("echo ready; read -r a; printf '+0\\n'");
        assert!(r.claim(4, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused_not_allocated() {
        let mut r = reader("echo ready; read -r a; printf '+99999999\\n'");
        assert!(matches!(r.claim(1, 1).await, Err(MirrorError::BadFrame(_))));
    }
}
