//! `eval-slot` — the evaluation harness for one embedding slot (design step 8).
//!
//! A golden set is a list of queries. Each names the documents a searcher typing that query
//! wants (`expect`) and, for a contrastive pair, the documents they do not want ranked above
//! those (`reject`): the opposite stance, the negated claim. A query with an empty `expect` is
//! *novel* — nothing in the corpus answers it — and its top-1 score is the unrelated baseline for
//! the floor calibration.
//!
//! The harness embeds every query through the running embedding-service (purpose `query`, and
//! it refuses a vector whose descriptor hash is not the index slot's), runs **the k-NN query the
//! api runs** — same `k`, same per-request `ef_search`, the same non-deleted filter inside the
//! clause — and reports recall@top, MRR, the contrastive outcomes and a floor calibration for the
//! slot. The exit status is the gate: an error when mean recall@top is below `--min-recall`, and
//! with `--gate-contrastive` also when any contrastive pair fails. Stance is deferred by design
//! (D10), so contrastive rows are measured and printed but do not gate by default.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::Args;
use opensearch::{OpenSearch, SearchParts};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::index_meta;
use crate::embedding_slots;
use crate::opensearch_client;

/// Mirrors `api/src/services/search/opensearch.ts` (`SEMANTIC_K_MIN`, `SEMANTIC_EF_SEARCH`). The
/// harness must run the query the api runs, or its numbers describe something else.
const K_MIN: usize = 50;
const EF_SEARCH: u32 = 256;
/// Built into the binary so the Kubernetes job needs no file.
const BUILTIN_GOLDEN: &str = include_str!("../../golden/testnet-debate-claims.json");

/// Evaluate one embedding slot against a golden query set.
#[derive(Args)]
pub struct EvalSlotCommand {
    /// Index version to evaluate (default: the index the alias points to)
    #[arg(short, long, conflicts_with = "index")]
    version: Option<u32>,

    /// Exact index name to act on (instead of --version or the alias)
    #[arg(long)]
    index: Option<String>,

    /// Slot to evaluate (default: the index's default slot)
    #[arg(long)]
    slot: Option<String>,

    /// Running embedding-service, e.g. http://embedding-service:8080
    #[arg(long)]
    embedding_service: String,

    /// Golden set (JSON). Default: the built-in testnet debate-claims set
    #[arg(long)]
    golden: Option<String>,

    /// Gate: fail when mean recall@top over the expected results is below this
    #[arg(long, default_value_t = 0.9)]
    min_recall: f64,

    /// Also fail when any contrastive pair fails (off by default: stance is deferred, D10)
    #[arg(long, default_value_t = false)]
    gate_contrastive: bool,

    /// Write the full report as JSON to this path
    #[arg(long)]
    json: Option<String>,
}

#[derive(Deserialize)]
struct Golden {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    notes: String,
    #[serde(default = "default_top")]
    top: usize,
    #[serde(default)]
    filter: Option<GoldenFilter>,
    queries: Vec<GoldenQuery>,
}

fn default_top() -> usize {
    10
}

#[derive(Deserialize, Default)]
struct GoldenFilter {
    space_id: Option<String>,
    tag_id: Option<String>,
}

#[derive(Deserialize, Clone)]
struct GoldenQuery {
    id: String,
    kind: String,
    text: String,
    #[serde(default)]
    expect: Vec<String>,
    #[serde(default)]
    reject: Vec<String>,
}

#[derive(Serialize, Clone)]
struct Hit {
    rank: usize,
    name: String,
    score: f64,
}

#[derive(Serialize)]
struct NameOutcome {
    name: String,
    rank: Option<usize>,
    score: Option<f64>,
}

#[derive(Serialize)]
struct QueryReport {
    id: String,
    kind: String,
    text: String,
    top1: Option<Hit>,
    expect: Vec<NameOutcome>,
    reject: Vec<NameOutcome>,
    /// Names that do not exist in the index; excluded from every metric and reported.
    missing: Vec<String>,
    /// Present expected names within the top `top`, over present expected names.
    recall: Option<f64>,
    /// 1 / rank of the best expected hit within the top `top`; 0 when none is.
    rr: Option<f64>,
    /// Every present expected name ranks above every reject (a reject outside the k
    /// candidates counts as below).
    contrastive: Option<bool>,
}

