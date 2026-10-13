# Changelog

Release binaries and notes are published on the [releases page](https://github.com/RuutChatCSM/sofia/releases).

## 0.1.7

### Fixed

- Chat Completions providers can execute JSON-wrapped `apply_patch` calls.
- Historical custom-tool arguments are serialized as strings when resuming with
  Chat Completions providers such as DeepSeek.
- Non-GPT model-name differences no longer produce a false cyber fallback warning.
- macOS notarization uses unique temporary directories for API-key files.

## 0.1.5

Claude models now run over Anthropic's native Messages API with prompt caching,
and model metadata, catalogs, and context-window limits resolve against the
active provider.

### Added

- A native Anthropic Messages API wire (`wire_api = "anthropic"`) so Claude runs
  through the real agent harness — function and freeform tools (including
  `apply_patch`), extended thinking with signed-thinking replay, and correct
  streaming — instead of Anthropic's degraded OpenAI-compatible
  `/chat/completions` shim. Requests carry `x-api-key` and `anthropic-version`.
- Automatic prompt caching for Anthropic requests via a top-level
  `cache_control` breakpoint, so the stable prompt prefix is billed at cache-read
  rates instead of full input rates.
- A one-shot recovery when Anthropic rejects a request because `max_tokens`
  exceeds the model's output cap: the turn retries with a reduced cap instead of
  failing.

### Fixed

- Switching providers on a live session rebuilds the destination provider's
  model catalog, so model metadata — and with it the whole tool plan, including
  web search — resolves against the provider the session is actually on.
- A `provider/model` slug only switches providers when the prefix names a
  configured provider, so model names that contain a slash (for example
  `stealth/space-bunny-alpha`) keep the active provider and still resolve on
  resume or fork.
- Unknown (BYOK / models.dev) models get the `apply_patch` tool again instead of
  silently rewriting files through the shell.
- A global context-window override is clamped to the provider's advertised
  window, and provider context-window-overflow errors recover with a mid-turn
  auto-compaction instead of failing the turn.
- Inter-agent Chat Completions messages are attributed to their author, and
  Chat Completions function arguments are marked as plaintext.

### Changed

- A custom model catalog is merged over the bundled catalog instead of replacing
  it, preserving reasoning presets, guardian policy, and instruction templates
  for known models.
- Sessions reuse the thread manager's model catalog when they resolve against
  the same provider and static catalog, so catalog refreshes stay visible to
  live sessions.
