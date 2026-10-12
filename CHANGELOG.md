# Changelog

Release binaries and notes are published on the [releases page](https://github.com/RuutChatCSM/sofia/releases).

## 0.1.5

Model metadata, catalogs, and context-window limits now resolve against the
active provider, and Anthropic runs over Chat Completions.

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
- Anthropic is wired over Chat Completions rather than `/v1/responses`.
- Sessions reuse the thread manager's model catalog when they resolve against
  the same provider and static catalog, so catalog refreshes stay visible to
  live sessions.
