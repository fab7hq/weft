pub mod model;
pub mod normalize;

pub use model::{
    ConversationEntry, ConversationReading, PaneSnapshot, SenseCurrentPointer, SenseModel,
    SurfaceReading, SurfaceState, TranscriptSnippet, sha256_hex,
};
pub use normalize::{
    ADDED_TOKENS, compute_surface_dynamics, normalize_conversation_entry, normalize_surface_frame,
    normalize_terminal_line, quiet_bucket,
};