#[derive(Serialize, Default)]
struct Stats {
    n: usize,
    min: Option<f64>,
    p10: Option<f64>,
    median: Option<f64>,
    p90: Option<f64>,
    max: Option<f64>,
}

#[derive(Serialize)]
struct FloorReport {
    declared: f64,
    expected_hits: Stats,
    reject_hits: Stats,
    novel_top1: Stats,
    expected_below_declared: usize,
    novel_above_declared: usize,
    /// Midpoint between the highest novel top-1 and the lowest expected hit, when they separate.
    suggested: Option<f64>,
}

#[derive(Serialize)]
struct Report {
    index: String,
    slot: String,
    descriptor_hash: String,
    model_id: String,
    golden: String,
    top: usize,
    k: usize,
    ef_search: u32,
    queries: Vec<QueryReport>,
    mean_recall: f64,
    mrr: f64,
    contrastive_pass: usize,
    contrastive_total: usize,
    floor: FloorReport,
    gate_passed: bool,
}

impl EvalSlotCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let golden: Golden = match &self.golden {
            Some(path) => serde_json::from_str(
                &std::fs::read_to_string(path).with_context(|| format!("read {path}"))?,
            )
            .with_context(|| format!("parse {path}"))?,
            None => {
                serde_json::from_str(BUILTIN_GOLDEN).context("parse the built-in golden set")?
            }
        };
        if golden.queries.is_empty() || golden.queries.len() > 256 {
            bail!(
                "golden set has {} queries; need 1..=256 (one embedding request)",
                golden.queries.len()
            );
        }

        let client = opensearch_client::create_client(opensearch_url)?;
        let index =
            index_meta::resolve_index(&client, index_alias, self.version, self.index.as_deref())
                .await?;
        let mappings = index_meta::get_mappings(&client, &index).await?;
        let meta = index_meta::meta_of(&mappings).unwrap_or(json!({}));
        let slot = match &self.slot {
            Some(s) => s.clone(),
            None => embedding_slots::default_slot(&meta)
                .context("the index has no default slot; pass --slot")?,
        };
        let descriptor = embedding_slots::slot_descriptor(&meta, &slot)?;
        let field = descriptor.vector_field();
        if mappings["properties"].get(&field).is_none() {
            bail!("slot {slot} is in _meta but its field {field} is not mapped");
        }
        let expected_hash = descriptor.hash();

        // 1. One embedding request for every query, purpose `query`. A vector from any other
        //    descriptor is refused — the same check the api and the indexer make.
        let texts: Vec<&str> = golden.queries.iter().map(|q| q.text.as_str()).collect();
        let url = format!("{}/embed", self.embedding_service.trim_end_matches('/'));
        let resp: Value = reqwest::Client::new()
            .post(&url)
            .json(&json!({ "slot": slot, "purpose": "query", "texts": texts }))
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .with_context(|| format!("POST {url}"))?
            .json()
            .await
            .context("parse /embed response")?;
        let got_hash = resp["descriptor_hash"].as_str().unwrap_or("");
        if got_hash != expected_hash {
            bail!(
                "embedding-service answered with descriptor {got_hash}; the index's slot {slot} is {expected_hash}"
            );
        }
        let vectors: Vec<Vec<f64>> =
            serde_json::from_value(resp["vectors"].clone()).context("/embed vectors")?;
        if vectors.len() != golden.queries.len() {
            bail!(
                "embedding-service returned {} vectors for {} queries",
                vectors.len(),
                golden.queries.len()
            );
        }

        // 2. Which named documents exist. A name that does not is reported and left out of the
        //    metrics rather than counted as a miss — the set outlives any one corpus snapshot.
        let mut present: BTreeMap<String, bool> = BTreeMap::new();
        for q in &golden.queries {
            for n in q.expect.iter().chain(q.reject.iter()) {
                if !present.contains_key(n) {
                    let c = index_meta::count(
                        &client,
                        &index,
                        json!({ "query": { "term": { "name_raw": n } } }),
                    )
                    .await?;
                    present.insert(n.clone(), c > 0);
                }
            }
        }

        // 3. The api's query, once per golden query.
        let k = golden.top.max(K_MIN);
        let filter = eligibility(golden.filter.as_ref());
        let mut queries = Vec::with_capacity(golden.queries.len());
        for (q, vector) in golden.queries.iter().zip(vectors.iter()) {
            let hits = knn(&client, &index, &field, vector, k, &filter).await?;
            queries.push(score(q, &hits, golden.top, &present));
        }

        // 4. Aggregates, floor calibration, gate.
        let scored: Vec<&QueryReport> = queries.iter().filter(|r| r.recall.is_some()).collect();
        let scored_n = scored.len();
        let mean_recall = mean(scored.iter().filter_map(|r| r.recall));
        let mrr = mean(scored.iter().filter_map(|r| r.rr));
        drop(scored);
        let contrastive_total = queries.iter().filter(|r| r.contrastive.is_some()).count();
        let contrastive_pass = queries
            .iter()
            .filter(|r| r.contrastive == Some(true))
            .count();
        let floor = calibrate(&queries, descriptor.score_floor);
        let gate_passed = mean_recall >= self.min_recall
            && (!self.gate_contrastive || contrastive_pass == contrastive_total);
        let report = Report {
            index: index.clone(),
            slot: slot.clone(),
            descriptor_hash: expected_hash,
            model_id: descriptor.model_id.clone(),
            golden: golden.name.clone(),
            top: golden.top,
            k,
            ef_search: EF_SEARCH,
            queries,
            mean_recall,
            mrr,
            contrastive_pass,
            contrastive_total,
            floor,
            gate_passed,
        };

        print_report(&report, scored_n, self.min_recall, self.gate_contrastive);
        if let Some(path) = &self.json {
            std::fs::write(path, serde_json::to_string_pretty(&report)?)
                .with_context(|| format!("write {path}"))?;
            println!("Report written to {path}");
        }
        if !report.gate_passed {
            bail!(
                "gate failed: recall@{} {:.3} (min {:.3}){}",
                report.top,
                report.mean_recall,
                self.min_recall,
                if self.gate_contrastive {
                    format!(
                        ", contrastive {}/{}",
                        report.contrastive_pass, report.contrastive_total
                    )
                } else {
                    String::new()
                }
            );
        }
        Ok(())
    }
}

