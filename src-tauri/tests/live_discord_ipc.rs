use didi_lib::discord::LiveDiscordIpcAdapter;
use didi_lib::discord::{DiscordError, DiscordPresenceAdapter};
use didi_lib::presence::{
    ActivityType, ApplicationId, ArtworkRef, PresenceActivity, StatusDisplayType, TimestampMode,
};
use std::time::Duration;

#[tokio::test]
async fn live_discord_client_compatibility_verification() -> Result<(), String> {
    // 1. Determine whether a test Application ID is configured via environment
    let configured_app_id = std::env::var("DISCORD_TEST_APP_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut adapter = LiveDiscordIpcAdapter::new();

    match configured_app_id {
        Some(app_id_str) => {
            println!("INFO: Found DISCORD_TEST_APP_ID in environment; running full live publish verification.");
            let app_id = ApplicationId::parse(&app_id_str).map_err(|e| e.to_string())?;

            let session = match adapter.connect(&app_id).await {
                Ok(s) => s,
                Err(DiscordError::DiscordNotRunning) => {
                    println!("SKIPPED: Live Discord Desktop client is not running on this host.");
                    return Ok(());
                }
                Err(e) => return Err(format!("Unexpected connect error: {e:?}")),
            };

            println!(
                "PROVEN: Connected to live Discord client (protocol v{}, slot {})",
                session.protocol_version, session.pipe_slot
            );

            let activity = PresenceActivity {
                activity_type: ActivityType::Playing,
                status_display_type: Some(StatusDisplayType::Name),
                name_override: None,
                details: Some("DiDi Phase 3 PoC Verification".to_owned()),
                details_url: None,
                state: Some("Live IPC Test".to_owned()),
                state_url: None,
                large_asset: ArtworkRef::None,
                large_text: None,
                large_url: None,
                small_asset: ArtworkRef::None,
                small_text: None,
                small_url: None,
                timestamp_mode: TimestampMode::None,
                start_timestamp: None,
                end_timestamp: None,
                buttons: Vec::new(),
            };

            adapter
                .set_activity(&activity)
                .await
                .map_err(|e| e.to_string())?;
            println!("PROVEN: Live Discord client acknowledged SET_ACTIVITY!");

            tokio::time::sleep(Duration::from_millis(500)).await;

            adapter.clear_activity().await.map_err(|e| e.to_string())?;
            println!("PROVEN: Live Discord client acknowledged CLEAR_ACTIVITY!");

            adapter.disconnect().await.map_err(|e| e.to_string())?;
            println!("PROVEN: Disconnected cleanly from live Discord client.");
        }
        None => {
            println!("INFO: DISCORD_TEST_APP_ID not set; testing live Discord handshake and rejection behavior.");
            let dummy_id = ApplicationId::parse("123456789012345678").map_err(|e| e.to_string())?;

            let result = adapter.connect(&dummy_id).await;
            match result {
                Err(DiscordError::InvalidClientId) => {
                    println!("PROVEN: Live Discord Desktop discovered, connected, and rejected unregistered App ID with code 4000 (InvalidClientId).");
                }
                Err(DiscordError::DiscordNotRunning) => {
                    println!("SKIPPED: Live Discord Desktop client is not running on this host.");
                }
                Err(other) => {
                    return Err(format!(
                        "Expected InvalidClientId from live Discord, got {other:?}"
                    ));
                }
                Ok(_) => {
                    return Err(
                        "Expected handshake to fail with unregistered dummy ID, but succeeded"
                            .to_owned(),
                    );
                }
            }
        }
    }

    Ok(())
}
