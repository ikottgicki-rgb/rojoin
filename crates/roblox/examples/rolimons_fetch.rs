//! Check that the Rolimons scrape still works.
//!
//! The history on a game's page is not a documented API, so it can break
//! whenever they change their markup. This is the fastest way to find out:
//!
//! ```text
//! cargo run -p rojoin-roblox --example rolimons_fetch            # Jailbreak
//! cargo run -p rojoin-roblox --example rolimons_fetch -- 1818    # a small game
//! ```
//!
//! A parse failure here means `rolimons::extract` needs updating, and the
//! History tab will be showing its error state to users in the meantime.

#[tokio::main]
async fn main() {
    let place: i64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(606849621);

    match rojoin_roblox::rolimons::history(place).await {
        Ok(h) => {
            let now = h.timestamps.last().copied().unwrap_or(0);
            println!(
                "OK  place {place}: {} samples over {} days",
                h.timestamps.len(),
                h.covered_days()
            );
            println!(
                "    playing now {:?} · 30d peak {:?} · rating {:?}% · avg session {:?} min",
                h.latest_players(),
                h.peak(30, now).map(|(_, p)| p),
                h.latest_rating(),
                h.latest_avg_playtime(),
            );
            let columns = rojoin_roblox::rolimons::bucket_players(&h.recent_players(30, now), 150);
            println!("    30 days reduces to {} columns", columns.len());
        }
        Err(e) => {
            println!("FAILED place {place}: {e}");
            std::process::exit(1);
        }
    }
}
