use crate::bespoke_event_handling::apply_bespoke_event_handling;
use crate::command_exec::CommandExecManager;
use crate::command_exec::StartCommandExecParams;
use crate::config_manager::ConfigManager;
use crate::error_code::INPUT_TOO_LARGE_ERROR_CODE;
use crate::error_code::invalid_params;
use crate::models::supported_models;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::RequestContext;
use crate::outgoing_message::ThreadScopedOutgoingMessageSender;
use crate::skills_watcher::SkillsWatcher;
use crate::thread_status::ThreadWatchManager;
use crate::thread_status::resolve_thread_status;
use chrono::Duration as ChronoDuration;
use chrono::SecondsFormat;
use sofia_analytics::AnalyticsEventsClient;
use sofia_analytics::AnalyticsJsonRpcError;
use sofia_analytics::InputError;
use sofia_analytics::TurnSteerRequestError;
use sofia_app_server_protocol::Account;
use sofia_app_server_protocol::AccountLoginCompletedNotification;
use sofia_app_server_protocol::AccountTokenUsageDailyBucket;
use sofia_app_server_protocol::AccountTokenUsageSummary;
use sofia_app_server_protocol::AccountUpdatedNotification;
use sofia_app_server_protocol::AddCreditsNudgeCreditType;
use sofia_app_server_protocol::AddCreditsNudgeEmailStatus;
use sofia_app_server_protocol::AdditionalContextEntry;
use sofia_app_server_protocol::AdditionalContextKind;
use sofia_app_server_protocol::AppListUpdatedNotification;
use sofia_app_server_protocol::AppSummary;
use sofia_app_server_protocol::AppTemplateSummary;
use sofia_app_server_protocol::AppTemplateUnavailableReason;
use sofia_app_server_protocol::AppsInstalledParams;
use sofia_app_server_protocol::AppsInstalledResponse;
use sofia_app_server_protocol::AppsListParams;
use sofia_app_server_protocol::AppsListResponse;
use sofia_app_server_protocol::AppsReadParams;
use sofia_app_server_protocol::AppsReadResponse;
use sofia_app_server_protocol::AskForApproval;
use sofia_app_server_protocol::AuthMode;
use sofia_app_server_protocol::CancelLoginAccountParams;
use sofia_app_server_protocol::CancelLoginAccountResponse;
use sofia_app_server_protocol::CancelLoginAccountStatus;
use sofia_app_server_protocol::ClientInfo;
use sofia_app_server_protocol::ClientRequest;
use sofia_app_server_protocol::ClientResponsePayload;
use sofia_app_server_protocol::CodexErrorInfo;
use sofia_app_server_protocol::CollaborationModeListParams;
use sofia_app_server_protocol::CollaborationModeListResponse;
use sofia_app_server_protocol::CommandExecParams;
use sofia_app_server_protocol::CommandExecResizeParams;
use sofia_app_server_protocol::CommandExecTerminateParams;
use sofia_app_server_protocol::CommandExecWriteParams;
use sofia_app_server_protocol::ConfigWarningNotification;
use sofia_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;
use sofia_app_server_protocol::ConsumeAccountRateLimitResetCreditParams;
use sofia_app_server_protocol::ConsumeAccountRateLimitResetCreditResponse;
use sofia_app_server_protocol::ConversationGitInfo;
use sofia_app_server_protocol::ConversationSummary;
use sofia_app_server_protocol::DeprecationNoticeNotification;
use sofia_app_server_protocol::DynamicToolFunctionSpec;
use sofia_app_server_protocol::DynamicToolNamespaceTool;
use sofia_app_server_protocol::DynamicToolSpec;
use sofia_app_server_protocol::EnvironmentAddParams;
use sofia_app_server_protocol::EnvironmentAddResponse;
use sofia_app_server_protocol::EnvironmentInfoParams;
use sofia_app_server_protocol::EnvironmentInfoResponse;
use sofia_app_server_protocol::EnvironmentShellInfo;
use sofia_app_server_protocol::EnvironmentStatusKind;
use sofia_app_server_protocol::EnvironmentStatusParams;
use sofia_app_server_protocol::EnvironmentStatusResponse;
use sofia_app_server_protocol::ExperimentalFeature as ApiExperimentalFeature;
use sofia_app_server_protocol::ExperimentalFeatureListParams;
use sofia_app_server_protocol::ExperimentalFeatureListResponse;
use sofia_app_server_protocol::ExperimentalFeatureStage as ApiExperimentalFeatureStage;
use sofia_app_server_protocol::FeedbackUploadParams;
use sofia_app_server_protocol::FeedbackUploadResponse;
use sofia_app_server_protocol::GetAccountParams;
use sofia_app_server_protocol::GetAccountRateLimitsResponse;
use sofia_app_server_protocol::GetAccountResponse;
use sofia_app_server_protocol::GetAccountTokenUsageParams;
use sofia_app_server_protocol::GetAccountTokenUsageResponse;
use sofia_app_server_protocol::GetAuthStatusParams;
use sofia_app_server_protocol::GetAuthStatusResponse;
use sofia_app_server_protocol::GetConversationSummaryParams;
use sofia_app_server_protocol::GetConversationSummaryResponse;
use sofia_app_server_protocol::GetWorkspaceMessagesResponse;
use sofia_app_server_protocol::GitDiffToRemoteParams;
use sofia_app_server_protocol::GitDiffToRemoteResponse;
use sofia_app_server_protocol::GitInfo as ApiGitInfo;
use sofia_app_server_protocol::HookHandlerMetadata;
use sofia_app_server_protocol::HookMetadata;
use sofia_app_server_protocol::HooksListParams;
use sofia_app_server_protocol::HooksListResponse;
use sofia_app_server_protocol::InitializeParams;
use sofia_app_server_protocol::InitializeResponse;
use sofia_app_server_protocol::InstalledApp;
use sofia_app_server_protocol::JSONRPCErrorError;
use sofia_app_server_protocol::ListMcpServerStatusParams;
use sofia_app_server_protocol::ListMcpServerStatusResponse;
use sofia_app_server_protocol::LoginAccountParams;
use sofia_app_server_protocol::LoginAccountResponse;
use sofia_app_server_protocol::LoginApiKeyParams;
use sofia_app_server_protocol::LoginAppBrand;
use sofia_app_server_protocol::LogoutAccountResponse;
use sofia_app_server_protocol::MarketplaceAddParams;
use sofia_app_server_protocol::MarketplaceAddResponse;
use sofia_app_server_protocol::MarketplaceInterface;
use sofia_app_server_protocol::MarketplaceRemoveParams;
use sofia_app_server_protocol::MarketplaceRemoveResponse;
use sofia_app_server_protocol::MarketplaceUpgradeErrorInfo;
use sofia_app_server_protocol::MarketplaceUpgradeParams;
use sofia_app_server_protocol::MarketplaceUpgradeResponse;
use sofia_app_server_protocol::McpResourceReadParams;
use sofia_app_server_protocol::McpResourceReadResponse;
use sofia_app_server_protocol::McpServerOauthClientRegistration;
use sofia_app_server_protocol::McpServerOauthLoginCompletedNotification;
use sofia_app_server_protocol::McpServerOauthLoginParams;
use sofia_app_server_protocol::McpServerOauthLoginResponse;
use sofia_app_server_protocol::McpServerRefreshResponse;
use sofia_app_server_protocol::McpServerStatus;
use sofia_app_server_protocol::McpServerStatusDetail;
use sofia_app_server_protocol::McpServerToolCallParams;
use sofia_app_server_protocol::McpServerToolCallResponse;
use sofia_app_server_protocol::MemoryResetResponse;
use sofia_app_server_protocol::MockExperimentalMethodParams;
use sofia_app_server_protocol::MockExperimentalMethodResponse;
use sofia_app_server_protocol::ModelListParams;
use sofia_app_server_protocol::ModelListResponse;
use sofia_app_server_protocol::PermissionProfileListParams;
use sofia_app_server_protocol::PermissionProfileListResponse;
use sofia_app_server_protocol::PermissionProfileSummary;
use sofia_app_server_protocol::PluginDetail;
use sofia_app_server_protocol::PluginInstallParams;
use sofia_app_server_protocol::PluginInstallResponse;
use sofia_app_server_protocol::PluginInstalledParams;
use sofia_app_server_protocol::PluginInstalledResponse;
use sofia_app_server_protocol::PluginInterface;
use sofia_app_server_protocol::PluginListMarketplaceKind;
use sofia_app_server_protocol::PluginListParams;
use sofia_app_server_protocol::PluginListResponse;
use sofia_app_server_protocol::PluginMarketplaceEntry;
use sofia_app_server_protocol::PluginReadParams;
use sofia_app_server_protocol::PluginReadResponse;
use sofia_app_server_protocol::PluginShareCheckoutParams;
use sofia_app_server_protocol::PluginShareCheckoutResponse;
use sofia_app_server_protocol::PluginShareContext;
use sofia_app_server_protocol::PluginShareDeleteParams;
use sofia_app_server_protocol::PluginShareDeleteResponse;
use sofia_app_server_protocol::PluginShareDiscoverability;
use sofia_app_server_protocol::PluginShareListItem;
use sofia_app_server_protocol::PluginShareListParams;
use sofia_app_server_protocol::PluginShareListResponse;
use sofia_app_server_protocol::PluginSharePrincipal;
use sofia_app_server_protocol::PluginSharePrincipalType;
use sofia_app_server_protocol::PluginShareSaveParams;
use sofia_app_server_protocol::PluginShareSaveResponse;
use sofia_app_server_protocol::PluginShareTarget;
use sofia_app_server_protocol::PluginShareUpdateDiscoverability;
use sofia_app_server_protocol::PluginShareUpdateTargetsParams;
use sofia_app_server_protocol::PluginShareUpdateTargetsResponse;
use sofia_app_server_protocol::PluginSkillReadParams;
use sofia_app_server_protocol::PluginSkillReadResponse;
use sofia_app_server_protocol::PluginSource;
use sofia_app_server_protocol::PluginSummary;
use sofia_app_server_protocol::PluginUninstallParams;
use sofia_app_server_protocol::PluginUninstallResponse;
use sofia_app_server_protocol::RateLimitResetCredit;
use sofia_app_server_protocol::RateLimitResetCreditStatus;
use sofia_app_server_protocol::RateLimitResetCreditsSummary;
use sofia_app_server_protocol::RateLimitResetType;
use sofia_app_server_protocol::RequestId;
use sofia_app_server_protocol::ReviewDelivery as ApiReviewDelivery;
use sofia_app_server_protocol::ReviewStartParams;
use sofia_app_server_protocol::ReviewStartResponse;
use sofia_app_server_protocol::ReviewTarget as ApiReviewTarget;
use sofia_app_server_protocol::SandboxMode;
use sofia_app_server_protocol::SendAddCreditsNudgeEmailParams;
use sofia_app_server_protocol::SendAddCreditsNudgeEmailResponse;
use sofia_app_server_protocol::ServerNotification;
use sofia_app_server_protocol::ServerRequestResolvedNotification;
use sofia_app_server_protocol::SkillSummary;
use sofia_app_server_protocol::SkillsConfigWriteParams;
use sofia_app_server_protocol::SkillsConfigWriteResponse;
use sofia_app_server_protocol::SkillsExtraRootsSetParams;
use sofia_app_server_protocol::SkillsExtraRootsSetResponse;
use sofia_app_server_protocol::SkillsListParams;
use sofia_app_server_protocol::SkillsListResponse;
use sofia_app_server_protocol::SortDirection;
use sofia_app_server_protocol::Thread;
use sofia_app_server_protocol::ThreadApproveGuardianDeniedActionParams;
use sofia_app_server_protocol::ThreadApproveGuardianDeniedActionResponse;
use sofia_app_server_protocol::ThreadArchiveParams;
use sofia_app_server_protocol::ThreadArchiveResponse;
use sofia_app_server_protocol::ThreadArchivedNotification;
use sofia_app_server_protocol::ThreadBackgroundTerminal;
use sofia_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use sofia_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use sofia_app_server_protocol::ThreadBackgroundTerminalsListParams;
use sofia_app_server_protocol::ThreadBackgroundTerminalsListResponse;
use sofia_app_server_protocol::ThreadBackgroundTerminalsTerminateParams;
use sofia_app_server_protocol::ThreadBackgroundTerminalsTerminateResponse;
use sofia_app_server_protocol::ThreadClosedNotification;
use sofia_app_server_protocol::ThreadCompactStartParams;
use sofia_app_server_protocol::ThreadCompactStartResponse;
use sofia_app_server_protocol::ThreadDecrementElicitationParams;
use sofia_app_server_protocol::ThreadDecrementElicitationResponse;
use sofia_app_server_protocol::ThreadDeleteParams;
use sofia_app_server_protocol::ThreadDeleteResponse;
use sofia_app_server_protocol::ThreadDeletedNotification;
use sofia_app_server_protocol::ThreadForkParams;
use sofia_app_server_protocol::ThreadForkResponse;
use sofia_app_server_protocol::ThreadGoal;
use sofia_app_server_protocol::ThreadGoalClearParams;
use sofia_app_server_protocol::ThreadGoalClearResponse;
use sofia_app_server_protocol::ThreadGoalClearedNotification;
use sofia_app_server_protocol::ThreadGoalGetParams;
use sofia_app_server_protocol::ThreadGoalGetResponse;
use sofia_app_server_protocol::ThreadGoalSetParams;
use sofia_app_server_protocol::ThreadGoalSetResponse;
use sofia_app_server_protocol::ThreadGoalStatus;
use sofia_app_server_protocol::ThreadGoalUpdatedNotification;
use sofia_app_server_protocol::ThreadHistoryBuilder;
#[cfg(test)]
use sofia_app_server_protocol::ThreadHistoryMode;
use sofia_app_server_protocol::ThreadIncrementElicitationParams;
use sofia_app_server_protocol::ThreadIncrementElicitationResponse;
use sofia_app_server_protocol::ThreadInjectItemsParams;
use sofia_app_server_protocol::ThreadInjectItemsResponse;
use sofia_app_server_protocol::ThreadItem;
use sofia_app_server_protocol::ThreadItemEntry;
use sofia_app_server_protocol::ThreadItemsListParams;
use sofia_app_server_protocol::ThreadItemsListResponse;
use sofia_app_server_protocol::ThreadListCwdFilter;
use sofia_app_server_protocol::ThreadListParams;
use sofia_app_server_protocol::ThreadListResponse;
use sofia_app_server_protocol::ThreadLoadedListParams;
use sofia_app_server_protocol::ThreadLoadedListResponse;
use sofia_app_server_protocol::ThreadMemoryModeSetParams;
use sofia_app_server_protocol::ThreadMemoryModeSetResponse;
use sofia_app_server_protocol::ThreadMetadataGitInfoUpdateParams;
use sofia_app_server_protocol::ThreadMetadataUpdateParams;
use sofia_app_server_protocol::ThreadMetadataUpdateResponse;
use sofia_app_server_protocol::ThreadNameUpdatedNotification;
use sofia_app_server_protocol::ThreadProjectUpdatedNotification;
use sofia_app_server_protocol::ThreadReadParams;
use sofia_app_server_protocol::ThreadReadResponse;
use sofia_app_server_protocol::ThreadRealtimeAppendAudioParams;
use sofia_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use sofia_app_server_protocol::ThreadRealtimeAppendSpeechParams;
use sofia_app_server_protocol::ThreadRealtimeAppendSpeechResponse;
use sofia_app_server_protocol::ThreadRealtimeAppendTextParams;
use sofia_app_server_protocol::ThreadRealtimeAppendTextResponse;
use sofia_app_server_protocol::ThreadRealtimeListVoicesResponse;
use sofia_app_server_protocol::ThreadRealtimeStartParams;
use sofia_app_server_protocol::ThreadRealtimeStartResponse;
use sofia_app_server_protocol::ThreadRealtimeStartTransport;
use sofia_app_server_protocol::ThreadRealtimeStopParams;
use sofia_app_server_protocol::ThreadRealtimeStopResponse;
use sofia_app_server_protocol::ThreadResumeInitialTurnsPageParams;
use sofia_app_server_protocol::ThreadResumeParams;
use sofia_app_server_protocol::ThreadResumeResponse;
use sofia_app_server_protocol::ThreadRollbackParams;
use sofia_app_server_protocol::ThreadSearchOccurrence;
use sofia_app_server_protocol::ThreadSearchOccurrencesParams;
use sofia_app_server_protocol::ThreadSearchOccurrencesResponse;
use sofia_app_server_protocol::ThreadSearchParams;
use sofia_app_server_protocol::ThreadSearchResponse;
use sofia_app_server_protocol::ThreadSearchResult;
use sofia_app_server_protocol::ThreadSearchSortKey;
use sofia_app_server_protocol::ThreadSearchTextRange;
use sofia_app_server_protocol::ThreadSetNameParams;
use sofia_app_server_protocol::ThreadSetNameResponse;
use sofia_app_server_protocol::ThreadSettings;
use sofia_app_server_protocol::ThreadSettingsUpdateParams;
use sofia_app_server_protocol::ThreadSettingsUpdateResponse;
use sofia_app_server_protocol::ThreadShellCommandParams;
use sofia_app_server_protocol::ThreadShellCommandResponse;
use sofia_app_server_protocol::ThreadSortKey;
use sofia_app_server_protocol::ThreadSourceKind;
use sofia_app_server_protocol::ThreadStartParams;
use sofia_app_server_protocol::ThreadStartResponse;
use sofia_app_server_protocol::ThreadStartedNotification;
use sofia_app_server_protocol::ThreadStatus;
use sofia_app_server_protocol::ThreadTimelineListParams;
use sofia_app_server_protocol::ThreadTimelineListResponse;
use sofia_app_server_protocol::ThreadTurnsListParams;
use sofia_app_server_protocol::ThreadTurnsListResponse;
use sofia_app_server_protocol::ThreadUnarchiveParams;
use sofia_app_server_protocol::ThreadUnarchiveResponse;
use sofia_app_server_protocol::ThreadUnarchivedNotification;
use sofia_app_server_protocol::ThreadUnsubscribeParams;
use sofia_app_server_protocol::ThreadUnsubscribeResponse;
use sofia_app_server_protocol::ThreadUnsubscribeStatus;
use sofia_app_server_protocol::Turn;
use sofia_app_server_protocol::TurnEnvironmentParams;
use sofia_app_server_protocol::TurnError;
use sofia_app_server_protocol::TurnInterruptParams;
use sofia_app_server_protocol::TurnInterruptResponse;
use sofia_app_server_protocol::TurnItemsView;
use sofia_app_server_protocol::TurnSettingsUpdateParams;
use sofia_app_server_protocol::TurnSettingsUpdateResponse;
use sofia_app_server_protocol::TurnSettingsUpdateStatus;
use sofia_app_server_protocol::TurnStartParams;
use sofia_app_server_protocol::TurnStartResponse;
use sofia_app_server_protocol::TurnStatus;
use sofia_app_server_protocol::TurnSteerParams;
use sofia_app_server_protocol::TurnSteerResponse;
use sofia_app_server_protocol::UserInput as V2UserInput;
use sofia_app_server_protocol::WindowsSandboxReadiness;
use sofia_app_server_protocol::WindowsSandboxReadinessResponse;
use sofia_app_server_protocol::WindowsSandboxSetupCompletedNotification;
use sofia_app_server_protocol::WindowsSandboxSetupMode;
use sofia_app_server_protocol::WindowsSandboxSetupStartParams;
use sofia_app_server_protocol::WindowsSandboxSetupStartResponse;
use sofia_app_server_protocol::WorkspaceMessage;
use sofia_app_server_protocol::WorkspaceMessageType;
use sofia_arg0::Arg0DispatchPaths;
use sofia_backend_client::AddCreditsNudgeCreditType as BackendAddCreditsNudgeCreditType;
use sofia_backend_client::Client as BackendClient;
use sofia_backend_client::CodexWorkspaceMessage as BackendWorkspaceMessage;
use sofia_backend_client::CodexWorkspaceMessageType as BackendWorkspaceMessageType;
use sofia_backend_client::CodexWorkspaceMessagesResponse as BackendWorkspaceMessagesResponse;
use sofia_backend_client::ConsumeRateLimitResetCreditCode as BackendConsumeRateLimitResetCreditCode;
use sofia_backend_client::RateLimitResetCreditDetails as BackendRateLimitResetCreditDetails;
use sofia_backend_client::RateLimitResetCreditsDetails as BackendRateLimitResetCreditsDetails;
use sofia_backend_client::RequestError as BackendRequestError;
use sofia_backend_client::TokenUsageProfile;
use sofia_chatgpt::connectors;
use sofia_config::CloudConfigBundleLoadError;
use sofia_config::CloudConfigBundleLoadErrorCode;
use sofia_config::ConfigLayerStack;
use sofia_config::loader::project_trust_key;
use sofia_config::types::McpServerTransportConfig;
use sofia_connectors::AppInfo;
use sofia_core::CodexThread;
use sofia_core::CodexThreadSettingsOverrides;
use sofia_core::ForkSnapshot;
use sofia_core::McpManager;
use sofia_core::NewThread;
use sofia_core::NotSubmittedReason;
#[cfg(test)]
use sofia_core::SessionMeta;
use sofia_core::StartThreadOptions;
use sofia_core::SteerSubmission;
use sofia_core::ThreadConfigSnapshot;
use sofia_core::ThreadManager;
use sofia_core::TurnInput;
use sofia_core::TurnInputRequest;
use sofia_core::TurnInputSubmission;
use sofia_core::TurnStartOptions;
use sofia_core::config::Config;
use sofia_core::config::ConfigOverrides;
use sofia_core::config::NetworkProxyAuditMetadata;
use sofia_core::config::edit::ConfigEdit;
use sofia_core::config::edit::ConfigEditsBuilder;
use sofia_core::connectors::AccessibleConnectorsStatus;
use sofia_core::exec::ExecCapturePolicy;
use sofia_core::exec::ExecExpiration;
use sofia_core::exec::ExecParams;
use sofia_core::exec_env::create_env;
use sofia_core::path_utils;
#[cfg(test)]
use sofia_core::read_head_for_summary;
use sofia_core::sandboxing::SandboxPermissions;
use sofia_core::truncate_rollout_after_turn_id;
use sofia_core::truncate_rollout_before_turn_id;
use sofia_core::windows_sandbox::WindowsSandboxLevelExt;
use sofia_core::windows_sandbox::WindowsSandboxSetupMode as CoreWindowsSandboxSetupMode;
use sofia_core::windows_sandbox::WindowsSandboxSetupRequest;
use sofia_core::windows_sandbox::sandbox_setup_is_complete;
use sofia_core_plugins::PluginInstallError as CorePluginInstallError;
use sofia_core_plugins::PluginInstallRequest;
use sofia_core_plugins::PluginReadRequest;
use sofia_core_plugins::PluginUninstallError as CorePluginUninstallError;
use sofia_core_plugins::PluginsManager;
use sofia_core_plugins::loader::load_plugin_apps;
use sofia_core_plugins::manifest::PluginManifestInterface;
use sofia_core_plugins::marketplace::MarketplaceError;
use sofia_core_plugins::marketplace::MarketplacePluginSource;
use sofia_core_plugins::marketplace_add::MarketplaceAddError;
use sofia_core_plugins::marketplace_add::MarketplaceAddRequest;
use sofia_core_plugins::marketplace_add::add_marketplace as add_marketplace_to_codex_home;
use sofia_core_plugins::marketplace_remove::MarketplaceRemoveError;
use sofia_core_plugins::marketplace_remove::MarketplaceRemoveRequest as CoreMarketplaceRemoveRequest;
use sofia_core_plugins::marketplace_remove::remove_marketplace;
use sofia_core_plugins::remote::RemoteMarketplace;
use sofia_core_plugins::remote::RemoteMarketplaceSource;
use sofia_core_plugins::remote::RemotePluginCatalogError;
use sofia_core_plugins::remote::RemotePluginDetail as RemoteCatalogPluginDetail;
use sofia_core_plugins::remote::RemotePluginServiceConfig;
use sofia_core_plugins::remote::RemotePluginShareContext as RemoteCatalogPluginShareContext;
use sofia_core_plugins::remote::RemotePluginShareSummary as RemoteCatalogPluginShareSummary;
use sofia_core_plugins::remote::RemotePluginSummary as RemoteCatalogPluginSummary;
use sofia_exec_server::EnvironmentManager;
use sofia_exec_server::EnvironmentObservedStatus;
use sofia_exec_server::LOCAL_ENVIRONMENT_ID;
use sofia_exec_server::LOCAL_FS;
use sofia_features::FEATURES;
use sofia_features::Feature;
use sofia_features::Stage;
use sofia_feedback::CodexFeedback;
use sofia_feedback::FeedbackAttachmentPath;
use sofia_feedback::FeedbackUploadOptions;
use sofia_git_utils::git_diff_to_remote;
use sofia_git_utils::resolve_root_git_project_for_trust;
use sofia_login::AuthManager;
use sofia_login::CodexAuth;
use sofia_login::LoginSuccessPage;
use sofia_login::LoginSuccessPageBrand;
use sofia_login::SOFIA_OPEN_APP_URL;
use sofia_login::ServerOptions as LoginServerOptions;
use sofia_login::ShutdownHandle;
use sofia_login::complete_device_code_login;
use sofia_login::login_with_api_key;
use sofia_login::login_with_bedrock_api_key;
use sofia_login::oauth_client_id;
use sofia_login::request_device_code;
use sofia_login::run_login_server;
use sofia_mcp::McpRuntimeContext;
use sofia_mcp::McpServerStatusSnapshot;
use sofia_mcp::McpSnapshotDetail;
use sofia_mcp::collect_mcp_server_status_snapshot_with_detail;
use sofia_mcp::discover_supported_scopes;
use sofia_mcp::read_mcp_resource as read_mcp_resource_without_thread;
use sofia_mcp::resolve_oauth_scopes;
use sofia_memories_write::clear_memory_roots_contents;
use sofia_model_provider::create_model_provider;
use sofia_models_manager::collaboration_mode_presets::builtin_collaboration_mode_presets;
use sofia_protocol::ThreadId;
use sofia_protocol::config_types::CollaborationMode;
use sofia_protocol::config_types::ForcedLoginMethod;
use sofia_protocol::config_types::Personality;
use sofia_protocol::config_types::ReasoningSummary;
use sofia_protocol::config_types::TrustLevel;
use sofia_protocol::config_types::WindowsSandboxLevel;
use sofia_protocol::error::CodexErr;
use sofia_protocol::error::Result as CodexResult;
#[cfg(test)]
use sofia_protocol::items::TurnItem;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::openai_models::ReasoningEffort;
use sofia_protocol::protocol::AgentStatus;
use sofia_protocol::protocol::ConversationAudioParams;
use sofia_protocol::protocol::ConversationSpeechParams;
use sofia_protocol::protocol::ConversationStartParams;
use sofia_protocol::protocol::ConversationStartTransport;
use sofia_protocol::protocol::ConversationTextParams;
use sofia_protocol::protocol::EnvironmentConfigState;
use sofia_protocol::protocol::EventMsg;
#[cfg(test)]
use sofia_protocol::protocol::GitInfo as CoreGitInfo;
use sofia_protocol::protocol::McpAuthStatus as CoreMcpAuthStatus;
use sofia_protocol::protocol::Op;
use sofia_protocol::protocol::RealtimeVoicesList;
use sofia_protocol::protocol::ReviewDelivery as CoreReviewDelivery;
use sofia_protocol::protocol::ReviewRequest;
use sofia_protocol::protocol::ReviewTarget as CoreReviewTarget;
use sofia_protocol::protocol::SessionConfiguredEvent;
#[cfg(test)]
use sofia_protocol::protocol::SessionMetaLine;
use sofia_protocol::protocol::TurnEnvironmentSelection;
use sofia_protocol::protocol::TurnEnvironmentSelections;
use sofia_protocol::protocol::W3cTraceContext;
use sofia_protocol::protocol::strip_user_message_prefix;
use sofia_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use sofia_protocol::user_input::UserInput as CoreInputItem;
use sofia_rmcp_client::McpOAuthClientRegistration;
use sofia_rmcp_client::StreamableHttpRedirectMode;
use sofia_rmcp_client::perform_oauth_login_return_url;
use sofia_rollout::InitialHistory;
use sofia_rollout::ResumedHistory;
use sofia_rollout::RolloutItem;
use sofia_rollout::is_persisted_rollout_item;
use sofia_rollout::state_db::StateDbHandle;
use sofia_rollout::state_db::reconcile_rollout;
use sofia_state::ThreadMetadata;
use sofia_state::log_db::LogDbLayer;
use sofia_thread_store::ArchiveThreadParams as StoreArchiveThreadParams;
use sofia_thread_store::ArchiveThreadsParams as StoreArchiveThreadsParams;
use sofia_thread_store::ClearableField as StoreClearableField;
use sofia_thread_store::DeleteThreadsParams as StoreDeleteThreadsParams;
use sofia_thread_store::GitInfoPatch as StoreGitInfoPatch;
use sofia_thread_store::ItemSortKey as StoreItemSortKey;
use sofia_thread_store::ListItemsParams as StoreListItemsParams;
use sofia_thread_store::ListThreadsParams as StoreListThreadsParams;
use sofia_thread_store::ListTimelineParams as StoreListTimelineParams;
use sofia_thread_store::ListTurnsParams as StoreListTurnsParams;
use sofia_thread_store::LoadThreadHistoryParams as StoreLoadThreadHistoryParams;
use sofia_thread_store::LocalThreadStore;
use sofia_thread_store::ReadThreadByRolloutPathParams as StoreReadThreadByRolloutPathParams;
use sofia_thread_store::ReadThreadParams as StoreReadThreadParams;
use sofia_thread_store::SearchThreadOccurrencesParams as StoreSearchThreadOccurrencesParams;
use sofia_thread_store::SearchThreadsParams as StoreSearchThreadsParams;
use sofia_thread_store::SortDirection as StoreSortDirection;
use sofia_thread_store::StoredThread;
use sofia_thread_store::StoredTurn;
use sofia_thread_store::StoredTurnItemsView;
use sofia_thread_store::StoredTurnStatus;
use sofia_thread_store::ThreadMetadataPatch as StoreThreadMetadataPatch;
use sofia_thread_store::ThreadRelationFilter as StoreThreadRelationFilter;
use sofia_thread_store::ThreadSortKey as StoreThreadSortKey;
use sofia_thread_store::ThreadStore;
use sofia_thread_store::ThreadStoreError;
use sofia_utils_absolute_path::AbsolutePathBuf;
use sofia_utils_pty::DEFAULT_OUTPUT_BYTES_CAP;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Error as IoError;
use std::path::Path;
use std::path::PathBuf;
use std::result::Result;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::SemaphorePermit;
use tokio::sync::broadcast;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::DropGuard;
use tokio_util::task::TaskTracker;
use toml::Value as TomlValue;
use tracing::Instrument;
use tracing::error;
use tracing::info;
use tracing::warn;
use uuid::Uuid;

