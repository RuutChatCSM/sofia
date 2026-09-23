use crate::config::Config;
pub use sofia_rollout::ARCHIVED_SESSIONS_SUBDIR;
pub use sofia_rollout::Cursor;
pub use sofia_rollout::INTERACTIVE_SESSION_SOURCES;
pub use sofia_rollout::RolloutRecorder;
pub use sofia_rollout::RolloutRecorderParams;
pub use sofia_rollout::SESSIONS_SUBDIR;
pub use sofia_rollout::SessionMeta;
pub use sofia_rollout::SortDirection;
pub use sofia_rollout::ThreadItem;
pub use sofia_rollout::ThreadSortKey;
pub use sofia_rollout::ThreadsPage;
pub use sofia_rollout::append_thread_name;
pub use sofia_rollout::find_archived_thread_path_by_id_str;
#[deprecated(note = "use find_thread_path_by_id_str")]
pub use sofia_rollout::find_conversation_path_by_id_str;
pub use sofia_rollout::find_thread_meta_by_name_str;
pub use sofia_rollout::find_thread_name_by_id;
pub use sofia_rollout::find_thread_names_by_ids;
pub use sofia_rollout::find_thread_path_by_id_str;
pub use sofia_rollout::parse_cursor;
pub use sofia_rollout::read_head_for_summary;
pub use sofia_rollout::read_session_meta_line;
pub use sofia_rollout::rollout_date_parts;

impl sofia_rollout::RolloutConfigView for Config {
    fn sofia_home(&self) -> &std::path::Path {
        self.sofia_home.as_path()
    }

    fn sqlite_config(&self) -> &sofia_state::SqliteConfig {
        self.sqlite_config()
    }

    fn cwd(&self) -> &std::path::Path {
        self.cwd.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }

    fn generate_memories(&self) -> bool {
        self.memories.generate_memories
    }
}

pub(crate) mod list {
    pub use sofia_rollout::find_thread_path_by_id_str;
}

#[cfg(test)]
pub(crate) mod recorder {
    pub use sofia_rollout::RolloutRecorder;
}

pub(crate) use crate::session_rollout_init_error::map_session_init_error;

pub(crate) mod truncation {
    pub(crate) use crate::thread_rollout_truncation::*;
}
