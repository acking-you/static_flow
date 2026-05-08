//! Local binary journal for llm-access usage diagnostics.

pub mod cli;
pub mod config;
pub mod inspect;
pub mod reader;
pub mod retention;
pub mod state;
pub mod status;
pub mod wire;
pub mod writer;

pub use config::JournalConfig;
pub use inspect::collect_journal_file_lists;
pub use reader::JournalReader;
pub use state::JournalConsumerState;
pub use status::{
    JournalFileListsSnapshot, JournalFileSnapshot, JournalStatusSnapshot, WorkerProgressSnapshot,
};
pub use wire::{JournalUsageBatchV1, JournalUsageEventV1};
pub use writer::JournalWriter;
