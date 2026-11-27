pub mod model;
pub mod rate_limit;
pub mod service;
pub mod state;
pub mod validation;

pub use model::{
    ActivityButton, ActivityType, ApplicationId, ApplicationIdError, ArtworkRef, PresenceActivity,
    StatusDisplayType, TimestampMode,
};
pub use rate_limit::{
    CoalescingBuffer, RollingRateLimiter, DEBOUNCE_DURATION, MAX_UPDATES_PER_WINDOW,
    RATE_LIMIT_WINDOW,
};
pub use service::{PresenceService, PresenceServiceHandle, PresenceServiceStatus, ServiceCommand};
pub use state::PresenceState;
pub use validation::ValidationError;