#[cfg(test)]
use sofia_app_server_protocol::ServerRequest;

mod account_processor;
mod apps_processor;
mod bedrock_auth;
mod catalog_processor;
mod command_exec_processor;
mod config_processor;
mod diagnostics;
mod environment_processor;
mod feedback_doctor_report;
mod feedback_processor;
mod feedback_thread_index;
mod fs_processor;
mod git_processor;
mod initialize_processor;
mod marketplace_processor;
mod mcp_event_stream;
mod mcp_processor;
mod persisted_resume_settings;
mod plugins;
mod process_exec_processor;
mod projects;
mod remote_control_processor;
mod search;
mod thread_enrichment;
mod thread_fork_goal;
mod thread_input;
mod thread_processor;
mod thread_queue_processor;
mod thread_sections;
mod token_usage_replay;
mod turn_processor;
mod windows_sandbox_processor;

pub(crate) use account_processor::AccountRequestProcessor;
pub(crate) use apps_processor::AppsRequestProcessor;
pub(crate) use catalog_processor::CatalogRequestProcessor;
pub(crate) use command_exec_processor::CommandExecRequestProcessor;
pub(crate) use config_processor::ConfigRequestProcessor;
pub(crate) use diagnostics::read_server_diagnostics;
pub(crate) use environment_processor::EnvironmentRequestProcessor;
pub(crate) use feedback_processor::FeedbackRequestProcessor;
pub(crate) use fs_processor::FsRequestProcessor;
pub(crate) use git_processor::GitRequestProcessor;
pub(crate) use initialize_processor::InitializeRequestProcessor;
pub(crate) use marketplace_processor::MarketplaceRequestProcessor;
pub(crate) use mcp_event_stream::McpEventStreamReady;
pub(crate) use mcp_event_stream::McpEventStreams;
pub(crate) use mcp_processor::McpRequestProcessor;
pub(crate) use plugins::PluginRequestProcessor;
pub(crate) use process_exec_processor::ProcessExecRequestProcessor;
pub(crate) use projects::ProjectRequestProcessor;
pub(crate) use remote_control_processor::RemoteControlRequestProcessor;
pub(crate) use search::SearchRequestProcessor;
pub(crate) use thread_goal_processor::ThreadGoalRequestProcessor;
pub(crate) use thread_processor::ThreadRequestProcessor;
pub(crate) use thread_queue_processor::ThreadQueueRequestProcessor;
pub(crate) use turn_processor::TurnRequestProcessor;
pub(crate) use windows_sandbox_processor::WindowsSandboxRequestProcessor;

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use crate::filters::compute_source_filters;
use crate::filters::source_kind_matches;
use crate::thread_state::ConnectionCapabilities;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadState;
use crate::thread_state::ThreadStateManager;
use token_usage_replay::restored_token_usage_turn_id;
use token_usage_replay::send_thread_token_usage_update_to_connection;

