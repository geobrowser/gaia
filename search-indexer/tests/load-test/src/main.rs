mod config;
#[allow(dead_code)]
mod expected_state;
#[allow(dead_code)]
mod generators;
mod kafka_lag;
mod scenario;
mod sender;
mod validator;

use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use config::LoadTestConfig;
use sender::LoadTestSender;

#[tokio::main]
async fn main() -> Result<()> {
    let config = LoadTestConfig::parse();

    // Set up logging
    let filter = if config.debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    println!();
    println!("  Search-Indexer Load Test");
    println!("  =======================");
    println!("  Seed:    {}", config.seed);
    println!("  Scale:   {}", config.scale);
    println!("  Broker:  {}", config.broker);
    println!("  OpenSearch: {}", config.opensearch_url);
    println!("  Index:   {}", config.resolved_index());
    println!();

    // =========================================================================
    // Step 1: Generate events + expected state
    // =========================================================================
    if !config.validate_only {
        println!("  [1/6] Generating events...");
        let gen_start = Instant::now();
        let scenario = scenario::generate(&config)?;
        let gen_duration = gen_start.elapsed();

        println!(
            "  Generated {} events + {} late scores in {:.2}s",
            scenario.stats.total_events,
            scenario.stats.late_scores,
            gen_duration.as_secs_f64()
        );
        println!(
            "    Add+Delete relation:     {}",
            scenario.stats.add_delete_relation
        );
        println!(
            "    Create+Delete entity:    {}",
            scenario.stats.create_delete_entity
        );
        println!(
            "    Delete+Restore entity:   {}",
            scenario.stats.delete_restore_entity
        );
        println!(
            "    Score overwrite race:    {}",
            scenario.stats.score_overwrite
        );
        println!(
            "    Interleaved space:       {}",
            scenario.stats.interleaved_space
        );
        println!(
            "    Early space topic:       {}",
            scenario.stats.early_space_topic
        );
        println!(
            "    Unset then set:          {}",
            scenario.stats.unset_then_set
        );
        println!(
            "    Bulk relation churn:     {}",
            scenario.stats.bulk_relation_churn
        );
        println!(
            "    Avatar+Cover relations:  {}",
            scenario.stats.avatar_cover_relations
        );
        println!(
            "    Space scores:            {}",
            scenario.stats.space_scores
        );
        println!(
            "    Perspective scores:      {}",
            scenario.stats.perspective_scores
        );
        println!("    Filler:                  {}", scenario.stats.filler);
        println!(
            "    Late scores:             {}",
            scenario.stats.late_scores
        );
        println!(
            "  Expected documents: {} total, {} live",
            scenario.expected_state.total_doc_count(),
            scenario.expected_state.live_doc_count(),
        );
        println!();

        // =====================================================================
        // Step 2: Send main events to Kafka (interleaved)
        // =====================================================================
        println!("  [2/6] Sending main events to Kafka...");
        let sender = LoadTestSender::new(&config.broker)?;
        let send_stats = sender.send_all(scenario.events).await?;

        println!(
            "  Sent {} events in {:.2}s ({:.0} events/sec)",
            send_stats.total,
            send_stats.duration.as_secs_f64(),
            send_stats.total as f64 / send_stats.duration.as_secs_f64(),
        );
        if send_stats.errors > 0 {
            println!("  WARNING: {} send errors", send_stats.errors);
        }
        println!();

        if config.send_only {
            println!("  --send-only mode: skipping validation");
            println!();
            return Ok(());
        }

        // =====================================================================
        // Step 3: Wait for indexer to process main events
        // =====================================================================
        println!("  [3/6] Waiting for indexer to process main events...");
        let http_client = reqwest::Client::new();
        let main_stats = validator::wait_for_processing(
            &http_client,
            &config,
            scenario.expected_state.total_doc_count(),
        )
        .await?;
        let wait_duration = main_stats.duration;
        println!();

        // =====================================================================
        // Step 4: Send late score events to Kafka
        // =====================================================================
        let late_count = scenario.late_score_events.len();
        println!("  [4/6] Sending {} late score events to Kafka...", late_count);
        let late_send_stats = sender.send_all(scenario.late_score_events).await?;

        println!(
            "  Sent {} late scores in {:.2}s ({:.0} events/sec)",
            late_send_stats.total,
            late_send_stats.duration.as_secs_f64(),
            if late_send_stats.duration.as_secs_f64() > 0.0 {
                late_send_stats.total as f64 / late_send_stats.duration.as_secs_f64()
            } else {
                0.0
            },
        );
        if late_send_stats.errors > 0 {
            println!("  WARNING: {} late send errors", late_send_stats.errors);
        }
        println!();

        // =====================================================================
        // Step 5: Wait for indexer to consume all late scores
        // =====================================================================
        println!("  [5/6] Waiting for indexer to consume late scores...");
        let scores_group_id = config.resolved_scores_group_id();

        let late_score_timeout_secs = 300; // 5 minutes
        let late_wait_duration = kafka_lag::wait_for_committed_past(
            &config.broker,
            &scores_group_id,
            &late_send_stats.max_offsets,
            late_score_timeout_secs,
        )
        .await?;

        println!("  Late scores committed in {:.1}s", late_wait_duration.as_secs_f64());

        // Final refresh to ensure all updates are searchable in OpenSearch
        let refresh_url = format!("{}/{}/_refresh", config.opensearch_url, config.resolved_index());
        http_client.post(&refresh_url).send().await?.error_for_status()?;
        println!();

        // =====================================================================
        // Step 6: Validate
        // =====================================================================
        println!("  [6/6] Validating documents...");
        let report =
            validator::validate(&http_client, &config, &scenario.expected_state).await?;
        report.print();

        // Print overall timing summary
        println!();
        println!("  === Timing Summary ===");
        println!("  Generation:    {:.2}s", gen_duration.as_secs_f64());
        println!(
            "  Send:          {:.2}s ({} events)",
            send_stats.duration.as_secs_f64(),
            send_stats.total
        );
        println!("  Wait/index:    {:.2}s", wait_duration.as_secs_f64());
        println!(
            "  Late scores:   {:.2}s send + {:.2}s wait ({} events)",
            late_send_stats.duration.as_secs_f64(),
            late_wait_duration.as_secs_f64(),
            late_send_stats.total
        );
        println!("  Validation:    {:.2}s", report.duration.as_secs_f64());
        println!(
            "  Total:         {:.2}s",
            gen_duration.as_secs_f64()
                + send_stats.duration.as_secs_f64()
                + wait_duration.as_secs_f64()
                + late_send_stats.duration.as_secs_f64()
                + late_wait_duration.as_secs_f64()
                + report.duration.as_secs_f64()
        );
        println!();

        // Print performance metrics
        let total_docs = main_stats.samples.last().map(|(c, _)| *c).unwrap_or(0);
        let processing_secs = main_stats.duration.as_secs_f64();

        println!("  === Performance ===");
        println!("  Main event processing:");
        println!(
            "    {:>5} events -> {:>5} docs in {:.1}s",
            send_stats.total, total_docs, processing_secs
        );
        if processing_secs > 0.0 {
            println!(
                "    Throughput: {:.0} events/sec, {:.0} docs/sec",
                send_stats.total as f64 / processing_secs,
                total_docs as f64 / processing_secs,
            );
        }

        println!("  Late score processing:");
        let late_secs = late_wait_duration.as_secs_f64();
        println!(
            "    {:>5} scores in {:.1}s",
            late_send_stats.total, late_secs
        );
        if late_secs > 0.0 {
            println!(
                "    Throughput: {:.0} scores/sec",
                late_send_stats.total as f64 / late_secs,
            );
        }

        // Document milestones from samples
        if total_docs > 0 {
            println!("  Document milestones:");
            for pct in [25u32, 50, 75, 100] {
                let target = (total_docs as f64 * pct as f64 / 100.0).ceil() as u64;
                if let Some((count, elapsed)) =
                    main_stats.samples.iter().find(|(c, _)| *c >= target)
                {
                    println!(
                        "    {:>3}% ({:>5} docs): {:>6.1}s",
                        pct,
                        count,
                        elapsed.as_secs_f64()
                    );
                }
            }
        }
        println!();

        if !report.is_pass() {
            std::process::exit(1);
        }
    } else {
        // validate-only mode
        println!("  [1/2] Regenerating expected state...");
        let gen_start = Instant::now();
        let scenario = scenario::generate(&config)?;
        let gen_duration = gen_start.elapsed();
        println!(
            "  Regenerated expected state in {:.2}s ({} documents)",
            gen_duration.as_secs_f64(),
            scenario.expected_state.total_doc_count(),
        );
        println!();

        println!("  [2/2] Validating documents...");
        let http_client = reqwest::Client::new();

        let refresh_url = format!("{}/{}/_refresh", config.opensearch_url, config.resolved_index());
        http_client.post(&refresh_url).send().await?.error_for_status()?;

        let report =
            validator::validate(&http_client, &config, &scenario.expected_state).await?;
        report.print();

        if !report.is_pass() {
            std::process::exit(1);
        }
    }

    Ok(())
}
