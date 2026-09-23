<p align="center"><strong>Sofia CLI</strong> is a coding agent from OpenAI that runs locally on your computer.
<p align="center">
  <img src="https://github.com/RuutChatCSM/sofia/blob/main/.github/sofia-cli-splash.png" alt="Sofia CLI splash" width="80%" />
</p>
</br>
If you want Sofia in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/sofia/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>sofia app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Sofia App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Sofia Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

## Quickstart

### Installing and running Sofia CLI

Run the following on Mac or Linux to install Sofia CLI:

```shell
curl -fsSL https://github.com/RuutChatCSM/sofia/releases/latest/download/install.sh | sh
```

Run the following on Windows to install Sofia CLI:

```shell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/RuutChatCSM/sofia/releases/latest/download/install.ps1 | iex"
```

The standalone installers download from this repository's latest GitHub Release by default and fall back to OpenAI's hosted releases (`releases.openai.com`) if GitHub Releases is unavailable. To prefer OpenAI's hosted downloads instead, set `SOFIA_INSTALLER_USE_RELEASES_OPENAI_COM` to `true` (`1` and `yes` are also accepted):

```shell
curl -fsSL https://github.com/RuutChatCSM/sofia/releases/latest/download/install.sh | SOFIA_INSTALLER_USE_RELEASES_OPENAI_COM=true sh
```

```powershell
$env:SOFIA_INSTALLER_USE_RELEASES_OPENAI_COM='true'; irm https://github.com/RuutChatCSM/sofia/releases/latest/download/install.ps1 | iex
```

Sofia CLI can also be installed via the following package managers:

```shell
# Install using npm
npm install -g @ruut/sofia
```

```shell
# Install using Homebrew
brew install --cask sofia
```

Then simply run `sofia` to get started.

<details>
<summary>You can also go to the <a href="https://github.com/RuutChatCSM/sofia/releases/latest">latest GitHub Release</a> and download the appropriate binary for your platform.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `sofia-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `sofia-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `sofia-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `sofia-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `sofia-x86_64-unknown-linux-musl`), so you likely want to rename it to `sofia` after extracting it.

</details>

### Using Sofia with your ChatGPT plan

Run `sofia` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Sofia as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-sofia-in-chatgpt).

You can also use Sofia with an API key, but this requires [additional setup](https://developers.openai.com/sofia/auth#sign-in-with-an-api-key).

## Docs

- [**Sofia Documentation**](https://developers.openai.com/sofia)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