/// The api's eligibility for a request without scope: its `buildNonDeletedFilter`, verbatim,
/// plus the golden set's optional space / tag restriction.
fn eligibility(f: Option<&GoldenFilter>) -> Vec<Value> {
    let mut v = vec![json!({
        "bool": {
            "should": [
                { "term": { "deleted": false } },
                { "bool": { "must_not": [ { "exists": { "field": "deleted" } } ] } }
            ],
            "minimum_should_match": 1
        }
    })];
    if let Some(f) = f {
        if let Some(s) = &f.space_id {
            v.push(json!({ "term": { "space_id": s } }));
        }
        if let Some(t) = &f.tag_id {
            v.push(json!({
                "nested": { "path": "relations", "query": { "term": { "relations.to_entity_id": t } } }
            }));
        }
    }
    v
}

/// The api's `knnClause`: filters inside the clause (exact within the filter), `ef_search` per
/// request (the Lucene engine ignores the index setting), `k` candidates, names in `_source`.
async fn knn(
    client: &OpenSearch,
    index: &str,
    field: &str,
    vector: &[f64],
    k: usize,
    filter: &[Value],
) -> Result<Vec<Hit>> {
    let body = json!({
        "size": k,
        "_source": ["name"],
        "query": { "knn": { field: {
            "vector": vector,
            "k": k,
            "method_parameters": { "ef_search": EF_SEARCH },
            "filter": { "bool": { "filter": filter } }
        } } }
    });
    let resp = client
        .search(SearchParts::Index(&[index]))
        .body(body)
        .send()
        .await
        .context("k-NN search")?;
    let status = resp.status_code();
    let v: Value = resp.json().await.context("parse k-NN response")?;
    if !status.is_success() {
        bail!(
            "k-NN search failed ({status}): {}",
            v["error"]["reason"].as_str().unwrap_or("unknown error")
        );
    }
    Ok(v["hits"]["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, h)| Hit {
            rank: i + 1,
            name: h["_source"]["name"].as_str().unwrap_or("").to_string(),
            score: h["_score"].as_f64().unwrap_or(0.0),
        })
        .collect())
}