pub(crate) fn apply_live_thread_settings(
    thread: &mut Thread,
    config_snapshot: &ThreadConfigSnapshot,
) {
    thread.model = Some(config_snapshot.model.clone());
    thread.reasoning_effort = config_snapshot.reasoning_effort.clone();
    thread.environments = Some(
        config_snapshot
            .environment_selections()
            .iter()
            .map(Into::into)
            .collect(),
    );
}

fn resolve_request_cwd(cwd: Option<PathBuf>) -> Result<Option<AbsolutePathBuf>, JSONRPCErrorError> {
    cwd.map(|cwd| {
        AbsolutePathBuf::relative_to_current_dir(path_utils::normalize_for_native_workdir(cwd))
            .map_err(|err| invalid_request(format!("invalid cwd: {err}")))
    })
    .transpose()
}

fn resolve_turn_environment_selections(
    thread_manager: &ThreadManager,
    environments: Option<Vec<TurnEnvironmentParams>>,
) -> Result<Option<Vec<TurnEnvironmentSelection>>, JSONRPCErrorError> {
    let Some(environments) = environments else {
        return Ok(None);
    };
    let mut selections = Vec::with_capacity(environments.len());
    for environment in environments {
        let environment_id = environment.environment_id;
        let cwd = environment
            .cwd
            .to_inferred_path_uri()
            .ok_or_else(|| {
                invalid_request(format!(
                    "invalid cwd for environment `{environment_id}`: path `{}` does not use absolute POSIX or Windows path syntax",
                    environment.cwd
                ))
            })?;
        let workspace_roots = environment
            .runtime_workspace_roots
            .map(|roots| {
                let mut resolved_roots = Vec::new();
                for root in roots {
                    let root = root.to_inferred_path_uri().ok_or_else(|| {
                        invalid_request(format!(
                            "invalid runtime workspace root for environment `{environment_id}`: path `{root}` does not use absolute POSIX or Windows path syntax"
                        ))
                    })?;
                    if !resolved_roots.contains(&root) {
                        resolved_roots.push(root);
                    }
                }
                Ok::<_, JSONRPCErrorError>(resolved_roots)
            })
            .transpose()?
            .unwrap_or_else(|| vec![cwd.clone()]);
        selections.push(TurnEnvironmentSelection {
            environment_id,
            cwd,
            workspace_roots,
            config: EnvironmentConfigState::FromThread,
        });
    }
    thread_manager
        .validate_environment_selections(&selections)
        .map_err(environment_selection_error)?;
    Ok(Some(selections))
}

