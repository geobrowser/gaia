//! The reminder `replay_edit` prints when it finishes: it writes Postgres only, so geo-db has not
//! got the edit until the paired `geodb-replay-edit` Job runs (geodb/RUNBOOK.md section 2b-quater).
//! Edit 6a1398bd (version key 125690:0) was replayed here with no twin, and 45 entities of one
//! Space never reached geo-db.

/// What the paired geo-db replay needs, as `replay_edit` knows it.
pub struct TwinArgs<'a> {
    pub uri: &'a str,
    pub space: &'a str,
    pub block: u64,
    pub sequence: u32,
    pub created_at: u64,
    pub edit_id: &'a str,
}

/// The banner printed at the end of every `replay_edit` run.
pub fn twin_notice(a: &TwinArgs<'_>) -> String {
    format!(
        "\n\
=====================================================================\n\
POSTGRES ONLY. geo-db has NOT received edit {edit}.\n\
This tool writes Postgres and nothing else. Run the paired geo-db replay\n\
(geodb/RUNBOOK.md section 2b-quater), dry run first:\n\
\n\
  geodb-replay-edit --uri {uri} --space {space} --block {block} \\\n\
    --sequence {seq} --created-at {at} --edit {edit} --dry-run\n\
  (then the same arguments with --apply, feeder scaled to 0, only after a clean dry run)\n\
\n\
Skipping it leaves the edit's entities in Postgres and missing from geo-db.\n\
Check later: scripts/check-pg-only-edits.py\n\
=====================================================================",
        edit = a.edit_id,
        uri = a.uri,
        space = a.space,
        block = a.block,
        seq = a.sequence,
        at = a.created_at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> TwinArgs<'static> {
        TwinArgs {
            uri: "ipfs://bafyTEST",
            space: "88883e1ec8261b8ac323f564e272b5be",
            block: 125690,
            sequence: 0,
            created_at: 1_763_489_460,
            edit_id: "6a1398bde58f4380adbe6209fa179a3e",
        }
    }

    #[test]
    fn says_postgres_only_and_names_the_twin_job() {
        let n = twin_notice(&args());
        assert!(n.contains("POSTGRES ONLY"), "{n}");
        assert!(n.contains("geodb-replay-edit"), "{n}");
        assert!(n.contains("2b-quater"), "{n}");
    }

    #[test]
    fn prints_the_paired_command_arguments_verbatim() {
        let n = twin_notice(&args());
        for want in [
            "--uri ipfs://bafyTEST",
            "--space 88883e1ec8261b8ac323f564e272b5be",
            "--block 125690",
            "--sequence 0",
            "--created-at 1763489460",
            "--edit 6a1398bde58f4380adbe6209fa179a3e",
            "--dry-run",
        ] {
            assert!(n.contains(want), "missing {want:?} in:\n{n}");
        }
    }

    #[test]
    fn never_prints_apply_as_the_first_step() {
        // A dry run comes first, and only after it is clean is --apply run.
        let n = twin_notice(&args());
        let dry = n.find("--dry-run").expect("dry run");
        let apply = n.find("--apply").expect("apply");
        assert!(dry < apply, "{n}");
    }
}