fn score(
    q: &GoldenQuery,
    hits: &[Hit],
    top: usize,
    present: &BTreeMap<String, bool>,
) -> QueryReport {
    let is_present = |n: &str| present.get(n).copied().unwrap_or(false);
    let outcome = |name: &String| {
        let h = hits.iter().find(|h| &h.name == name);
        NameOutcome {
            name: name.clone(),
            rank: h.map(|h| h.rank),
            score: h.map(|h| h.score),
        }
    };
    let missing: Vec<String> = q
        .expect
        .iter()
        .chain(q.reject.iter())
        .filter(|n| !is_present(n))
        .cloned()
        .collect();
    let expect: Vec<NameOutcome> = q.expect.iter().map(outcome).collect();
    let reject: Vec<NameOutcome> = q
        .reject
        .iter()
        .filter(|n| is_present(n))
        .map(outcome)
        .collect();
    let present_expect: Vec<&NameOutcome> = expect.iter().filter(|o| is_present(&o.name)).collect();

    let (recall, rr) = if present_expect.is_empty() {
        (None, None)
    } else {
        let in_top = present_expect
            .iter()
            .filter(|o| o.rank.is_some_and(|r| r <= top))
            .count();
        let best = present_expect
            .iter()
            .filter_map(|o| o.rank)
            .filter(|r| *r <= top)
            .min();
        (
            Some(in_top as f64 / present_expect.len() as f64),
            Some(best.map(|r| 1.0 / r as f64).unwrap_or(0.0)),
        )
    };
    let contrastive = if present_expect.is_empty() || reject.is_empty() {
        None
    } else {
        let worst_expect = present_expect
            .iter()
            .map(|o| o.rank.unwrap_or(usize::MAX))
            .max()
            .unwrap_or(usize::MAX);
        let best_reject = reject
            .iter()
            .filter_map(|o| o.rank)
            .min()
            .unwrap_or(usize::MAX);
        Some(worst_expect < best_reject)
    };
    QueryReport {
        id: q.id.clone(),
        kind: q.kind.clone(),
        text: q.text.clone(),
        top1: hits.first().cloned(),
        expect,
        reject,
        missing,
        recall,
        rr,
        contrastive,
    }
}

fn calibrate(queries: &[QueryReport], declared: f64) -> FloorReport {
    let expected: Vec<f64> = queries
        .iter()
        .flat_map(|r| r.expect.iter().filter_map(|o| o.score))
        .collect();
    let rejects: Vec<f64> = queries
        .iter()
        .flat_map(|r| r.reject.iter().filter_map(|o| o.score))
        .collect();
    let novel: Vec<f64> = queries
        .iter()
        .filter(|r| r.recall.is_none() && r.missing.is_empty())
        .filter_map(|r| r.top1.as_ref().map(|h| h.score))
        .collect();
    let expected_below_declared = expected.iter().filter(|s| **s < declared).count();
    let novel_above_declared = novel.iter().filter(|s| **s >= declared).count();
    let suggested = match (
        novel
            .iter()
            .cloned()
            .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))),
        expected
            .iter()
            .cloned()
            .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x)))),
    ) {
        (Some(max_novel), Some(min_expected)) if max_novel < min_expected => {
            Some((max_novel + min_expected) / 2.0)
        }
        _ => None,
    };
    FloorReport {
        declared,
        expected_hits: stats(expected),
        reject_hits: stats(rejects),
        novel_top1: stats(novel),
        expected_below_declared,
        novel_above_declared,
        suggested,
    }
}

