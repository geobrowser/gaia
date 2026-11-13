"""Alternate scoring data provider reading members, editors and user_votes from geo-db (a twin, off by default).

Postgres stays the source of truth. :class:`GeoDbScoringDataProvider` is a :class:`ScoringDataProvider` whose
``_fetch_users`` and ``_fetch_votes`` read geo-db instead of the ``members``, ``editors`` and ``user_votes`` tables; spaces,
entities and perspectives still come from Postgres. It returns the same ``User`` and ``Vote`` objects, so one fixture can
be given to both providers and the rows compared (``tests/test_geodb_reads.py``).

:func:`compare_geodb_reads` is what the cronjob runs when ``GEODB_SCORING_READS`` is ``1`` or ``true``: it reads the same
rows from geo-db, compares them with what Postgres gave, counts every difference in :class:`ReadStats` and logs counts only.
Its result is never used for scoring, it never raises, and it is bounded by ``GEODB_SCORING_READS_BUDGET_S``.

Where the rows live (geodb-feeder, plan 10.12 and 10.11 GAP-7):

- Membership: in the ``governance`` system Space, a ``member`` or ``editor`` reference on each Space's own entity
  (``cid("entity", space)``), the value a reference to ``cid("entity", member_space_id)``. One card read per batch of 32
  Spaces lists them all; a revoked grant is inactive and not listed.
- Votes: in the ``votes`` system Space, one record per (user, object, object type, space, kind) with the columns of
  ``USER_VOTE_COLUMNS`` (api/src/geodb/keyedRecordsShadow.ts). geo-db has no scan over a record kind, so the records are
  found from the voter: the backlinks of the user's entity, then the records' cards. The voters read are the users that
  hold a membership row (and any ids the caller adds); a voter that holds none is not read. Postgres votes by anyone else
  are counted apart (``votes_outside_voters``), not as a mismatch.

Ids: geo-db holds a v4 id as itself and hashes anything else (``conforming_id``), and a hash is itself a v4, so a hashed
id cannot be told from a real one by looking at it. The ids Postgres lists (the Space ids) are mapped back; a hashed id
nothing maps back is read as it is, so it shows as a row Postgres does not hold, counted in ``mismatch``. ``unmapped`` counts
only values that are not an id at all.

Serving (``GEODB_SCORES_SERVE``, off by default, a sibling of the compare flag because it reads the derived score
collections, not members and votes): the scoring writer diffs the new scores against the stored ones to find the rows the
geo-db mirror must resend. With the flag on that diff reads the stored rows from geo-db's collections 2, 3 and 4 (each group read
in bounded pages by the ``scan`` resume cursor of a ``geodb-client derived pipe``, which needs a client with that field)
instead of Postgres. Any error or timeout falls back to the Postgres diff and is
counted in ``gaia_scoring_scores_serve_fallback_total{reason}`` (``error``, ``timeout``, ``misconfigured``). See
:class:`ScoresServe`.

Needs ``GEODB_KEY_PATH`` (or ``GEODB_READ_KEY_PATH``, which wins when set), ``GEODB_BINDING_PATH`` and ``GEODB_SYSTEM_SPACES_PATH`` (the feeder's ``system-spaces.json``,
``{"<family>": {"geo": "<32 hex>"}}``), and the key's binding must allow card reads in both system Spaces.
"""

import hashlib
import json
import logging
import math
import os
import re
import select
import subprocess
import time
import uuid
from collections import Counter
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Protocol

import psycopg

from src.algorithm.models import User, Vote, VoteType
from src.scoring_data_provider.scoring_data_provider import ScoringData, ScoringDataProvider

logger = logging.getLogger(__name__)

_UUID_V4 = re.compile(r"^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$")
_HEX32 = re.compile(r"^[0-9a-f]{32}$")

# Pinned like the feeder's copies (geodb-feeder/src/membership.ts ROLE_PROPERTY, recordKinds.ts) and the api's
# (api/src/geodb/keyedRecordsShadow.ts USER_VOTE_COLUMNS); a test holds them to the values below.
ROLE_MEMBER = "dbe3bdf637a5458d9206d737fb104793"
ROLE_EDITOR = "79c3a498486641bc930f3b973de4b4d3"
VOTE_USER = "4bd6d3068a204ff6a62bb084263045bc"
VOTE_OBJECT = "46e83cb03e0c46a4a874d7deeb5889dd"
VOTE_OBJECT_TYPE = "e2f5df668d07476ba3f1721c885bd53e"
VOTE_SPACE = "59217d4ba84c4fdcbdc3ff6fb3cb83fa"
VOTE_KIND = "4f65ca0db5874a519d8068671a0222c5"
VOTE_DIRECTION = "8f3588d7a11c4e7eb86b61015670b491"
VOTE_VOTED_AT = "6e088d8604924b05b09e3ebeceebf5db"

