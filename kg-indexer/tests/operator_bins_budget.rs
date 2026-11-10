//! Source check: operator binaries must send every statement through a budgeted transaction
//! (`Storage::begin_operator_tx`), never straight at `storage.pool`. Through the shared
//! transaction-mode PgBouncer a pool-level statement runs with no `statement_timeout`.

use std::fs;
use std::path::Path;

#[test]
fn operator_bins_never_touch_the_pool_directly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("read src/bin") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = fs::read_to_string(&path).expect("read bin source");
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains(".pool"),
                "{}:{} reaches the pool outside the budget helper; use storage.begin_operator_tx()",
                path.display(),
                n + 1
            );
            assert!(
                !code.contains(".begin_with_timeout("),
                "{}:{} uses the server budget; operator bins use begin_operator_tx()",
                path.display(),
                n + 1
            );
        }
        checked += 1;
    }
    assert!(checked >= 6, "expected the operator bins, found {checked}");
}
