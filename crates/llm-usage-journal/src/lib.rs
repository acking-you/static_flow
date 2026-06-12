//! Local binary journal for llm-access usage diagnostics.

pub mod cli;
pub mod config;
pub mod inspect;
pub mod preview;
pub mod reader;
pub mod recovery;
pub mod retention;
pub mod rollup;
pub mod state;
pub mod status;
pub mod wire;
pub mod writer;
pub mod writer_state;

pub use config::JournalConfig;
pub use inspect::collect_journal_file_lists;
pub use preview::{JournalPreviewReader, JournalPreviewReport};
pub use reader::{JournalBatchStream, JournalFileSummary, JournalReader, JournalStreamReport};
pub use recovery::{recover_orphan_active_files, ActiveRecoveryReport};
pub use rollup::{
    recover_orphan_active_rollup_files, RollupActiveRecoveryReport, RollupJournalBatchStream,
    RollupJournalReader, RollupJournalWriter,
};
pub use state::JournalConsumerState;
pub use status::{
    JournalFileListsSnapshot, JournalFileSnapshot, JournalStatusSnapshot, WorkerProgressSnapshot,
};
pub use wire::{JournalUsageBatchV1, JournalUsageEventV1};
pub use writer::JournalWriter;