CARD_IDS_MAX = 32
BACKLINKS_LIMIT = 4096
DEFAULT_BUDGET_S = 600.0
DEFAULT_READ_TIMEOUT_S = 30.0


def compact(value: object) -> str:
    return str(value).replace("-", "").lower()


def conforming_id(kind: str, legacy_id: str) -> str:
    """The id geo-db holds for a gaia id: a v4 as itself, anything else hashed (geodb-feeder/src/ids.ts)."""
    cid = compact(legacy_id)
    if _UUID_V4.match(cid):
        return cid
    digest = bytearray(hashlib.sha256(f"geo:legacy-{kind}:v1\0{cid}".encode()).digest()[:16])
    digest[6] = (digest[6] & 0x0F) | 0x40
    digest[8] = (digest[8] & 0x3F) | 0x80
    return digest.hex()


def dashed(hex32: str) -> str:
    """The form psycopg gives a uuid column (``str(uuid.UUID)``)."""
    return str(uuid.UUID(hex32))


@dataclass
class Row:
    """One card row: the property, the value as geo-db prints it, and the entity owning the row."""

    prop: str
    raw: str
    entity: str


class GeoDbReadError(Exception):
    """A read geo-db refused, or a client that broke its framing, timed out or died."""


class GeoDbTimeout(GeoDbReadError):
    """The client did not answer within the read's time limit."""


class GeoDbReader(Protocol):
    def cards(self, space: str, entity_ids: list[str]) -> list[Row]:
        """The active rows of 1..32 entities in one Space."""

    def backlinks(self, space: str, entity_id: str, limit: int) -> list[Row]:
        """Up to ``limit`` rows referring to ``entity_id``; ``entity`` is the referring entity."""

    def close(self) -> None: ...


def _unescape(text: str) -> str:
    return re.sub(r"\\(t|n|\\)", lambda m: {"t": "\t", "n": "\n", "\\": "\\"}[m.group(1)], text)


def card_document(entity_id: str, backlinks: int = 0) -> str:
    extra = f", .backlinks = {backlinks}" if backlinks else ""
    return f'.{{ .version = 1, .select = .{{ .c = .{{ .field = "entity", .args = .{{ .id = "{entity_id}"{extra} }} }} }} }}'


def cards_document(entity_ids: list[str]) -> str:
    ids = ", ".join(f'"{i}"' for i in entity_ids)
    return f'.{{ .version = 1, .select = .{{ .c = .{{ .field = "entity", .args = .{{ .ids = .{{ {ids} }} }} }} }} }}'


def parse_rows(payload: str, only_entity: str | None = None, backlinks_of: str | None = None) -> list[Row]:
    """Rows of a card answer: cells (property, value, statement, entity).

    ``backlinks_of`` keeps only the rows whose value is that entity (a backlink read also returns the entity's own card).
    """
    out: list[Row] = []
    for line in payload.split("\n"):
        if not line:
            continue
        cells = line.split("\t")
        if len(cells) < 4 or not _HEX32.match(cells[3]):
            raise GeoDbReadError(f"a card row has no entity cell: {line[:60]}")
        if only_entity is not None and cells[3] != only_entity:
            continue
        if backlinks_of is not None and cells[1] != backlinks_of:
            continue
        out.append(Row(prop=cells[0], raw=_unescape(cells[1]), entity=cells[3]))
    return out


