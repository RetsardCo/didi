pub mod app;
pub mod discord;
pub mod errors;
pub mod presence;
pub mod storage;

use std::path::PathBuf;
use tauri::{Manager, RunEvent};

use crate::app::coordinator::{PendingImportState, StartupNoticeDto};
use crate::app::lifecycle::{open_or_focus_settings_window, request_exit, AppLifecycleState};
use crate::app::startup::{
    has_minimized_flag, reconcile_autostart_on_startup, TauriAutostartBackend, MINIMIZED_ARG,
};
use crate::app::tray::setup_tray;
use crate::discord::LiveDiscordIpcAdapter;
use crate::errors::DiDiError;
use crate::presence::{ApplicationId, PresenceService};
use crate::storage::{AppConfigV1, ConfigStore};

pub fn run() {
    let launch_args: Vec<String> = std::env::args().collect();
    let start_minimized = has_minimized_flag(&launch_args);

    let default_config_dir = std::env::var("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("DiDi"))
        .unwrap_or_else(|_| PathBuf::from("config"));

    let config_store = ConfigStore::new(default_config_dir);

    let (initial_config, startup_notice, can_publish_on_startup) = match config_store.load_or_init()
    {
        Ok((cfg, status)) => (cfg, StartupNoticeDto::from_load_success(status), true),
        Err(err) => {
            let notice = StartupNoticeDto::from_load_error(&err);
            let mut safe_fallback = AppConfigV1::default_config();
            safe_fallback.presence_enabled = false;
            (safe_fallback, notice, false)
        }
    };

    let (presence_service, service_handle) = if can_publish_on_startup {
        // Read DISCORD_TEST_APP_ID if present in the environment for live authorized testing,
        // otherwise fallback to the effective Application ID of the active profile.
        let initial_app_id = std::env::var("DISCORD_TEST_APP_ID")
            .ok()
            .and_then(|raw| ApplicationId::parse(&raw).ok())
            .or_else(|| {
                initial_config
                    .active_profile()
                    .ok()
                    .and_then(|p| {
                        p.effective_application_id(initial_config.default_application_id.as_deref())
                    })
                    .and_then(|id_str| ApplicationId::parse(id_str).ok())
            });

        let initial_activity = if initial_config.presence_enabled {
            initial_config
                .active_profile()
                .ok()
                .map(|p| p.to_presence_activity(None))
        } else {
            None
        };

        PresenceService::with_initial_activity(
            Box::new(LiveDiscordIpcAdapter::new()),
            initial_app_id,
            initial_config.presence_enabled,
            initial_activity,
        )
    } else {
        let err_msg = startup_notice.message.clone().unwrap_or_else(|| {
            DiDiError::Lifecycle("configuration integrity error".to_owned()).user_safe_message()
        });
        PresenceService::with_startup_error(Box::new(LiveDiscordIpcAdapter::new()), err_msg)
    };

    let lifecycle_state = AppLifecycleState::with_close_to_tray(initial_config.close_to_tray);
    let should_reconcile_autostart = startup_notice.can_edit_config;

    let builder = tauri::Builder::default()
        .manage(lifecycle_state)
        .manage(service_handle)
        .manage(config_store)
        .manage(startup_notice)
        .manage(PendingImportState::default())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !has_minimized_flag(&args) {
                let _ = open_or_focus_settings_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![MINIMIZED_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            app::commands::get_scaffold_status,
            app::commands::get_presence_status,
            app::commands::get_startup_notice,
            app::commands::retry_presence,
            app::commands::exit_app,
            app::commands::get_app_config,
            app::commands::save_profile,
            app::commands::delete_profile,
            app::commands::set_active_profile,
            app::commands::set_default_application_id,
            app::commands::set_presence_enabled,
            app::commands::update_preferences,
            app::commands::pick_and_preview_profile_import,
            app::commands::confirm_profile_import,
            app::commands::export_profiles_to_file,
            app::commands::open_logs_folder,
            app::commands::open_external_docs_url,
        ])
        .setup(move |app| {
            if should_reconcile_autostart {
                let store = app.state::<ConfigStore>();
                let backend = TauriAutostartBackend(app.handle());
                let _ = reconcile_autostart_on_startup(store.inner(), &backend);
            }

            tauri::async_runtime::spawn(presence_service.run());

            setup_tray(app.handle())?;

            if !start_minimized {
                open_or_focus_settings_window(app.handle())?;
            }

            Ok(())
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(instance) => instance,
        Err(err) => {
            eprintln!("DiDi failed to initialize: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            let lifecycle = app_handle.state::<AppLifecycleState>();
            if !lifecycle.is_exiting() {
                if lifecycle.close_to_tray() {
                    // Keep the native Rust tray runtime alive after the on-demand settings window
                    // is closed/destroyed when close_to_tray = true (D-019).
                    api.prevent_exit();
                } else {
                    api.prevent_exit();
                    request_exit(app_handle);
                }
            }
        }
    });
}
