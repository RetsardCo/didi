pub mod adapter;
pub mod codec;
pub mod errors;
pub mod live_adapter;
pub mod payload;
pub mod reconnect;
pub mod windows_pipe;

pub use adapter::{
    AdapterEvent, DiscordPresenceAdapter, MockAdapterCall, MockDiscordAdapter, SessionInfo,
};
pub use errors::DiscordError;
pub use live_adapter::LiveDiscordIpcAdapter;
pub use reconnect::{BackoffSchedule, PresenceReconnectStateMachine};