fn resolve_runtime_workspace_roots(workspace_roots: Vec<AbsolutePathBuf>) -> Vec<AbsolutePathBuf> {
    let mut resolved_roots = Vec::new();
    for root in workspace_roots {
        if !resolved_roots.iter().any(|existing| existing == &root) {
            resolved_roots.push(root);
        }
    }
    resolved_roots
}

mod config_errors;
mod request_errors;
mod thread_delete;
mod thread_goal_processor;
mod thread_lifecycle;
mod thread_resume_redaction;
mod thread_summary;

use self::config_errors::*;
use self::request_errors::*;
use self::thread_goal_processor::api_thread_goal_from_state;
use self::thread_lifecycle::*;
use self::thread_resume_redaction::*;
use self::thread_summary::*;

pub(crate) use self::thread_lifecycle::populate_thread_turns_from_history;
pub(crate) use self::thread_processor::thread_from_stored_thread;
#[cfg(test)]
pub(crate) use self::thread_summary::read_summary_from_rollout;
#[cfg(test)]
pub(crate) use self::thread_summary::summary_to_thread;
pub(crate) use self::thread_summary::thread_settings_from_config_snapshot;

pub(crate) fn build_legacy_api_turns_from_rollout_items(items: &[RolloutItem]) -> Vec<Turn> {
    let mut builder = ThreadHistoryBuilder::new();
    for item in items {
        if is_persisted_rollout_item(item, sofia_protocol::protocol::ThreadHistoryMode::Legacy) {
            builder.handle_rollout_item(item);
        }
    }
    builder.finish()
}