fn stats(mut xs: Vec<f64>) -> Stats {
    if xs.is_empty() {
        return Stats::default();
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    let p = |q: f64| xs[((xs.len() - 1) as f64 * q).round() as usize];
    Stats {
        n: xs.len(),
        min: Some(xs[0]),
        p10: Some(p(0.1)),
        median: Some(p(0.5)),
        p90: Some(p(0.9)),
        max: Some(xs[xs.len() - 1]),
    }
}

fn mean(it: impl Iterator<Item = f64>) -> f64 {
    let (n, sum) = it.fold((0usize, 0.0), |(n, s), x| (n + 1, s + x));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn fmt_stats(s: &Stats) -> String {
    match (s.min, s.median, s.max) {
        (Some(min), Some(med), Some(max)) => {
            format!("n={} min {min:.3} median {med:.3} max {max:.3}", s.n)
        }
        _ => "n=0".to_string(),
    }
}

fn print_report(r: &Report, scored: usize, min_recall: f64, gate_contrastive: bool) {
    println!(
        "\nIndex {}   slot {} ({}, hash {}…)   golden {} ({} queries)   k={} ef_search={} top={}\n",
        r.index,
        r.slot,
        r.model_id,
        &r.descriptor_hash[..12],
        r.golden,
        r.queries.len(),
        r.k,
        r.ef_search,
        r.top
    );
    println!(
        "{:<12} {:<28} {:<48} {:>5} {:>5}  contrastive",
        "kind",
        "id",
        "top-1",
        format!("r@{}", r.top),
        "rr"
    );
    for q in &r.queries {
        let top1 = q
            .top1
            .as_ref()
            .map(|h| format!("{:.3} {}", h.score, short(&h.name, 40)))
            .unwrap_or_else(|| "(no hits)".into());
        let num = |x: Option<f64>| x.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into());
        let contrastive = match q.contrastive {
            None => "—".to_string(),
            Some(true) => "ok".to_string(),
            Some(false) => {
                let best = q
                    .reject
                    .iter()
                    .filter(|o| o.rank.is_some())
                    .min_by_key(|o| o.rank.unwrap_or(usize::MAX));
                match best {
                    Some(o) => format!(
                        "FAIL #{} {:.3} {}",
                        o.rank.unwrap_or(0),
                        o.score.unwrap_or(0.0),
                        short(&o.name, 36)
                    ),
                    None => "FAIL (expected outside k)".to_string(),
                }
            }
        };
        println!(
            "{:<12} {:<28} {:<48} {:>5} {:>5}  {}",
            q.kind,
            short(&q.id, 28),
            top1,
            num(q.recall),
            num(q.rr),
            contrastive
        );
        for m in &q.missing {
            println!("{:<12} {:<28} ! not in index: {}", "", "", m);
        }
    }
    println!(
        "\nrecall@{} {:.3} over {} queries   MRR {:.3}   contrastive {}/{} pass   missing names: {}",
        r.top,
        r.mean_recall,
        scored,
        r.mrr,
        r.contrastive_pass,
        r.contrastive_total,
        r.queries.iter().map(|q| q.missing.len()).sum::<usize>()
    );
    let f = &r.floor;
    println!("floor (declared {:.3}):", f.declared);
    println!("  expected hits  {}", fmt_stats(&f.expected_hits));
    println!("  reject hits    {}", fmt_stats(&f.reject_hits));
    println!("  novel top-1    {}", fmt_stats(&f.novel_top1));
    println!(
        "  declared floor drops {} expected hit(s) and admits {} novel top-1(s); {}",
        f.expected_below_declared,
        f.novel_above_declared,
        match f.suggested {
            Some(s) => format!("suggested floor {s:.3} (midpoint of max novel and min expected)"),
            None => "novel and expected scores overlap — no floor separates them".to_string(),
        }
    );
    println!(
        "GATE: {} (min recall {:.2}{})",
        if r.gate_passed { "pass" } else { "FAIL" },
        min_recall,
        if gate_contrastive {
            ", contrastive must all pass"
        } else {
            ""
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(names: &[(&str, f64)]) -> Vec<Hit> {
        names
            .iter()
            .enumerate()
            .map(|(i, (n, s))| Hit {
                rank: i + 1,
                name: n.to_string(),
                score: *s,
            })
            .collect()
    }
    fn query(expect: &[&str], reject: &[&str]) -> GoldenQuery {
        GoldenQuery {
            id: "q".into(),
            kind: "contrastive".into(),
            text: "t".into(),
            expect: expect.iter().map(|s| s.to_string()).collect(),
            reject: reject.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn present(names: &[&str]) -> BTreeMap<String, bool> {
        names.iter().map(|n| (n.to_string(), true)).collect()
    }

    #[test]
    fn recall_rr_and_contrastive_from_ranks() {
        let h = hits(&[("A", 0.95), ("R", 0.93), ("B", 0.91)]);
        let r = score(
            &query(&["A", "B"], &["R"]),
            &h,
            10,
            &present(&["A", "B", "R"]),
        );
        assert_eq!(r.recall, Some(1.0));
        assert_eq!(r.rr, Some(1.0));
        // B (rank 3) is below the reject (rank 2): the pair fails even though A leads.
        assert_eq!(r.contrastive, Some(false));
        assert!(r.missing.is_empty());
    }

    #[test]
    fn expected_outside_top_counts_against_recall_but_inside_k_still_scores() {
        let mut names: Vec<(&str, f64)> = (0..12).map(|_| ("x", 0.9)).collect();
        names.push(("A", 0.8)); // rank 13, inside k = 50 but outside top = 10
        let r = score(&query(&["A"], &[]), &hits(&names), 10, &present(&["A"]));
        assert_eq!(r.recall, Some(0.0));
        assert_eq!(r.rr, Some(0.0));
        assert_eq!(r.contrastive, None); // no rejects → not a contrastive row
        assert_eq!(r.expect[0].rank, Some(13));
    }

    #[test]
    fn missing_names_are_reported_and_excluded() {
        let h = hits(&[("A", 0.95)]);
        let r = score(
            &query(&["A", "GONE"], &["ALSO_GONE"]),
            &h,
            10,
            &present(&["A"]),
        );
        assert_eq!(r.missing, vec!["GONE".to_string(), "ALSO_GONE".to_string()]);
        assert_eq!(r.recall, Some(1.0)); // over the one present expected name
        assert_eq!(r.contrastive, None); // the only reject is absent
    }

    #[test]
    fn novel_query_has_no_metrics() {
        let r = score(&query(&[], &[]), &hits(&[("x", 0.7)]), 10, &present(&[]));
        assert_eq!((r.recall, r.rr, r.contrastive), (None, None, None));
        assert_eq!(r.top1.as_ref().map(|h| h.score), Some(0.7));
    }

    #[test]
    fn floor_suggests_midpoint_only_when_separable() {
        let good = |e: f64, novel: f64| {
            let expected = score(
                &query(&["A"], &[]),
                &hits(&[("A", e)]),
                10,
                &present(&["A"]),
            );
            let n = score(&query(&[], &[]), &hits(&[("x", novel)]), 10, &present(&[]));
            calibrate(&[expected, n], 0.85)
        };
        let f = good(0.90, 0.80);
        assert!((f.suggested.expect("separable") - 0.85).abs() < 1e-9);
        assert_eq!((f.expected_below_declared, f.novel_above_declared), (0, 0));
        let f = good(0.84, 0.88);
        assert_eq!(f.suggested, None);
        assert_eq!((f.expected_below_declared, f.novel_above_declared), (1, 1));
    }

    #[test]
    fn stats_percentiles() {
        let s = stats(vec![0.5, 0.1, 0.9, 0.3, 0.7]);
        assert_eq!(
            (s.n, s.min, s.median, s.max),
            (5, Some(0.1), Some(0.5), Some(0.9))
        );
        assert_eq!(stats(vec![]).n, 0);
    }

    #[test]
    fn eligibility_is_the_api_non_deleted_filter_plus_golden_scope() {
        assert_eq!(eligibility(None).len(), 1);
        let f = GoldenFilter {
            space_id: Some("s".into()),
            tag_id: Some("t".into()),
        };
        let v = eligibility(Some(&f));
        assert_eq!(v.len(), 3);
        assert_eq!(v[1]["term"]["space_id"], "s");
        assert_eq!(
            v[2]["nested"]["query"]["term"]["relations.to_entity_id"],
            "t"
        );
    }
}
