//! Read-only discovery for the local worktree browser; ownership is not activity or exclusion.

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use sofia_protocol::ThreadId;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub id: uuid::Uuid,
    pub cwd: PathBuf,
    pub thread_id: Option<ThreadId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub cwd: PathBuf,
    pub owner: Option<ThreadId>,
}

#[derive(Clone, Debug)]
pub(crate) enum Action {
    Resume(ThreadId),
    Copy(PathBuf),
}

pub(crate) fn fetch(request: Request, sofia_home: PathBuf, tx: AppEventSender) {
    tokio::spawn(async move {
        let result = list(sofia_home, request.cwd.clone())
            .await
            .map_err(|error| error.to_string());
        tx.send(AppEvent::ManagedWorktreesLoaded { request, result });
    });
}

pub(crate) async fn list(sofia_home: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<Entry>> {
    let host = crate::legacy_core::config::load_config_toml_with_layer_stack(
        &sofia_home,
        /*cwd*/ None,
        Vec::new(),
        sofia_config::ConfigLoadOptions::default(),
    )
    .await?;
    let settings =
        sofia_worktree::WorktreeSettings::for_cli(&sofia_home, host.config_toml.desktop.as_ref())?;
    // Closing the popup discards its result; an already-running blocking Git call still finishes.
    tokio::task::spawn_blocking(move || {
        let cwd = sofia_git_utils::get_git_repo_root(&cwd).unwrap_or(cwd);
        let manager = sofia_worktree::WorktreeManager::new(settings);
        Ok(manager
            .list(&cwd)?
            .into_iter()
            .map(|checkout| Entry {
                owner: manager
                    .owner(&checkout.root)
                    .ok()
                    .flatten()
                    .and_then(|owner| ThreadId::from_string(&owner).ok()),
                cwd: checkout.cwd,
            })
            .collect())
    })
    .await?
}