class PipeReader:
    """One held ``geodb-client pipe`` child: a request line in, one ``+<n>`` / ``-<n>`` length-prefixed frame out."""

    def __init__(self, argv: list[str], timeout: float = DEFAULT_READ_TIMEOUT_S):
        self._argv = argv
        self._timeout = timeout
        self._proc: subprocess.Popen | None = None
        self._buf = b""

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> "PipeReader | None":
        env = os.environ if environ is None else environ
        # GEODB_READ_KEY_PATH lets the reads use a read-only key while GEODB_KEY_PATH stays the derived mirror's writer key.
        key, binding = env.get("GEODB_READ_KEY_PATH") or env.get("GEODB_KEY_PATH"), env.get("GEODB_BINDING_PATH")
        if not key or not binding:
            logger.warning("geodb_reads.misconfigured: GEODB_READ_KEY_PATH/GEODB_KEY_PATH or GEODB_BINDING_PATH is unset")
            return None
        argv = [env.get("GEODB_CLIENT_BIN") or "geodb-client", "--key", key, "--binding", binding, "pipe"]
        return cls(argv)

    def cards(self, space: str, entity_ids: list[str]) -> list[Row]:
        if not 1 <= len(entity_ids) <= CARD_IDS_MAX:
            raise GeoDbReadError("a card read names 1 to 32 entities")
        return parse_rows(self._request(f"{space}\t{cards_document(entity_ids)}\n"))

    def backlinks(self, space: str, entity_id: str, limit: int) -> list[Row]:
        return parse_rows(self._request(f"{space}\t{card_document(entity_id, limit)}\n"), backlinks_of=entity_id)

    def close(self) -> None:
        proc, self._proc, self._buf = self._proc, None, b""
        if proc is None:
            return
        try:
            proc.kill()
            proc.wait(timeout=5)
        except Exception:  # noqa: BLE001 - closing never raises
            pass

    def _request(self, line: str) -> str:
        deadline = time.monotonic() + self._timeout
        try:
            proc = self._ensure(deadline)
            assert proc.stdin is not None
            proc.stdin.write(line.encode())
            proc.stdin.flush()
            header = self._read_line(proc, deadline)
            m = re.fullmatch(r"([+-])(\d{1,9})", header)
            if not m:
                raise GeoDbReadError("geodb-client broke its framing")
            body = self._read_exact(proc, int(m.group(2)), deadline).decode("utf8", "replace")
            if m.group(1) == "-":
                raise GeoDbReadError(body[:300])
            return body
        except GeoDbReadError:
            self.close()  # a refusal ends the session; the next request starts a fresh one
            raise
        except (OSError, ValueError) as e:
            self.close()
            raise GeoDbReadError(str(e)[:300]) from e

    def _ensure(self, deadline: float) -> subprocess.Popen:
        if self._proc is not None and self._proc.poll() is None:
            return self._proc
        self.close()
        proc = subprocess.Popen(self._argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self._proc = proc
        first = self._read_line(proc, deadline)
        if first != "ready":
            raise GeoDbReadError("geodb-client did not start")
        return proc

    def _fill(self, proc: subprocess.Popen, deadline: float) -> None:
        assert proc.stdout is not None
        wait = deadline - time.monotonic()
        if wait <= 0 or not select.select([proc.stdout], [], [], wait)[0]:
            raise GeoDbTimeout("geodb-client timed out")
        chunk = os.read(proc.stdout.fileno(), 65536)
        if not chunk:
            raise GeoDbReadError("geodb-client closed")
        self._buf += chunk

    def _read_line(self, proc: subprocess.Popen, deadline: float) -> str:
        while b"\n" not in self._buf:
            self._fill(proc, deadline)
        line, self._buf = self._buf.split(b"\n", 1)
        return line.decode("utf8", "replace")

    def _read_exact(self, proc: subprocess.Popen, size: int, deadline: float) -> bytes:
        while len(self._buf) < size:
            self._fill(proc, deadline)
        data, self._buf = self._buf[:size], self._buf[size:]
        return data


def system_spaces_from_env(environ: Mapping[str, str] | None = None) -> dict[str, str]:
    """``{"governance": <32 hex>, "votes": <32 hex>}`` from ``GEODB_SYSTEM_SPACES_PATH``; empty when unset or unreadable."""
    env = os.environ if environ is None else environ
    path = env.get("GEODB_SYSTEM_SPACES_PATH", "").strip()
    if not path:
        return {}
    try:
        with open(path, encoding="utf8") as f:
            raw = json.load(f)
        out = {}
        for family, entry in raw.items():
            geo = compact((entry or {}).get("geo", ""))
            if _HEX32.match(geo):
                out[family] = geo
        return out
    except (OSError, ValueError, AttributeError):
        logger.warning("geodb_reads.misconfigured: GEODB_SYSTEM_SPACES_PATH is unreadable")
        return {}


def enabled(environ: Mapping[str, str] | None = None) -> bool:
    env = os.environ if environ is None else environ
    return env.get("GEODB_SCORING_READS", "").strip().lower() in ("1", "true")


@dataclass
class ReadStats:
    """Counts only. ``mismatch`` is the number of rows (or users, for votes) that differ between the two providers."""

    members_geodb: int = 0
    editors_geodb: int = 0
    votes_geodb: int = 0
    members_postgres: int = 0
    editors_postgres: int = 0
    votes_postgres: int = 0
    mismatch: Counter = field(default_factory=Counter)  # by kind: members, editors, votes
    unmapped: int = 0
    truncated_voters: int = 0
    bad_records: int = 0
    votes_outside_voters: int = 0
    failed: str = ""

    @property
    def total_mismatch(self) -> int:
        return sum(self.mismatch.values())

    def as_log(self) -> dict[str, object]:
        return {
            "geodb_reads_members": self.members_geodb,
            "geodb_reads_editors": self.editors_geodb,
            "geodb_reads_votes": self.votes_geodb,
            "geodb_reads_mismatch": self.total_mismatch,
            "geodb_reads_mismatch_members": self.mismatch["members"],
            "geodb_reads_mismatch_editors": self.mismatch["editors"],
            "geodb_reads_mismatch_votes": self.mismatch["votes"],
            "geodb_reads_unmapped": self.unmapped,
            "geodb_reads_truncated_voters": self.truncated_voters,
            "geodb_reads_bad_records": self.bad_records,
            "geodb_reads_votes_outside_voters": self.votes_outside_voters,
            "geodb_reads_failed": self.failed,
        }


class BudgetExceeded(Exception):
    pass


def _chunks(items: list[str], n: int) -> Iterable[list[str]]:
    for i in range(0, len(items), n):
        yield items[i : i + n]


class GeoDbScoringDataProvider(ScoringDataProvider):
    """The Postgres provider with users and votes read from geo-db; fetch_all, spaces and entities are inherited."""

    def __init__(
        self,
        connection_string: str,
        reader: GeoDbReader,
        governance_space: str,
        votes_space: str,
        extra_voters: Iterable[str] = (),
        stats: ReadStats | None = None,
        deadline: float | None = None,
        clock: Callable[[], float] = time.monotonic,
    ):
        super().__init__(connection_string)
        self._reader = reader
        self._governance = governance_space
        self._votes_space = votes_space
        self._extra_voters = [compact(v) for v in extra_voters]
        self.stats = stats if stats is not None else ReadStats()
        self._deadline = deadline
        self._clock = clock
        self.voters: set[str] = set()

    def _check_budget(self) -> None:
        if self._deadline is not None and self._clock() > self._deadline:
            raise BudgetExceeded()

    def _space_ids(self, conn: psycopg.Connection) -> list[str]:
        with conn.cursor() as cur:
            cur.execute("SELECT id FROM spaces")
            return [compact(r[0]) for r in cur.fetchall()]

    def _fetch_users(self, conn: psycopg.Connection) -> list[User]:
        spaces = self._space_ids(conn)
        back = {conforming_id("entity", s): s for s in spaces}
        subjects = {conforming_id("entity", s): s for s in spaces}
        members: dict[str, set[str]] = {}
        editors: dict[str, set[str]] = {}
        for batch in _chunks(sorted(subjects), CARD_IDS_MAX):
            self._check_budget()
            for row in self._reader.cards(self._governance, batch):
                bucket = members if row.prop == ROLE_MEMBER else editors if row.prop == ROLE_EDITOR else None
                if bucket is None or row.entity not in subjects:
                    continue
                target = back.get(row.raw) or (row.raw if _UUID_V4.match(row.raw) else None)
                if target is None:
                    self.stats.unmapped += 1
                    continue
                bucket.setdefault(dashed(target), set()).add(dashed(subjects[row.entity]))
        self.stats.members_geodb = sum(len(v) for v in members.values())
        self.stats.editors_geodb = sum(len(v) for v in editors.values())
        ids = set(members) | set(editors)
        self.voters = {compact(i) for i in ids} | set(self._extra_voters)
        return [User(id=i, member_spaces=members.get(i, set()), editor_spaces=editors.get(i, set())) for i in ids]

    def _fetch_votes(self, conn: psycopg.Connection) -> list[Vote]:
        spaces = {conforming_id("entity", s): s for s in self._space_ids(conn)}
        votes: list[Vote] = []
        for voter in sorted(self.voters):
            self._check_budget()
            user_ref = conforming_id("entity", voter)
            links = self._reader.backlinks(self._votes_space, user_ref, BACKLINKS_LIMIT)
            records = sorted({r.entity for r in links if r.prop == VOTE_USER})
            if len(links) >= BACKLINKS_LIMIT:
                self.stats.truncated_voters += 1
            by_record: dict[str, dict[str, str]] = {}
            for batch in _chunks(records, CARD_IDS_MAX):
                self._check_budget()
                for row in self._reader.cards(self._votes_space, batch):
                    by_record.setdefault(row.entity, {})[row.prop] = row.raw
            for record in records:
                vote = self._decode_vote(by_record.get(record, {}), dashed(voter), spaces)
                if vote is not None:
                    votes.append(vote)
        self.stats.votes_geodb = len(votes)
        return votes

    def _decode_vote(self, cols: dict[str, str], user: str, spaces: dict[str, str]) -> Vote | None:
        """One keyed record as the Postgres query reads it: object_type 0, vote_kind <> 3, direction 0 or 1."""
        try:
            if int(cols[VOTE_OBJECT_TYPE]) != 0 or int(cols[VOTE_KIND]) == 3:
                return None
            direction = int(cols[VOTE_DIRECTION])
            if direction not in (0, 1):
                return None
            obj = cols[VOTE_OBJECT]
            space = spaces.get(cols[VOTE_SPACE]) or (cols[VOTE_SPACE] if _UUID_V4.match(cols[VOTE_SPACE]) else None)
            if not _UUID_V4.match(obj) or space is None:
                self.stats.unmapped += 1
                return None
            m = re.fullmatch(r"instant:(-?\d+)\.(\d{9})", cols[VOTE_VOTED_AT])
            if m is None:
                raise ValueError("voted_at")
            at = datetime.fromtimestamp(int(m.group(1)), tz=timezone.utc).replace(microsecond=int(m.group(2)) // 1000)
        except (KeyError, ValueError):
            self.stats.bad_records += 1
            return None
        return Vote(
            user_id=user,
            entity_id=dashed(obj),
            space_id=dashed(space),
            vote_type=VoteType.UPVOTE if direction == 0 else VoteType.DOWNVOTE,
            timestamp=at,
        )


def _user_sets(users: list[User]) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    members = {u.id: set(u.member_spaces) for u in users if u.member_spaces}
    editors = {u.id: set(u.editor_spaces) for u in users if u.editor_spaces}
    return members, editors


def _vote_keys(votes: Iterable[Vote]) -> set[tuple]:
    # The timestamp is part of the row: a vote geo-db holds at another time is a different row.
    return {(v.user_id, v.entity_id, v.space_id, v.vote_type, v.timestamp) for v in votes}


def count_differences(postgres: ScoringData, geodb_users: list[User], geodb_votes: list[Vote], voters: set[str], stats: ReadStats) -> None:
    """Fill the stats with the postgres row counts and the number of rows each side holds that the other does not."""
    pm, pe = _user_sets(postgres.users)
    gm, ge = _user_sets(geodb_users)
    stats.members_postgres = sum(len(v) for v in pm.values())
    stats.editors_postgres = sum(len(v) for v in pe.values())
    for kind, left, right in (("members", pm, gm), ("editors", pe, ge)):
        for user in set(left) | set(right):
            stats.mismatch[kind] += len(left.get(user, set()) ^ right.get(user, set()))
    inside = [v for v in postgres.votes if compact(v.user_id) in voters]
    stats.votes_outside_voters = len(postgres.votes) - len(inside)
    stats.votes_postgres = len(inside)
    stats.mismatch["votes"] += len(_vote_keys(inside) ^ _vote_keys(geodb_votes))


def compare_geodb_reads(
    database_url: str,
    postgres: ScoringData,
    reader: GeoDbReader | None = None,
    spaces: Mapping[str, str] | None = None,
    environ: Mapping[str, str] | None = None,
    provider_factory: Callable[..., GeoDbScoringDataProvider] = GeoDbScoringDataProvider,
) -> ReadStats:
    """Read the same rows from geo-db, count how they differ from ``postgres``, log counts only. Never raises."""
    env = os.environ if environ is None else environ
    stats = ReadStats()
    owned = reader is None
    try:
        system = dict(spaces) if spaces is not None else system_spaces_from_env(env)
        if "governance" not in system or "votes" not in system:
            stats.failed = "no_system_spaces"
            return stats
        reader = reader or PipeReader.from_env(env)
        if reader is None:
            stats.failed = "no_client"
            return stats
        try:
            budget = float(env.get("GEODB_SCORING_READS_BUDGET_S", "") or DEFAULT_BUDGET_S)
        except ValueError:
            budget = DEFAULT_BUDGET_S
        provider = provider_factory(
            database_url, reader, system["governance"], system["votes"], stats=stats, deadline=time.monotonic() + budget
        )
        with psycopg.connect(database_url) as conn:
            users = provider._fetch_users(conn)
            votes = provider._fetch_votes(conn)
        count_differences(postgres, users, votes, provider.voters, stats)
    except BudgetExceeded:
        stats.failed = "budget"
    except GeoDbReadError as e:
        stats.failed = "read_failed"
        logger.warning("geodb_reads.failed: %s", str(e)[:120])
    except Exception as e:  # noqa: BLE001 - a twin never takes the run down
        stats.failed = type(e).__name__
        logger.warning("geodb_reads.failed: %s", type(e).__name__)
    finally:
        if owned and reader is not None:
            reader.close()
        logger.log(
            logging.WARNING if (stats.total_mismatch or stats.failed) else logging.INFO,
            "geodb_reads: mismatch=%d failed=%s",
            stats.total_mismatch,
            stats.failed or "-",
            extra=stats.as_log(),
        )
    return stats


# ---------------------------------------------------------------------------------------------------------------------
# Serve path: the stored scores, read from geo-db's derived collections 2 (global), 3 (local) and 4 (space).
# ---------------------------------------------------------------------------------------------------------------------

COLLECTION_GLOBAL = 2
COLLECTION_LOCAL = 3
COLLECTION_SPACE = 4
DEFAULT_SERVE_BUDGET_S = 600.0
# The serve mints each session's grant for 300 s (geodb/supervisor.sh); replace the child well before that, as the
# derived mirror does (GEODB_DERIVED_SESSION_REFRESH_S) and the api's derived reader does.
SERVE_SESSION_REFRESH_S = 240.0
SCORE_TOLERANCE = 1e-9
FALLBACK_REASONS = ("error", "timeout", "misconfigured")
SERVE_TABLES = ("global_scores", "local_scores", "space_scores")
_ZERO_GROUP = "0" * 32

FALLBACK_METRIC = "gaia_scoring_scores_serve_fallback_total"


def serve_enabled(environ: Mapping[str, str] | None = None) -> bool:
    """``GEODB_SCORES_SERVE``: off unless ``1`` or ``true``."""
    env = os.environ if environ is None else environ
    return env.get("GEODB_SCORES_SERVE", "").strip().lower() in ("1", "true")


class ServeFallback(Exception):
    """The geo-db read cannot answer; ``reason`` is the counter's label."""

    def __init__(self, reason: str, detail: str = ""):
        super().__init__(f"{reason}: {detail}" if detail else reason)
        self.reason = reason


@dataclass
class ServeStats:
    """Counts for one run. ``fallback`` is the counter ``gaia_scoring_scores_serve_fallback_total{reason}``."""

    fallback: Counter = field(default_factory=Counter)
    served: Counter = field(default_factory=Counter)  # by table: reads that answered from geo-db
    rows_read: int = 0

    def render(self) -> str:
        """The counter in the Prometheus text format, every reason present at zero. The CronJob has no scrape endpoint,
        so this goes to the log (one line per series) where a log-based metric can read it."""
        lines = [f"# TYPE {FALLBACK_METRIC} counter"]
        lines += [f'{FALLBACK_METRIC}{{reason="{r}"}} {self.fallback.get(r, 0)}' for r in FALLBACK_REASONS]
        return "\n".join(lines)

    def as_log(self) -> dict[str, object]:
        return {
            "geodb_scores_serve_fallback_error": self.fallback["error"],
            "geodb_scores_serve_fallback_timeout": self.fallback["timeout"],
            "geodb_scores_serve_fallback_misconfigured": self.fallback["misconfigured"],
            "geodb_scores_serve_served_global": self.served["global_scores"],
            "geodb_scores_serve_served_local": self.served["local_scores"],
            "geodb_scores_serve_rows_read": self.rows_read,
        }


@dataclass(frozen=True)
class StoredScore:
    """What a ``get`` answers: the row's group (32 hex), score and value (hex)."""

    group: str
    score: float
    value: str


@dataclass(frozen=True)
class ScanPage:
    """One page of a group scan: ``(score, key hex, value hex)`` rows, and the cursor to resume from (``None`` at the end)."""

    rows: list[tuple[float, str, str]]
    next: str | None


SCAN_PAGE_ROWS = 512  # geo-db's page_max: the most rows one DerivedScan frame answers


class DerivedGetReader(PipeReader):
    """A held ``geodb-client derived pipe`` child answering ``get`` (``none`` or ``<group hex>\t<score>\t<value hex>``)."""

    def __init__(self, argv: list[str], timeout: float = DEFAULT_READ_TIMEOUT_S, refresh: float = SERVE_SESSION_REFRESH_S,
                 clock: Callable[[], float] = time.monotonic):
        super().__init__(argv, timeout)
        self._refresh = refresh
        self._clock = clock
        self._opened_at = 0.0

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> "DerivedGetReader | None":
        env = os.environ if environ is None else environ
        key, binding = env.get("GEODB_READ_KEY_PATH") or env.get("GEODB_KEY_PATH"), env.get("GEODB_BINDING_PATH")
        if not key or not binding:
            logger.warning("geodb_reads.misconfigured: GEODB_READ_KEY_PATH/GEODB_KEY_PATH or GEODB_BINDING_PATH is unset")
            return None
        return cls([env.get("GEODB_CLIENT_BIN") or "geodb-client", "--key", key, "--binding", binding, "derived", "pipe"])

    def _ensure(self, deadline: float) -> subprocess.Popen:
        if self._proc is not None and self._clock() - self._opened_at > self._refresh:
            self.close()  # a fresh handshake and grant before the old one expires
        fresh = self._proc is None or self._proc.poll() is not None
        proc = super()._ensure(deadline)
        if fresh:
            self._opened_at = self._clock()
        return proc

    def get(self, collection: int, key_hex: str) -> StoredScore | None:
        body = self._request(f"get\t{collection}\thex:{key_hex}\n").rstrip("\n")
        if body == "none":
            return None
        parts = body.split("\t")
        if len(parts) != 3 or not _HEX32.match(parts[0]) or not re.fullmatch(r"([0-9a-f]{2})*", parts[2]):
            raise GeoDbReadError(f"derived get answer is malformed: {body[:60]}")
        try:
            score = float(parts[1])
        except ValueError as e:
            raise GeoDbReadError("derived get score is not a number") from e
        if not math.isfinite(score):
            raise GeoDbReadError("derived get score is not finite")
        return StoredScore(group=parts[0], score=score, value=parts[2])

    def scan(self, collection: int, group_hex: str, after: str | None) -> ScanPage:
        """One page of a group in geo-db's order, resuming past ``after`` (the ``more<TAB><hex>`` cursor the last page
        returned). Needs a ``geodb-client`` that has the scan resume field; an older one answers the line as malformed."""
        body = self._request(f"scan\t{collection}\t0x{group_hex}\t{SCAN_PAGE_ROWS}\t1\t{after or '-'}\n")
        rows: list[tuple[float, str, str]] = []
        cursor: str | None = None
        for line in body.split("\n"):
            if not line:
                continue
            if line.startswith("more\t"):
                cursor = line[5:]
                if not re.fullmatch(r"([0-9a-f]{2})+", cursor):
                    raise GeoDbReadError("derived scan cursor is malformed")
                continue
            parts = line.split("\t")
            if len(parts) != 3 or not re.fullmatch(r"([0-9a-f]{2})+", parts[1]) or not re.fullmatch(r"([0-9a-f]{2})*", parts[2]):
                raise GeoDbReadError(f"derived scan row is malformed: {line[:60]}")
            try:
                score = float(parts[0])
            except ValueError as e:
                raise GeoDbReadError("derived scan score is not a number") from e
            if not math.isfinite(score):
                raise GeoDbReadError("derived scan score is not finite")
            rows.append((score, parts[1], parts[2]))
        return ScanPage(rows, cursor)


class ScoreReader(Protocol):
    def get(self, collection: int, key_hex: str) -> StoredScore | None: ...

    def scan(self, collection: int, group_hex: str, after: str | None) -> ScanPage: ...

    def close(self) -> None: ...


def _same(stored: StoredScore | None, group: str, score: float) -> bool:
    """Whether geo-db holds this row as the writer would write it: the group, no value, the score within tolerance."""
    if stored is None or stored.value or stored.group != group:
        return False
    return abs(stored.score - score) <= SCORE_TOLERANCE * max(1.0, abs(score))


class ScoresServe:
    """Answers the writer's "which stored score rows differ from these new ones" from geo-db, or says it cannot.

    ``changed_keys`` returns the same shape as the Postgres diff (``ScoringDataWriter._changed_keys``): a set of normalized
    entity ids for ``global_scores``, of ``(entity id, space id)`` for ``local_scores``. It returns ``None`` on any error or
    timeout, after counting it, and the caller then runs the Postgres diff. It never raises.
    """

    def __init__(self, reader: ScoreReader | None, budget_s: float = DEFAULT_SERVE_BUDGET_S,
                 clock: Callable[[], float] = time.monotonic, stats: ServeStats | None = None):
        self._reader = reader
        self._budget = budget_s
        self._clock = clock
        self.stats = stats if stats is not None else ServeStats()

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> "ScoresServe | None":
        """``None`` unless ``GEODB_SCORES_SERVE`` is on. On with no usable client the serve is built without a reader and
        every diff counts ``misconfigured`` and uses Postgres."""
        env = os.environ if environ is None else environ
        if not serve_enabled(env):
            return None
        try:
            budget = float(env.get("GEODB_SCORES_SERVE_BUDGET_S", "") or DEFAULT_SERVE_BUDGET_S)
        except ValueError:
            budget = DEFAULT_SERVE_BUDGET_S
        return cls(DerivedGetReader.from_env(env), budget_s=budget)

    def changed_keys(self, table: str, entities: list) -> set | None:
        try:
            if self._reader is None:
                raise ServeFallback("misconfigured", "no geodb-client key or binding")
            result = self._diff(table, entities, self._clock() + self._budget)
        except ServeFallback as e:
            return self._fell_back(table, e.reason, str(e))
        except GeoDbTimeout as e:
            return self._fell_back(table, "timeout", str(e))
        except Exception as e:  # noqa: BLE001 - serving never takes the run down; Postgres answers
            return self._fell_back(table, "error", type(e).__name__ + ": " + str(e)[:120])
        self.stats.served[table] += 1
        logger.info("geodb_scores_serve served table=%s changed=%d", table, len(result), extra=self.stats.as_log())
        return result

    def close(self) -> None:
        if self._reader is not None:
            self._reader.close()

    def _fell_back(self, table: str, reason: str, detail: str) -> None:
        self.stats.fallback[reason] += 1
        logger.warning("geodb_scores_serve fallback table=%s reason=%s detail=%s", table, reason, detail[:160],
                       extra=self.stats.as_log())
        logger.warning("%s", self.stats.render())
        # A held client that failed mid-read is killed by PipeReader; close here as well so the next table starts clean.
        if self._reader is not None:
            self._reader.close()
        return None

    def _scan_group(self, collection: int, group_hex: str, deadline: float) -> dict[str, StoredScore]:
        """Every row of one group, by key hex, read in bounded pages by the resume cursor: a few thousand calls for a
        million rows instead of one ``get`` a row. Raises ``ServeFallback`` past the budget."""
        reader = self._reader
        assert reader is not None
        rows: dict[str, StoredScore] = {}
        after: str | None = None
        while True:
            if self._clock() > deadline:
                raise ServeFallback("timeout", "budget exceeded")
            page = reader.scan(collection, group_hex, after)
            self.stats.rows_read += len(page.rows)
            for score, key_hex, value_hex in page.rows:
                rows[key_hex] = StoredScore(group=group_hex, score=score, value=value_hex)
            if page.next is None:
                return rows
            if page.next == after:
                raise ServeFallback("error", "scan cursor did not advance")
            after = page.next

    def _diff(self, table: str, entities: list, deadline: float) -> set:
        assert self._reader is not None
        out: set = set()

        if table == "global_scores":
            stored = self._scan_group(COLLECTION_GLOBAL, _ZERO_GROUP, deadline)
            for e in entities:
                ekey = _id_bytes(e.id)
                if ekey is not None and not _same(stored.get(ekey.hex()), _ZERO_GROUP, float(e.normalized_score)):
                    out.add(str(uuid.UUID(bytes=ekey)))
            return out

        # local_scores: the group is the Space id (a u128, 32 hex digits), so scan each Space once and compare its rows.
        by_space: dict[str, list[tuple[bytes, bytes, float]]] = {}
        for e in entities:
            for p in e.perspectives:
                pe, ps = _id_bytes(p.entity_id), _id_bytes(p.space_id)
                if pe is None or ps is None:
                    continue  # the mirror skips a row whose id is not a UUID, so there is nothing to diff
                by_space.setdefault(ps.hex(), []).append((pe, ps, float(p.normalized_score)))
        for group, wanted in by_space.items():
            stored = self._scan_group(COLLECTION_LOCAL, group, deadline)
            for pe, ps, score in wanted:
                if not _same(stored.get((pe + ps).hex()), group, score):
                    out.add((str(uuid.UUID(bytes=pe)), str(uuid.UUID(bytes=ps))))
        return out


def _id_bytes(value: object) -> bytes | None:
    try:
        return uuid.UUID(str(value)).bytes
    except (ValueError, TypeError, AttributeError):
        return None
