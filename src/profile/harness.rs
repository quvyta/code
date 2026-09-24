//! The harnesses QCode can install into a profile image, and what each one needs.
//!
//! Every field below is taken from the harness's own documentation or source, never from
//! memory, and then checked against the installed package in a container built from the base
//! image; the comment above each record says where the fact was read and what the installed
//! version said, and `harness_live.rs` is the check. A harness whose login path could not be
//! established does not belong here: a wrong path silently loses a login, a missing harness
//! only limits the list.
//!
//! On the installs: npm 11 warns about a package's install scripts and runs them anyway. All
//! four command-line harnesses installed clean with the plain command, and the two that need a
//! script (Claude Code and opencode copy a native binary over a stub in `postinstall`) answered
//! afterwards, so no `--allow-scripts` is written here; the live test is what would notice if npm
//! stopped running them.
//!
//! One harness here opens a window instead of drawing in a terminal. Its record carries a
//! [`Desktop`] beside the fields every harness has, and the fields that only mean something in a
//! terminal — the registry install, the unattended argument, the login file QCode carries, the
//! resume argument — are empty for it. [`HarnessKind::TERMINAL`] is the list to walk when a rule
//! is about those.

/// A harness QCode can build a profile image for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessKind {
    /// Claude Code.
    ClaudeCode,
    /// opencode.
    OpenCode,
    /// Gemini CLI.
    GeminiCli,
    /// Codex CLI.
    Codex,
    /// Kimi Code CLI.
    KimiCode,
    /// Qwen Code.
    QwenCode,
    /// Antigravity IDE, which opens a window instead of drawing in a terminal.
    AntigravityIde,
}

/// What a profile signs in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    /// Nothing: the harness runs on models its makers offer for free, without an account. There
    /// is no login to make, store or carry.
    Free,
    /// A plan the user logs in to in a browser.
    Subscription,
    /// A key the user pastes in.
    ApiKey,
    /// A login the person makes inside the harness's own window, with nothing for QCode to
    /// make, store or carry: the harness writes it into the workspace's home volume itself and
    /// finds it there again. QCode's sign-in container and credential volume have no part in it.
    InApp,
    /// A provider the person added on the Providers page, named in the profile by its tag and a
    /// model of its own. Nothing for QCode's sign-in container to make either: the key already
    /// lives in `providers.toml`, and a tab of this profile is pointed at the workspace's relay
    /// instead of the provider's own address, so the key never enters the container.
    Provider,
}

/// A configuration file a template writes into the image, by its path under the home directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFile {
    /// Where the file goes, relative to the harness's home directory in the container.
    pub path: &'static str,
    /// What the file holds, written as the harness expects to read it.
    pub contents: &'static str,
}

/// A file the image carries outside the home directory, written as root and readable by everyone:
/// the harness reads it on every machine the image runs on, and no home volume a workspace keeps
/// can hold an older copy of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemFile {
    /// Where the file goes, as an absolute path in the container.
    pub path: &'static str,
    /// What the file holds, written as the harness expects to read it.
    pub contents: &'static str,
}

/// Where a harness draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// In the tab's own terminal, which is where every command-line harness draws.
    Terminal,
    /// In a window of its own on the person's desktop, opened by a container. The tab has no
    /// terminal then; it says where the window stands and offers the two things that can be done
    /// to it.
    Desktop(&'static Desktop),
}

/// A harness that opens a window: where its application comes from, what the image needs beside
/// the base image to run it, and how the window is started.
///
/// The application is never carried inside a QCode image. Its terms permit running it, not
/// redistributing it, so the image is built on the person's own machine and fetches the archive
/// from the maker's address at install time, exactly as a command-line harness is installed from
/// its own registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Desktop {
    /// The version this record was read from and checked against.
    pub version: &'static str,
    /// Where the archive is downloaded from while the image is built.
    pub archive: &'static str,
    /// How many bytes that archive is, as the server reports its length.
    pub bytes: u64,
    /// The SHA-256 of the archive, so an image is never built from something else that answered
    /// at the same address.
    pub sha256: &'static str,
    /// Where the archive unpacks to in the image, the one directory inside it stripped away.
    pub install_dir: &'static str,
    /// The program that opens the window, by its name under [`Desktop::install_dir`].
    ///
    /// The archive's own launcher script is not used: it runs the real program through `sh`, and
    /// a container whose first process is that shell never passes the stop signal on, so every
    /// stop costs the engine's ten-second timeout and a kill.
    pub program: &'static str,
    /// The arguments the window is always opened with.
    pub flags: &'static [&'static str],
    /// The Debian packages the application needs beside what the base image brings.
    pub packages: &'static [&'static str],
    /// How much room the built image takes, in whole mebibytes, as it was measured **on top of
    /// `qcode/base`**, which is what the recipe builds on. The person is told this before they
    /// ask for the image: a desktop image is an order of magnitude larger than a command-line
    /// harness's.
    ///
    /// Measure it the way the recipe builds it, not on a bare Debian: the first number here was
    /// taken on `debian:trixie-slim` during the trial and understated the image by 900 MiB,
    /// which is a promise broken before the download even starts. The live test
    /// `the_image_builds_from_the_makers_archive_and_carries_the_application_and_its_settings`
    /// prints the built size, so a re-measure is one run away.
    pub image_mib: u64,
}

impl Desktop {
    /// The program that opens the window, as an absolute path in the image.
    #[must_use]
    pub fn command(&self) -> String {
        format!("{}/{}", self.install_dir, self.program)
    }

    /// The whole line that opens the window on the workspace at `workspace`: the program, the
    /// arguments it always takes, and the folder to open.
    #[must_use]
    pub fn command_line(&self, workspace: &str) -> Vec<String> {
        let mut line = vec![self.command()];
        line.extend(self.flags.iter().map(|flag| (*flag).to_owned()));
        line.push(workspace.to_owned());
        line
    }
}

/// Where a harness reads the MCP servers it starts in every workspace, and in which shape.
///
/// These are the user-level settings, the ones a harness reads without asking for trust or
/// approval, because QCode registers its bridge between tabs there (see [`crate::bridge`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpSettings {
    /// Where the file is, relative to the harness's home directory in the container.
    pub path: &'static str,
    /// How a server is written into it.
    pub shape: McpShape,
}

/// How a harness writes the servers of its settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpShape {
    /// JSON, servers under `mcpServers` with `type`, `command` and `args`.
    Claude,
    /// JSON, servers under `mcp` with `type` and one `command` list.
    OpenCode,
    /// JSON, servers under `mcpServers` with `command`, `args` and `trust`.
    Gemini,
    /// TOML, one `[mcp_servers.<name>]` table per server with `command` and `args`.
    Codex,
    /// JSON, servers under `mcpServers` with `command` and `args` alone: a server with a `command`
    /// is read as one started on standard input and output, so no `type` is needed, and there is
    /// no `trust` to give.
    Kimi,
    /// JSON, servers under `mcpServers` with `command`, `args` and `env`. Its schema names every
    /// field a server may have and takes no other, so neither Claude Code's `type` nor Gemini
    /// CLI's `trust` belongs in this file.
    Antigravity,
}

/// Everything QCode needs to know about one harness to build its image, start it and carry its
/// login from one volume to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Harness {
    /// How the harness is written in definition files.
    pub id: &'static str,
    /// The name the harness gives itself, shown as it writes it.
    pub display_name: &'static str,
    /// The account types a new profile of the harness can sign in with.
    pub accounts: &'static [AccountKind],
    /// Account types the harness's makers closed to most people after profiles were made with
    /// them. A profile that has one still loads, because the few for whom it still works would
    /// otherwise lose it, but no new profile is offered it and loading it says why.
    pub withdrawn: &'static [AccountKind],
    /// The shell commands that install the harness into the image. A harness that opens a window
    /// has none: what its image is made of is in [`Desktop`], which the recipe reads instead.
    pub install: &'static [&'static str],
    /// The program that starts the harness in the container.
    pub command: &'static str,
    /// The arguments that let the harness work without asking for permission. The container is
    /// the isolation, so they are always passed. A harness that opens a window has none: it asks
    /// the person, in its own window, and the arguments its window needs are in [`Desktop`].
    pub auto_run: &'static [&'static str],
    /// Environment variables the container must set for the harness to behave as described here.
    pub environment: &'static [(&'static str, &'static str)],
    /// The files the login lives in, relative to the home directory. Each one is a file, not a
    /// directory, so a copy never drags settings along with the login.
    ///
    /// Empty for a harness whose login QCode does not carry: one signed in to inside its own
    /// window ([`AccountKind::InApp`]) writes its login into the workspace's home volume itself,
    /// and there is no sign-in container it could be taken out of.
    pub identity: &'static [&'static str],
    /// The configuration the `recommended` template writes, when the harness reads one.
    pub settings: Option<ConfigFile>,
    /// What the harness keeps of the questions it asks on its very first start, already
    /// answered, so that a tab opens on its prompt instead of on a question whose highlighted
    /// answer leaves. Written by both QCode templates; `None` for a harness that asks nothing.
    pub first_start: Option<ConfigFile>,
    /// How the harness is told to open a conversation it had before, by that conversation's id;
    /// `None` for one that keeps no conversations QCode can list.
    pub resume: Option<Resume>,
    /// Where the harness draws.
    pub surface: Surface,
    /// Where the harness reads the MCP servers it starts; `None` for one that reads none.
    pub mcp: Option<McpSettings>,
    /// What keeps the harness's own sign-in to an API key, in the image of a profile whose
    /// account is [`AccountKind::ApiKey`]; `None` for a harness that offers nothing else there,
    /// or cannot be told to.
    pub key_only: Option<SystemFile>,
}

/// Where npm keeps what it downloads while an install step of an image runs, and where nothing is
/// left once that step is over.
pub const BUILD_NPM_CACHE: &str = "/tmp/qcode-npm-cache";

impl Harness {
    /// The install as the `RUN` steps of an image, one per command in [`Harness::install`].
    ///
    /// Each step gives npm a cache of its own and removes it before the step ends, so the layer
    /// the step leaves never holds npm's downloads: left in the base image's shared cache
    /// (`NPM_CONFIG_CACHE=/var/cache/npm`) they were measured at 323 MB of opencode's image, all
    /// of it tarballs of what the same layer already holds unpacked.
    ///
    /// A private directory rather than emptying the shared one afterwards: the shared cache is
    /// made by the base image, open to every user, for whatever npm fetches while a container
    /// runs, and a step that removed and remade it would have to know and repeat how the base
    /// image made it. The variable is exported rather than written before the command, so it
    /// holds for the whole step and not only for its first command.
    #[must_use]
    pub fn image_steps(&self) -> Vec<String> {
        self.install
            .iter()
            .map(|step| {
                format!("RUN export NPM_CONFIG_CACHE={BUILD_NPM_CACHE} \\\n && {step} \\\n && rm -rf {BUILD_NPM_CACHE}")
            })
            .collect()
    }
}

/// How a harness opens an earlier conversation from its command line.
///
/// Each form was checked in a throwaway container against the version its record names: the
/// line [`HarnessKind::command_line`] builds was run with an id no conversation has, and every
/// harness got past its argument parser to its own "no such conversation" answer (Codex, which
/// wants a terminal before it looks, got past its parser to that complaint instead). An argument
/// the parser did not know was refused on the same line, so reaching the lookup is the proof
/// that the whole line was read as meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// An option that takes the id, written after the unattended-mode arguments:
    /// `claude --dangerously-skip-permissions --resume <id>`.
    Option(&'static str),
    /// A subcommand that takes the id, written straight after the program so that the
    /// unattended-mode arguments are read as the subcommand's own:
    /// `codex resume --dangerously-bypass-approvals-and-sandbox <id>`.
    Subcommand(&'static str),
}

impl HarnessKind {
    /// Every harness, in the order the profile wizard offers them. The ones that draw in a
    /// terminal come first, because that is what nearly every profile is.
    pub const ALL: [Self; 7] = [
        Self::ClaudeCode,
        Self::OpenCode,
        Self::GeminiCli,
        Self::Codex,
        Self::KimiCode,
        Self::QwenCode,
        Self::AntigravityIde,
    ];

    /// The harnesses that draw in the tab's own terminal, in the same order.
    ///
    /// Most of what QCode knows about a harness — the install from a registry, the unattended
    /// arguments, the conversation script, the login file it carries — is only true of these.
    pub const TERMINAL: [Self; 6] =
        [Self::ClaudeCode, Self::OpenCode, Self::GeminiCli, Self::Codex, Self::KimiCode, Self::QwenCode];

    /// What QCode knows about this harness.
    #[must_use]
    pub fn record(self) -> &'static Harness {
        match self {
            Self::ClaudeCode => &CLAUDE_CODE,
            Self::OpenCode => &OPENCODE,
            Self::GeminiCli => &GEMINI_CLI,
            Self::Codex => &CODEX,
            Self::KimiCode => &KIMI_CODE,
            Self::QwenCode => &QWEN_CODE,
            Self::AntigravityIde => &ANTIGRAVITY_IDE,
        }
    }

    /// The window this harness opens, when it opens one rather than drawing in a terminal.
    #[must_use]
    pub fn desktop(self) -> Option<&'static Desktop> {
        match self.record().surface {
            Surface::Terminal => None,
            Surface::Desktop(desktop) => Some(desktop),
        }
    }

    /// The harness written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|harness| harness.record().id == id)
    }

    /// Whether the harness can sign in with `account`.
    #[must_use]
    pub fn supports(self, account: AccountKind) -> bool {
        self.record().accounts.contains(&account) || self.withdrawn(account)
    }

    /// Whether `account` is one the harness no longer offers to new profiles.
    #[must_use]
    pub fn withdrawn(self, account: AccountKind) -> bool {
        self.record().withdrawn.contains(&account)
    }

    /// The program and arguments a tab runs inside the profile's container: the harness in
    /// unattended mode, opening `conversation` when one is given and a new one otherwise.
    ///
    /// An id that [`history::is_safe_id`](super::history::is_safe_id) would not have let through
    /// is not passed on: a word starting with `-` would be read as an option, so the harness is
    /// started as if none had been asked for rather than with an argument nobody chose. Nor is one
    /// given to a harness that has no way of being told to open a conversation again.
    #[must_use]
    pub fn command_line(self, conversation: Option<&str>) -> Vec<String> {
        let record = self.record();
        let auto_run = record.auto_run.iter().map(|arg| (*arg).to_owned());
        let mut line = vec![record.command.to_owned()];
        match conversation.filter(|id| super::history::is_safe_id(id)).zip(record.resume) {
            None => line.extend(auto_run),
            Some((id, Resume::Option(option))) => {
                line.extend(auto_run);
                line.extend([option.to_owned(), id.to_owned()]);
            }
            Some((id, Resume::Subcommand(command))) => {
                line.push(command.to_owned());
                line.extend(auto_run);
                line.push(id.to_owned());
            }
        }
        line
    }
}

impl AccountKind {
    /// Every account type. The wizard offers each harness's own list, in that harness's order.
    pub const ALL: [Self; 5] = [Self::Free, Self::Subscription, Self::ApiKey, Self::InApp, Self::Provider];

    /// How the account type is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Subscription => "subscription",
            Self::ApiKey => "api-key",
            Self::InApp => "in-app",
            Self::Provider => "provider",
        }
    }

    /// The account type written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|account| account.id() == id)
    }

    /// Whether QCode has a login to make for a profile with this account. One that has none is
    /// ready as soon as its image is, and nothing about it waits for a credentials volume.
    ///
    /// An in-app login is none of QCode's: the person makes it inside the harness's own window,
    /// where it lands in the workspace's home volume. There is nothing for the sign-in container to
    /// capture and nothing to carry from workspace to workspace, so the profile is ready the moment
    /// its image is, exactly like one that signs in to nothing.
    ///
    /// A provider is the same: its key already lives in `providers.toml`, added on the Providers
    /// page long before this profile existed, so there is nothing for a sign-in container to
    /// capture either.
    #[must_use]
    pub fn needs_login(self) -> bool {
        matches!(self, Self::Subscription | Self::ApiKey)
    }
}

/// Claude Code. Install, start command and credential file from the Claude Code documentation
/// (`code.claude.com/docs/en/setup`, `code.claude.com/docs/en/iam`: "On Linux, credentials are
/// stored in `~/.claude/.credentials.json`"); the argument and the settings key from
/// `code.claude.com/docs/en/cli-reference` and `code.claude.com/docs/en/settings-reference`.
///
/// Checked against 2.1.275 in the image. The program is one binary
/// (`node_modules/@anthropic-ai/claude-code-linux-x64/claude`), whose text holds
/// `storagePath:z(e,".credentials.json")` next to the configuration directory and the line
/// `dangerously-skip-permissions / --permission-mode bypassPermissions / a settings defaultMode
/// of bypassPermissions`. `claude auth status` reads the file: with one placed at
/// `~/.claude/.credentials.json` it answers `"loggedIn": true`. `claude doctor` reads the
/// settings file and names it under `Invalid settings` when it is broken; with the file below
/// it finds nothing to say.
///
/// Resuming, from `code.claude.com/docs/en/cli-reference` (`-r, --resume` takes a session id)
/// and checked against 2.1.276: `claude --dangerously-skip-permissions --resume <id> --print hi`
/// with an id no transcript has answers `No conversation found with session ID: <id>`, and with
/// the id [`history`](super::history) lists for a transcript it goes on to `Not logged in`.
///
/// MCP servers, from `code.claude.com/docs/en/mcp` (user scope lives in `~/.claude.json`, under
/// `mcpServers`) and checked against 2.1.278: with the entry [`crate::bridge::config`] writes,
/// `claude mcp list` checks the server's health and prints `qcode: node … - ✔ Connected`, and
/// the harness keeps the entry when it rewrites the file at its next start.
///
/// First start, checked against 2.1.278 on a terminal in a container at `/work`: a fresh home
/// asks for a text style, shows its security notes, then asks whether the folder is trusted and
/// whether the permission mode it was started in is accepted — the last two with "No, exit"
/// highlighted, so a person pressing Return leaves. Answered by hand, it writes
/// `hasCompletedOnboarding` and `projects["/work"].hasTrustDialogAccepted` into `~/.claude.json`
/// and `theme` and `skipDangerousModePermissionPrompt` into `~/.claude/settings.json`. With the
/// files below and no key pressed it draws its prompt at once; without
/// `skipDangerousModePermissionPrompt` the permission warning comes back, without
/// `hasTrustDialogAccepted` the folder question, and without `hasCompletedOnboarding` the text
/// style. `theme` is not needed: the style question belongs to the onboarding that key closes.
static CLAUDE_CODE: Harness = Harness {
    id: "claude-code",
    display_name: "Claude Code",
    // A provider is offered by `ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN`, read once at
    // start (see `ProviderChoice::environment`), and was proven in a container with no network
    // on an ollama server and on OpenRouter. Codex and Gemini CLI are not offered one: neither
    // was ever run through the relay, and offering it to them would be a claim nobody checked.
    accounts: &[AccountKind::Subscription, AccountKind::ApiKey, AccountKind::Provider],
    withdrawn: &[],
    install: &["npm install -g @anthropic-ai/claude-code"],
    command: "claude",
    auto_run: &["--dangerously-skip-permissions"],
    environment: &[],
    identity: &[".claude/.credentials.json"],
    settings: Some(ConfigFile {
        path: ".claude/settings.json",
        contents: "{\n  \"permissions\": {\n    \"defaultMode\": \"bypassPermissions\"\n  },\n  \"skipDangerousModePermissionPrompt\": true\n}\n",
    }),
    first_start: Some(ConfigFile { path: ".claude.json", contents: CLAUDE_FIRST_START }),
    resume: Some(Resume::Option("--resume")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".claude.json", shape: McpShape::Claude }),
    key_only: None,
};

/// Claude Code's answers to its first-start questions, for the workspace mounted at
/// [`CODE_DIR`](crate::base::paths::CODE_DIR). The same file is where the bridge registers its
/// server, and it merges into what it finds, so these keys stay.
const CLAUDE_FIRST_START: &str = "{\n  \"hasCompletedOnboarding\": true,\n  \"projects\": {\n    \"/work\": {\n      \"hasTrustDialogAccepted\": true\n    }\n  }\n}\n";

/// opencode. Install and start command from the opencode documentation (`opencode.ai/docs`), the
/// argument from `opencode.ai/docs/cli` and `opencode.ai/docs/permissions`, the configuration
/// file from `opencode.ai/docs/config`, and the login file from the source: `auth/index.ts`
/// keeps it at `auth.json` under `Global.Path.data`, which `core/src/global.ts` resolves to the
/// XDG data directory.
///
/// Checked against 1.18.31 in the image. The program is one binary
/// (`node_modules/opencode-linux-x64/bin/opencode`), whose text holds
/// `process.env.XDG_DATA_HOME; if(X)return X7.join(X,"opencode","auth.json"); return
/// X7.join($,".local","share","opencode","auth.json")`. `opencode --help` lists `--auto
/// auto-approve permissions that are not explicitly denied` under the default command, the one
/// that opens the interface. `opencode providers list` prints `Credentials
/// ~/.local/share/opencode/auth.json` and counts a file placed there, and `opencode debug
/// config` prints the resolved configuration with the permission below in it.
///
/// Free use comes first: opencode starts and works without any login, on the free models it
/// offers itself, so a profile that signs in to nothing is a complete one here.
///
/// Resuming, from `opencode.ai/docs/cli` (`-s, --session` continues a session by id) and
/// checked against 1.18.31: `opencode --auto --session <id>` with an id no session has answers
/// `Error: Session not found: <id>`, and with the id [`history`](super::history) lists for a
/// session made by `opencode run` it opens the interface.
///
/// MCP servers, from `opencode.ai/docs/mcp-servers` (`mcp` in the configuration, a `local`
/// server by one `command` list) and checked against 1.18.31: with the entry written into the
/// file the template writes, `opencode mcp list` starts it and prints `✓ qcode connected`.
static OPENCODE: Harness = Harness {
    id: "opencode",
    display_name: "opencode",
    // A provider is offered by an OpenAI-compatible provider handed over in
    // `OPENCODE_CONFIG_CONTENT` (see `ProviderChoice::environment`), proven in a container with
    // no network on an ollama server.
    accounts: &[AccountKind::Free, AccountKind::Subscription, AccountKind::ApiKey, AccountKind::Provider],
    withdrawn: &[],
    install: &["npm install -g opencode-ai"],
    command: "opencode",
    auto_run: &["--auto"],
    environment: &[],
    identity: &[".local/share/opencode/auth.json"],
    settings: Some(ConfigFile {
        path: ".config/opencode/opencode.json",
        contents: "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"*\": \"allow\"\n  }\n}\n",
    }),
    first_start: None,
    resume: Some(Resume::Option("--session")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".config/opencode/opencode.json", shape: McpShape::OpenCode }),
    key_only: None,
};

/// Gemini CLI. Install and start command from the workspace's readme, the argument from
/// `docs/cli/cli-reference.md` (`--yolo` is deprecated in favour of `--approval-mode=yolo`), and
/// the login files from the source: `services/fileKeychain.ts` writes `gemini-credentials.json`
/// under `~/.gemini` when no OS keyring answers, `config/storage.ts` keeps
/// `google_accounts.json` beside it, and `services/keychainService.ts` reads
/// `GEMINI_FORCE_FILE_STORAGE` to skip the keyring, which a container has none of.
///
/// Checked against 0.60.0 in the image, whose `bundle/chunk-*.js` carries the source with its
/// file names in comments. `packages/core/dist/src/services/fileKeychain.js`: `const configDir
/// = path23.join(homedir(), GEMINI_DIR); this.tokenFilePath = path23.join(configDir,
/// "gemini-credentials.json")`, with `var GEMINI_DIR = ".gemini"` and `homedir()` answering
/// `GEMINI_CLI_HOME` or the user's home. `packages/core/dist/src/config/storage.js`:
/// `getGoogleAccountsPath() { return path4.join(_Storage.getGlobalGeminiDir(),
/// GOOGLE_ACCOUNTS_FILENAME) }` with `GOOGLE_ACCOUNTS_FILENAME = "google_accounts.json"`.
/// `oauth_creds.json`, which the same file names, is the store of older versions and is only
/// read to migrate. `packages/core/dist/src/services/keychainService.js`: `const
/// forceFileStorage = process.env[FORCE_FILE_STORAGE_ENV_VAR] === "true"; const nativeKeychain
/// = forceFileStorage ? null : await this.getNativeKeychain()`. The keyring module is there but
/// does not load in the image (`Failed to load keytar native addon`), so the file store would
/// be chosen anyway; the variable makes that a decision rather than an accident.
/// `gemini --help` lists `--approval-mode` with the choices `default, auto_edit, yolo, plan`,
/// and a value outside them is refused before anything else runs.
///
/// The settings exist because of one line in `packages/cli/src/config/config.ts` of the same
/// build: `if (!trustedFolder && approvalMode !== "default") { debugLogger.warn('Approval mode
/// overridden to "default" because the current folder is not trusted.'); approvalMode =
/// "default" }`, with `isFolderTrustEnabled` in `packages/cli/src/config/trustedFolders.ts`
/// reading `settings.security?.folderTrust?.enabled ?? true`. The workspace directory of a fresh container is not trusted, so without the file
/// below the argument is taken and then undone. The documentation's `docs/cli/settings.md`
/// lists the key with its default of `true`. Under the `base` template the harness asks once,
/// in its own trust dialog, and keeps the answer in `~/.gemini/trustedFolders.json`.
///
/// The login file is encrypted with a key made from the machine name and the user name, in
/// the same `fileKeychain.js` (the bundle numbers the `os` import): `deriveEncryptionKey() {
/// const salt = `${os15.hostname()}-${os15.userInfo().username}-gemini-cli`; return
/// crypto4.scryptSync("gemini-cli-oauth", salt, 32); }`. A copy of the file only decrypts
/// where both halves are the same as where it was written. That is why every container QCode
/// creates gets the one machine name in [`crate::engine::names::HOSTNAME`], and why the user
/// inside is always `qcode`; the live test checks the salt is still made of those two.
///
/// Resuming, from `geminicli.com/docs/cli/session-management` (`--resume` takes `latest`, an
/// index or a session id) and checked against 0.60.0, whose `SessionSelector.findSession` in
/// the bundle matches the argument against each session's `id` before trying it as an index:
/// `gemini --approval-mode=yolo --resume <id> --prompt hi` with an id the workspace's
/// conversations do not have answers `Error resuming session: Invalid session identifier`, and
/// with the id [`history`](super::history) lists it goes on to ask for an auth method.
///
/// Google stopped serving Gemini CLI to personal Google accounts (Code Assist for individuals and
/// the paid Pro and Ultra plans) and removed "Login with Google" for them on 2026-06-18
/// (`developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals`). Code
/// Assist Standard and Enterprise still sign in that way, so a profile made with a sign-in keeps
/// loading; a new one is offered an API key. On 2026-09-23 a personal account signing in from
/// 0.60.0 was answered "This client is no longer supported for Gemini Code Assist for
/// individuals. To continue using Gemini, please migrate to the Antigravity suite of products".
///
/// The harness's own sign-in dialog still offers Google first, so an API-key profile's image
/// keeps it to the key, read from 0.60.0 and then watched in a container. `AuthDialog.tsx` in
/// `interactiveCli-*.js` lists "Sign in with Google", "Use Gemini API Key" and "Vertex AI", then
/// `if (settings.merged.security.auth.enforcedType) { items = items.filter((item) => item.value
/// === settings.merged.security.auth.enforcedType) }`, with `AuthType.USE_GEMINI =
/// "gemini-api-key"`. The file is the system settings, `getSystemSettingsPath()` answering
/// `/etc/gemini-cli/settings.json` on Linux, which is merged above the user's: it is in the image
/// rather than in `~/.gemini/settings.json` because that one lives in each workspace's home
/// volume, where an image built later never reaches, and because the `base` template writes
/// none. `selectedType` is left out on purpose: set with no key stored, the start answers
/// "Authenticated with gemini-api-key" and asks nothing, while without it the dialog opens. Run
/// in a pty with the file in place, a fresh home showed "How would you like to authenticate for
/// this project? ● 1. Use Gemini API Key" and nothing else, then "Enter Gemini API Key … You can
/// get an API key from https://aistudio.google.com/app/apikey". The key typed there went through
/// `saveApiKey` in `core/apiKeyCredentialStorage.js` (service `gemini-cli-api-key`) into the file
/// keychain, which is `.gemini/gemini-credentials.json`: decrypted with the salt below it held
/// the one entry `gemini-cli-api-key` with the key typed, and the next start answered
/// "Authenticated with gemini-api-key" without asking. So the first file of [`Harness::identity`]
/// is where an API key lands too, and `store_login` finds it there. The dialog also wrote
/// `security.auth.selectedType` into the user's settings, beside the template's.
///
/// MCP servers, from `geminicli.com/docs/tools/mcp-server` (`mcpServers` in `settings.json`,
/// `trust` skips the confirmation of each call) and checked against 0.60.0: `gemini mcp list`
/// starts the server and prints `✓ qcode: node … (stdio) - Connected`. It prints `Disabled`
/// instead in a folder nobody trusted, because the harness then suppresses user-level servers
/// too; the `recommended` template's settings turn folder trust off, and under `base` the
/// person's own answer in the harness's trust dialog decides.
static GEMINI_CLI: Harness = Harness {
    id: "gemini-cli",
    display_name: "Gemini CLI",
    accounts: &[AccountKind::ApiKey],
    withdrawn: &[AccountKind::Subscription],
    install: &["npm install -g @google/gemini-cli"],
    command: "gemini",
    auto_run: &["--approval-mode=yolo"],
    environment: &[("GEMINI_FORCE_FILE_STORAGE", "true")],
    identity: &[".gemini/gemini-credentials.json", ".gemini/google_accounts.json"],
    settings: Some(ConfigFile {
        path: ".gemini/settings.json",
        contents: "{\n  \"security\": {\n    \"folderTrust\": {\n      \"enabled\": false\n    }\n  }\n}\n",
    }),
    first_start: None,
    resume: Some(Resume::Option("--resume")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".gemini/settings.json", shape: McpShape::Gemini }),
    key_only: Some(SystemFile {
        path: "/etc/gemini-cli/settings.json",
        contents: "{\n  \"security\": {\n    \"auth\": {\n      \"enforcedType\": \"gemini-api-key\"\n    }\n  }\n}\n",
    }),
};

/// Codex CLI. Install and start command from the workspace's readme, the argument from the source
/// (`codex-rs/cli`), and the login file from the source as well: `login/src/auth/storage.rs`
/// reads and writes `auth.json` under `CODEX_HOME`, and `config/src/types.rs` makes the file the
/// default store. `core/src/config` documents `CODEX_HOME` as `~/.codex` unless it is set. The
/// settings keys are from the Codex configuration documentation.
///
/// Checked against 0.155.0 in the image, where `codex --version` prints `codex-cli 0.155.0`.
/// The program is one binary (`node_modules/@openai/codex-linux-x64/vendor/
/// x86_64-unknown-linux-musl/bin/codex`); its text holds `auth.json`, `CODEX_HOME`,
/// `approval_policy`, `sandbox_mode` and `danger-full-access`, and its `--help` says the
/// configuration is "loaded from `~/.codex/config.toml`". `codex login status` answers `Not
/// logged in` and, with a file placed at `~/.codex/auth.json`, `Logged in using an API key`;
/// `codex doctor` prints `auth file ~/.codex/auth.json`. The argument parser refuses unknown
/// arguments, so `--version` behind the one below shows it is known. The file below is read by
/// every command: a wrong value in it is answered with `Error loading configuration`, and with
/// it in place `codex doctor` reports `unrestricted fs + enabled network · approval Never`
/// where it reported `restricted fs + restricted network · approval OnRequest` before.
///
/// Resuming, from the `resume` subcommand in `codex-rs/cli` of `github.com/openai/codex`
/// (`codex resume [OPTIONS] [SESSION_ID] [PROMPT]`) and checked against 0.155.0. `codex resume --help` lists
/// `--dangerously-bypass-approvals-and-sandbox` among the subcommand's own options, so the
/// argument follows `resume` rather than going before it, where it would be the top-level
/// command's and reach the resumed session only through however that version hands it down.
/// `codex resume --dangerously-bypass-approvals-and-sandbox <id>` is parsed and stops at
/// `Error: stdin is not a terminal`; an unknown argument in the same place is refused with
/// `error: unexpected argument`. Given a terminal the interface waits for the terminal's answers
/// before it looks the id up, so the lookup was checked through the same resume code without
/// one: `codex exec resume <id> hi` answers `no rollout found for thread id <id>` for an unknown
/// id, and for the id [`history`](super::history) lists it prints `session id: <id>` and goes
/// on to the model.
///
/// MCP servers, from `developers.openai.com/codex/mcp` (`[mcp_servers.<name>]` in
/// `config.toml`) and checked against 0.155.1: `codex mcp get qcode` reads the table back as
/// `enabled: true, transport: stdio`, and `codex exec` starts the server even before it finds
/// there is no login. Codex hands a server only a short list of variables, so the server finds
/// its tab's token in the harness's own process instead (see the server's `token`).
///
/// A provider of one's own is a `[model_providers.<id>]` table of its configuration, given for
/// one run with `-c` (see `ProviderChoice::arguments`). Checked against 0.156.1: the program
/// answers `wire_api = "chat"` with "`wire_api = "chat"` is no longer supported", so it speaks
/// only OpenAI's Responses shape (`/v1/responses`) to a provider. Through the relay, from a
/// container with no network, it answered on Xiaomi MiMo (`mimo-v2.6-flash`), Kimi Code
/// (`kimi-for-coding`) and OpenRouter, each of which serves that path; MiMo refused the first
/// request for naming Codex's web search, a tool only OpenAI's own servers run, so it is turned
/// off for such a profile.
///
/// First start, checked against 0.156.1 on a terminal at `/work` with a provider: before its
/// prompt it asks "Trust this folder?", and the answer lands in the same `config.toml` as
/// `[projects."/work"] trust_level = "trusted"`. So the template's settings carry that answer;
/// with them the prompt comes up with no key pressed.
static CODEX: Harness = Harness {
    id: "codex",
    display_name: "Codex",
    accounts: &[AccountKind::Subscription, AccountKind::ApiKey, AccountKind::Provider],
    withdrawn: &[],
    install: &["npm install -g @openai/codex"],
    command: "codex",
    auto_run: &["--dangerously-bypass-approvals-and-sandbox"],
    environment: &[],
    identity: &[".codex/auth.json"],
    settings: Some(ConfigFile {
        path: ".codex/config.toml",
        contents: "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n",
    }),
    first_start: None,
    resume: Some(Resume::Subcommand("resume")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".codex/config.toml", shape: McpShape::Codex }),
    key_only: None,
};

/// Kimi Code CLI, Moonshot's coding agent. Install, start command, data folder and sign-in from
/// its documentation (`moonshotai.github.io/kimi-code`, whose `llms-full.txt` holds every page):
/// "npm install -g @moonshot-ai/kimi-code", `kimi` to start, everything under `~/.kimi-code/`
/// (`KIMI_CODE_HOME` moves it), `/login` inside the interface or `kimi login` for Kimi Code's
/// device-code sign-in, and "Managed provider credentials are stored as `credentials/<name>.json`".
/// The TypeScript CLI replaced an older Python one of the same name; this is the npm package.
///
/// Checked against 2.1.0 in the image, whose program is one bundle (`dist/main.mjs`). `kimi
/// --help` lists `--auto`, "Start in Never Ask mode: never interrupts you; everything runs and is
/// decided automatically", beside `-y, --yolo`, which still asks for "risky actions", so `--auto`
/// is the unattended argument; it refuses to be combined with `-p`, which is why a one-shot run
/// leaves it out.
///
/// The login: the bundle's `resolveKimiTokenStorageName` turns the default slot `oauth/kimi-code`
/// into `kimi-code`, and its `FileTokenStorage` writes `<name>.json` under `credentials/`, so a
/// sign-in lands in `.kimi-code/credentials/kimi-code.json`. That file is not the whole login:
/// `applyManagedKimiCodeConfig` writes the provider `managed:kimi-code`, which names that slot,
/// its models and the default model into `.kimi-code/config.toml`, and without that entry the
/// token is never looked for (a fresh home answers "LLM not set, send /login to login"). So the
/// config file is carried beside the token, and no template writes it. The default slot is the
/// mainland (`auth.kimi.com`) one, which is what `/login` picks unless the person chooses the
/// global region; a global sign-in goes into a slot named after a hash of its hosts
/// (`kimi-code-env-<hash>`), which this record does not carry, and QCode then finds no login
/// rather than half of one. The key sign-in the same dialog offers (a Kimi Platform key) is
/// written into `config.toml` alone and is not offered here: a Kimi Code key is used through the
/// Providers page instead, where it never enters the container.
///
/// A provider is handed over in the `KIMI_MODEL_*` variables, which the documentation's
/// "Define a model from environment variables" describes and the bundle reads at start: with
/// `KIMI_MODEL_NAME` set it makes a provider of `KIMI_MODEL_PROVIDER_TYPE` at
/// `KIMI_MODEL_BASE_URL` with `KIMI_MODEL_API_KEY`, in memory, and nothing is written. Pointed at
/// an ollama server that way it sent its request there and printed the server's own answer.
///
/// First start, checked against 2.1.0 on a terminal at `/work`: a fresh home asks "Trust this
/// folder?" with "Trust this folder" highlighted and "Don't trust" leaving. The answer is a file
/// of its own, `.kimi-code/workspace-trust/<key>` holding `{"root":"/work","trustedAt":…}`, where
/// the key is the bundle's `encodeWorkDirKey`: `wd_`, the folder's last name, and the first twelve
/// hexadecimal digits of the SHA-256 of its path. With that file in place and no key pressed the
/// prompt comes up at once.
///
/// Resuming, from `kimi --help` (`-S, --session [id]`) and checked against 2.1.0: `kimi --auto
/// --session <id>` with an id no session has answers `Session "<id>" not found`, and with the id
/// [`history`](super::history) lists it opens that session.
///
/// MCP servers, from the documentation's MCP page (`~/.kimi-code/mcp.json`, `mcpServers`, "Entries
/// with a `command` field are stdio servers") and checked against 2.1.0: with the entry the bridge
/// writes, `/mcp` in the interface lists `qcode connected stdio 2 tools`.
///
/// Instructions: the documentation's agents page names the workspace's `AGENTS.md` and the
/// home's `~/.agents/skills/` for skills shared between tools, which is where graphify's `agents`
/// installer puts its section and its skill. graphify's own `kimi` platform (0.9.66) was tried and
/// does not fit: there is no `graphify kimi install` for the workspace, and `graphify install
/// --platform kimi` writes the skill to `~/.kimi/skills`, the folder of the older Python CLI.
/// Kimi Code CLI does not read it there; instead the folder's mere existence makes it open on a
/// "Migrate from kimi-cli" dialog in place of its prompt. `kimi migrate --run` would move the skill
/// to `~/.kimi-code/skills` and silence the dialog, but the workspace section would still need the
/// `agents` installer, which leaves a second copy of the same skill in `~/.agents/skills`.
static KIMI_CODE: Harness = Harness {
    id: "kimi-code",
    display_name: "Kimi Code CLI",
    accounts: &[AccountKind::Subscription, AccountKind::Provider],
    withdrawn: &[],
    install: &["npm install -g @moonshot-ai/kimi-code"],
    command: "kimi",
    auto_run: &["--auto"],
    environment: &[],
    identity: &[".kimi-code/credentials/kimi-code.json", ".kimi-code/config.toml"],
    settings: None,
    first_start: Some(ConfigFile {
        path: ".kimi-code/workspace-trust/wd_work_0c9a453fad61",
        contents: "{\"root\":\"/work\",\"trustedAt\":0}\n",
    }),
    resume: Some(Resume::Option("--session")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".kimi-code/mcp.json", shape: McpShape::Kimi }),
    key_only: None,
};

/// Qwen Code, Alibaba's fork of Gemini CLI. Install and start command from its readme
/// ("npm install -g @qwen-code/qwen-code@latest", `qwen`), the argument from `qwen --help`
/// (`--approval-mode` with `yolo`, "Automatically approve all tools"), and the rest from its
/// authentication guide (`qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/`).
///
/// There is no sign-in to carry. The guide says "The Qwen OAuth free tier was discontinued on
/// 2026-04-15", and the 0.24.4 help lists `qwen auth` as "(removed)". What remains is a key typed
/// into `/auth`, which the harness stores in `~/.qwen/settings.json` beside everything else, the
/// very file the template writes; that is not a login QCode could carry without carrying the
/// settings, so a key goes through the Providers page instead, and the key never enters the
/// container. A provider is handed over in `OPENAI_BASE_URL`, `OPENAI_API_KEY` and `OPENAI_MODEL`,
/// which the guide lists: with them set, a fresh home starts on its prompt saying "API Key |
/// <model>" and asks for nothing.
///
/// Checked against 0.24.4 in the image. The folder trust of its parent is still there
/// (`packages/cli/src/config/config.ts`: "Approval mode overridden to "default" because the current
/// folder is not trusted"), but `isFolderTrustEnabled` reads `settings.security?.folderTrust?.enabled
/// ?? false`, so it is off unless turned on; the template writes it off all the same, so a later
/// default cannot quietly undo the argument. The template also turns off the usage statistics,
/// `privacy.usageStatisticsEnabled`, which the settings schema of the same build defaults to
/// `true` and which the bundle sends to Alibaba Cloud's `gb4w8c3ygj-default-sea.rum.aliyuncs.com`.
///
/// Resuming, from `qwen --help` (`-r, --resume`, "Resume a specific session by its ID") and checked
/// against 0.24.4: with an id no session has it answers "No saved session found with ID <id>", and
/// with the id [`history`](super::history) lists it goes on to ask the model.
///
/// MCP servers, as its parent keeps them (`mcpServers` in `settings.json`, `trust` to skip the
/// confirmation of each call) and checked against 0.24.4: `qwen mcp list` starts the bridge's
/// server and prints `✓ qcode: node … (stdio) - Connected`.
///
/// Instructions: `memory-constants.ts` in the bundle reads `QWEN.md` and `AGENTS.md`, and its skill
/// folders are `~/.qwen/skills` and `~/.agents/skills`. graphify has no platform of Qwen Code's
/// own; its `agents` installer (0.9.66) writes its section into `AGENTS.md` and its skill into
/// `~/.agents/skills/graphify/`, both of which Qwen Code reads.
static QWEN_CODE: Harness = Harness {
    id: "qwen-code",
    display_name: "Qwen Code",
    accounts: &[AccountKind::Provider],
    withdrawn: &[],
    install: &["npm install -g @qwen-code/qwen-code"],
    command: "qwen",
    auto_run: &["--approval-mode=yolo"],
    environment: &[],
    identity: &[],
    settings: Some(ConfigFile {
        path: ".qwen/settings.json",
        contents: "{\n  \"security\": {\n    \"folderTrust\": {\n      \"enabled\": false\n    }\n  },\n  \"privacy\": {\n    \"usageStatisticsEnabled\": false\n  }\n}\n",
    }),
    first_start: None,
    resume: Some(Resume::Option("--resume")),
    surface: Surface::Terminal,
    mcp: Some(McpSettings { path: ".qwen/settings.json", shape: McpShape::Gemini }),
    key_only: None,
};

/// Antigravity IDE, the one harness here that opens a window instead of drawing in a terminal.
///
/// Everything below was measured in a throwaway container on a Wayland desktop, and the paragraphs
/// that follow say what each field was read from and what was seen.
///
/// The archive: the address, the version and the length are from the maker's own download page
/// (`antigravity.google/download`), whose Linux x64 link for the IDE is the one below; the server
/// answers it with `content-length: 240837095` and `last-modified` of 2026-09-13. The digest was
/// taken of that download. The address carries the version, so a new version is a new record and
/// a new image, the way a command-line harness is updated by installing it again.
///
/// The program: the archive holds one directory, `Antigravity IDE/`, and `antigravity-ide` inside
/// it is the real program. Its `bin/antigravity-ide` launcher is a shell script, and a container
/// whose first process is that shell swallowed the stop signal: `stop` waited its ten seconds
/// and killed. Started directly, with `--init` above it, the window closed on the signal in
/// under two and a half seconds with an exit code of 0.
///
/// The one flag: `--ozone-platform=wayland` was enough for a native Wayland window (the
/// compositor listed the client with `xwayland: false`). `--enable-features=UseOzonePlatform` was
/// not needed, the application draws its own title bar, and `--ignore-gpu-blocklist` must never
/// be added: with it the window came up empty and the graphics process restarted four times.
/// `--no-sandbox` is not here either, and is not to be added: the application's own sandbox comes
/// up inside the container on both engines (see `crate::desktop` for what docker needs for that).
///
/// The packages: the application is Electron 39 with Chromium 142 inside, so it wants GTK 3, NSS,
/// ALSA, GBM, libsecret, the X and Wayland client libraries and a font; Mesa, so that the
/// graphics process can fall back to drawing in software, which is what it did on the virtual
/// card it was measured on; `dbus` and `procps`, which it looks for on startup; and `curl`, which
/// the base image does not carry and the install step downloads with. Recommended packages stay
/// out, as everywhere in these images.
///
/// The login: the application offers nothing but "Continue with Google" on its first screen, and
/// that sign-in is not built yet — it opens a browser inside the container, which has none, and
/// waits on a port of the container's own network. So the account type says the login is the
/// person's to make inside the window, and the tab says plainly that the window is waiting for
/// one. Closing that gap means carrying the browser call out to this machine and the port it
/// answers on back in, and that is a slice of its own.
///
/// The settings: the `recommended` template turns the maker's telemetry and the application's own
/// updater off, and nothing else. It is written into the image's home directory like every
/// template's file, which means the workspace's home volume gets it when the volume is first filled
/// and never again, so an edit the person makes afterwards stays. The store trust question is
/// deliberately left alone: refusing it on someone's behalf is not QCode's to do.
///
/// The servers: the application reads user-level MCP servers from `~/.gemini/config/mcp_config.json`,
/// which is where its own code joins that path, and its schema takes no field it does not name.
/// This was measured in a throwaway container: a server written there was started, it was handed
/// the variable the entry's `env` named, and the application opened the newer era of the protocol
/// and asked for the tools. So the window's agent reaches the other tabs of the workspace and can
/// give them work; nothing can be given back to it, because it draws no prompt to type into.
static ANTIGRAVITY_IDE: Harness = Harness {
    id: "antigravity-ide",
    display_name: "Antigravity IDE",
    accounts: &[AccountKind::InApp],
    withdrawn: &[],
    install: &[],
    command: "/opt/antigravity-ide/antigravity-ide",
    auto_run: &[],
    environment: &[],
    identity: &[],
    settings: Some(ConfigFile {
        path: ".config/Antigravity IDE/User/settings.json",
        contents: "{\n  \"telemetry.telemetryLevel\": \"off\",\n  \"update.mode\": \"none\"\n}\n",
    }),
    first_start: None,
    resume: None,
    surface: Surface::Desktop(&ANTIGRAVITY),
    mcp: Some(McpSettings { path: ".gemini/config/mcp_config.json", shape: McpShape::Antigravity }),
    key_only: None,
};

/// The window Antigravity IDE opens, as the trial measured it.
static ANTIGRAVITY: Desktop = Desktop {
    version: "2.5.5",
    archive: "https://edgedl.me.gvt1.com/edgedl/release2/j0qc3/antigravity/stable/\
              2.5.5-4923483625488384/linux-x64/Antigravity%20IDE.tar.gz",
    bytes: 240_837_095,
    sha256: "0c5233b297d2b3aebb61af49f8944012c2953d361a5ebb16978490636917f831",
    install_dir: "/opt/antigravity-ide",
    program: "antigravity-ide",
    flags: &["--ozone-platform=wayland"],
    packages: &[
        "curl",
        "dbus",
        "fonts-dejavu-core",
        "libasound2t64",
        "libegl1",
        "libgbm1",
        "libgl1-mesa-dri",
        "libgtk-3-0t64",
        "libnss3",
        "libsecret-1-0",
        "libxkbfile1",
        "libxss1",
        "libxtst6",
        "mesa-vulkan-drivers",
        "procps",
        // Without it the application's own "open this in a browser" call returns success and does
        // nothing at all, which is how signing in stayed silent; measured 2026-09-20.
        "xdg-utils",
    ],
    // Measured 2026-09-20 on top of qcode/base: base 517 MB, the packages 351 MB, the
    // application 769 MB, which is 2295 MiB in all.
    image_mib: 2_295,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_is_described_completely() {
        for harness in HarnessKind::ALL {
            let record = harness.record();
            assert!(!record.id.is_empty() && !record.display_name.is_empty(), "{harness:?}");
            assert!(!record.accounts.is_empty(), "{harness:?} must support an account type");
            assert!(!record.command.is_empty(), "{harness:?} must say how it is started");
        }
    }

    #[test]
    fn every_command_line_harness_says_how_it_is_installed_run_and_resumed() {
        // The four fields below are what a terminal tab needs and a window has no use for: a
        // window is installed from an archive named in its own record, asks the person in its own
        // interface instead of taking an unattended flag, keeps a login QCode never carries, and
        // has no conversation id to be handed back.
        for harness in HarnessKind::TERMINAL {
            let record = harness.record();
            assert!(!record.install.is_empty(), "{harness:?} must say how it is installed");
            assert!(!record.auto_run.is_empty(), "{harness:?} must say how unattended mode is turned on");
            // A harness with nothing to sign in to has no login to carry; one with a sign-in must
            // say where it lands.
            let signs_in = record.accounts.iter().chain(record.withdrawn).any(|account| account.needs_login());
            assert_eq!(!record.identity.is_empty(), signs_in, "{harness:?} must say where its identity lives");
            assert!(record.resume.is_some(), "{harness:?} must say how a conversation is opened again");
            assert_eq!(harness.desktop(), None, "{harness:?} draws in the terminal");
        }
        assert_eq!(HarnessKind::TERMINAL.len() + 1, HarnessKind::ALL.len(), "every harness is one or the other");
    }

    #[test]
    fn identity_paths_stay_inside_the_home_directory() {
        for harness in HarnessKind::TERMINAL {
            for path in harness.record().identity {
                assert!(!path.starts_with('/') && !path.starts_with('~'), "{harness:?}: {path}");
                assert!(!path.split('/').any(|part| part == ".." || part.is_empty()), "{harness:?}: {path}");
            }
        }
    }

    #[test]
    fn configuration_a_template_writes_also_stays_inside_the_home_directory() {
        for harness in HarnessKind::ALL {
            let record = harness.record();
            for file in record.settings.into_iter().chain(record.first_start) {
                assert!(!file.path.starts_with('/') && !file.path.starts_with('~'), "{harness:?}");
                assert!(!file.contents.is_empty(), "{harness:?}");
            }
        }
    }

    #[test]
    fn every_harness_reads_its_servers_from_its_home_and_never_from_its_login() {
        for harness in HarnessKind::ALL {
            let record = harness.record();
            let path = record.mcp.expect("every harness starts servers of its own").path;
            assert!(!path.starts_with('/') && !path.starts_with('~'), "{harness:?}: {path}");
            assert!(!record.identity.contains(&path), "{harness:?}: registering a server would touch the login");
        }
        assert_eq!(HarnessKind::ClaudeCode.record().mcp.expect("it has one").path, ".claude.json");
        assert_eq!(HarnessKind::Codex.record().mcp.expect("it has one").shape, McpShape::Codex);
        let window = HarnessKind::AntigravityIde.record().mcp.expect("a window reads servers too");
        assert_eq!(window.path, ".gemini/config/mcp_config.json");
        assert_eq!(window.shape, McpShape::Antigravity);
    }

    #[test]
    fn where_a_template_writes_settings_the_servers_go_into_the_same_file() {
        // Otherwise the harness would read two files, and the one the template wrote could hide
        // the servers.
        for harness in [HarnessKind::OpenCode, HarnessKind::GeminiCli, HarnessKind::Codex, HarnessKind::QwenCode] {
            let record = harness.record();
            assert_eq!(record.settings.map(|file| file.path), record.mcp.map(|mcp| mcp.path), "{harness:?}");
        }
    }

    #[test]
    fn identity_is_never_the_file_a_template_writes() {
        // A profile's settings are generated; overwriting them with a copied identity would
        // silently undo the template.
        for harness in HarnessKind::ALL {
            let record = harness.record();
            for file in record.settings.into_iter().chain(record.first_start) {
                assert!(!record.identity.contains(&file.path), "{harness:?}: {}", file.path);
            }
        }
    }

    #[test]
    fn identifiers_are_unique_and_read_back_as_the_same_harness() {
        let mut seen = Vec::new();
        for harness in HarnessKind::ALL {
            let id = harness.record().id;
            assert!(!seen.contains(&id), "{id} twice");
            seen.push(id);
            assert_eq!(HarnessKind::parse(id), Some(harness));
        }
        assert_eq!(
            seen,
            ["claude-code", "opencode", "gemini-cli", "codex", "kimi-code", "qwen-code", "antigravity-ide"]
        );
        assert_eq!(HarnessKind::parse("Claude-Code"), None, "identifiers are written one way only");
        assert_eq!(HarnessKind::parse("cursor"), None);
        assert_eq!(HarnessKind::parse("antigravity-ide"), Some(HarnessKind::AntigravityIde));
    }

    #[test]
    fn account_types_read_back_as_the_same_type() {
        for account in AccountKind::ALL {
            assert_eq!(AccountKind::parse(account.id()), Some(account));
        }
        assert_eq!(AccountKind::parse("none"), None);
        assert_eq!(AccountKind::parse("free"), Some(AccountKind::Free));
        assert!(HarnessKind::ClaudeCode.supports(AccountKind::Subscription));
        assert!(HarnessKind::ClaudeCode.supports(AccountKind::ApiKey));
    }

    #[test]
    fn the_harnesses_proven_through_the_relay_are_offered_a_provider_of_ones_own_and_nothing_else_is() {
        for harness in [
            HarnessKind::ClaudeCode,
            HarnessKind::OpenCode,
            HarnessKind::Codex,
            HarnessKind::KimiCode,
            HarnessKind::QwenCode,
        ] {
            assert!(harness.supports(AccountKind::Provider), "{harness:?}");
        }
        for harness in [HarnessKind::GeminiCli, HarnessKind::AntigravityIde] {
            assert!(!harness.supports(AccountKind::Provider), "{harness:?}");
        }
        assert!(!AccountKind::Provider.needs_login(), "the key already lives in providers.toml");
    }

    #[test]
    fn only_opencode_is_offered_for_free_and_offers_it_first() {
        assert_eq!(HarnessKind::OpenCode.record().accounts.first(), Some(&AccountKind::Free));
        for harness in [
            HarnessKind::ClaudeCode,
            HarnessKind::GeminiCli,
            HarnessKind::Codex,
            HarnessKind::KimiCode,
            HarnessKind::QwenCode,
        ] {
            assert!(!harness.supports(AccountKind::Free), "{harness:?}");
        }
        assert!(!AccountKind::Free.needs_login());
        assert!(AccountKind::Subscription.needs_login() && AccountKind::ApiKey.needs_login());
        // QCode has no login of its own to make for a window the person signs in to themselves.
        assert!(!AccountKind::InApp.needs_login());
        assert_eq!(HarnessKind::AntigravityIde.record().accounts, [AccountKind::InApp]);
        for harness in HarnessKind::TERMINAL {
            assert!(!harness.supports(AccountKind::InApp), "{harness:?}");
        }
    }

    #[test]
    fn a_new_conversation_is_the_program_in_unattended_mode() {
        assert_eq!(HarnessKind::ClaudeCode.command_line(None), ["claude", "--dangerously-skip-permissions"]);
        assert_eq!(HarnessKind::OpenCode.command_line(None), ["opencode", "--auto"]);
        assert_eq!(HarnessKind::GeminiCli.command_line(None), ["gemini", "--approval-mode=yolo"]);
        assert_eq!(HarnessKind::Codex.command_line(None), ["codex", "--dangerously-bypass-approvals-and-sandbox"]);
        assert_eq!(HarnessKind::KimiCode.command_line(None), ["kimi", "--auto"]);
        assert_eq!(HarnessKind::QwenCode.command_line(None), ["qwen", "--approval-mode=yolo"]);
    }

    #[test]
    fn an_earlier_conversation_is_opened_the_way_each_harness_takes_it() {
        let id = "2afe99eb-008a-4542-b160-1aa5b29bb95f";
        assert_eq!(
            HarnessKind::ClaudeCode.command_line(Some(id)),
            ["claude", "--dangerously-skip-permissions", "--resume", id]
        );
        assert_eq!(
            HarnessKind::OpenCode.command_line(Some("ses_f4acc7e75ffeEArYIV9UJooqnn")),
            ["opencode", "--auto", "--session", "ses_f4acc7e75ffeEArYIV9UJooqnn"]
        );
        assert_eq!(HarnessKind::GeminiCli.command_line(Some(id)), ["gemini", "--approval-mode=yolo", "--resume", id]);
        // The subcommand comes first, so the unattended-mode argument is the subcommand's own.
        assert_eq!(
            HarnessKind::Codex.command_line(Some(id)),
            ["codex", "resume", "--dangerously-bypass-approvals-and-sandbox", id]
        );
        let kimi = "session_e3f864de-1a92-4498-a9f7-5fd551a34703";
        assert_eq!(HarnessKind::KimiCode.command_line(Some(kimi)), ["kimi", "--auto", "--session", kimi]);
        assert_eq!(HarnessKind::QwenCode.command_line(Some(id)), ["qwen", "--approval-mode=yolo", "--resume", id]);
    }

    #[test]
    fn an_id_that_could_be_taken_for_an_option_opens_a_new_conversation_instead() {
        for harness in HarnessKind::ALL {
            for id in ["--help", "-c", "", "two words", "a;b"] {
                assert_eq!(harness.command_line(Some(id)), harness.command_line(None), "{harness:?}: {id:?}");
            }
        }
    }

    #[test]
    fn verified_claude_code_facts() {
        let record = HarnessKind::ClaudeCode.record();
        assert_eq!(record.command, "claude");
        assert_eq!(record.auto_run, ["--dangerously-skip-permissions"]);
        assert_eq!(record.identity, [".claude/.credentials.json"]);
        assert!(record.install.iter().any(|step| step.contains("@anthropic-ai/claude-code")));
    }

    #[test]
    fn verified_opencode_facts() {
        let record = HarnessKind::OpenCode.record();
        assert_eq!(record.command, "opencode");
        assert_eq!(record.auto_run, ["--auto"]);
        assert_eq!(record.identity, [".local/share/opencode/auth.json"]);
        assert!(record.install.iter().any(|step| step.contains("opencode-ai")));
    }

    #[test]
    fn gemini_cli_offers_only_an_api_key_to_new_profiles() {
        // Google stopped personal "Login with Google" for Gemini CLI on 2026-06-18.
        assert_eq!(HarnessKind::GeminiCli.record().accounts, [AccountKind::ApiKey]);
        assert!(HarnessKind::GeminiCli.supports(AccountKind::Subscription), "existing profiles keep loading");
        assert!(HarnessKind::GeminiCli.withdrawn(AccountKind::Subscription));
        for harness in [HarnessKind::ClaudeCode, HarnessKind::OpenCode, HarnessKind::Codex] {
            assert!(!harness.withdrawn(AccountKind::Subscription), "{harness:?}");
        }
    }

    #[test]
    fn gemini_cli_is_kept_to_a_key_by_its_system_settings_and_no_other_harness_is() {
        let file = HarnessKind::GeminiCli.record().key_only.expect("Gemini CLI can be kept to a key");
        assert_eq!(file.path, "/etc/gemini-cli/settings.json");
        let settings: serde_json::Value = serde_json::from_str(file.contents).expect("the file is JSON");
        assert_eq!(settings, serde_json::json!({ "security": { "auth": { "enforcedType": "gemini-api-key" } } }));
        for harness in HarnessKind::ALL.into_iter().filter(|harness| *harness != HarnessKind::GeminiCli) {
            assert_eq!(harness.record().key_only, None, "{harness:?}");
        }
    }

    #[test]
    fn verified_gemini_cli_facts() {
        let record = HarnessKind::GeminiCli.record();
        assert_eq!(record.command, "gemini");
        assert_eq!(record.auto_run, ["--approval-mode=yolo"]);
        assert_eq!(record.identity, [".gemini/gemini-credentials.json", ".gemini/google_accounts.json"]);
        // Without this variable the credentials may go to an OS keyring the copy cannot reach.
        assert_eq!(record.environment, [("GEMINI_FORCE_FILE_STORAGE", "true")]);
        assert!(record.install.iter().any(|step| step.contains("@google/gemini-cli")));
        // Without this file the harness puts the approval mode back to "default" in a folder
        // nobody has trusted, which every fresh container's workspace directory is.
        let settings = record.settings.expect("the folder trust has to be turned off");
        assert_eq!(settings.path, ".gemini/settings.json");
        assert!(settings.contents.contains("\"folderTrust\""), "{}", settings.contents);
        assert!(settings.contents.contains("\"enabled\": false"), "{}", settings.contents);
    }

    #[test]
    fn verified_antigravity_ide_facts() {
        let record = HarnessKind::AntigravityIde.record();
        let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
        assert_eq!(record.command, desktop.command());
        assert_eq!(desktop.command(), "/opt/antigravity-ide/antigravity-ide");
        // The address is the maker's own, carries the version this record describes, and is the
        // Linux x64 archive of the IDE rather than of the other product on the same page.
        assert!(desktop.archive.starts_with("https://"), "{}", desktop.archive);
        assert!(desktop.archive.contains(desktop.version), "{}", desktop.archive);
        assert!(desktop.archive.contains("/linux-x64/"), "{}", desktop.archive);
        assert!(!desktop.archive.contains(' ') && !desktop.archive.contains('\n'), "{}", desktop.archive);
        assert_eq!(desktop.bytes, 240_837_095);
        assert_eq!(desktop.sha256.len(), 64);
        assert!(desktop.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // The one flag the window needs, and the two that must never be added: one blanked the
        // window, the other would give up the application's own sandbox.
        assert_eq!(desktop.flags, ["--ozone-platform=wayland"]);
        assert!(!desktop.flags.contains(&"--no-sandbox"), "the sandbox comes up on both engines");
        assert!(!desktop.flags.contains(&"--ignore-gpu-blocklist"), "it left the window empty");
        // Nothing is installed from a registry, and nothing of the archive is carried here.
        assert!(record.install.is_empty() && record.auto_run.is_empty() && record.resume.is_none());
        assert!(record.identity.is_empty(), "the login lives in the workspace's home volume");
        let settings = record.settings.expect("the template turns telemetry and the updater off");
        assert!(settings.contents.contains("\"telemetry.telemetryLevel\": \"off\""), "{}", settings.contents);
        assert!(settings.contents.contains("\"update.mode\": \"none\""), "{}", settings.contents);
        // Refusing the workspace trust question on someone's behalf is not QCode's to do.
        assert!(!settings.contents.contains("workspace.trust"), "{}", settings.contents);
        assert!(desktop.image_mib > 1_000, "the person is told how large it is: {}", desktop.image_mib);
    }

    #[test]
    fn a_window_is_opened_on_the_workspace_with_the_flags_it_always_takes() {
        let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
        assert_eq!(
            desktop.command_line("/work"),
            ["/opt/antigravity-ide/antigravity-ide", "--ozone-platform=wayland", "/work"]
        );
    }

    #[test]
    fn the_packages_a_window_needs_are_named_once_and_installable() {
        let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
        let mut sorted = desktop.packages.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, desktop.packages, "the list is sorted and says each package once");
        for package in desktop.packages {
            // A word apt takes as a package name, never an option and never a shell word.
            assert!(package.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()), "{package}");
            assert!(package.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c)));
        }
        // The archive is downloaded by the install step, and the base image carries no client.
        assert!(desktop.packages.contains(&"curl"), "nothing would fetch the archive");
    }

    #[test]
    fn verified_codex_facts() {
        let record = HarnessKind::Codex.record();
        assert_eq!(record.command, "codex");
        assert_eq!(record.auto_run, ["--dangerously-bypass-approvals-and-sandbox"]);
        assert_eq!(record.identity, [".codex/auth.json"]);
        assert!(record.install.iter().any(|step| step.contains("@openai/codex")));
        // The folder's trust is answered where Codex keeps the answer, so a tab opens on its prompt.
        let settings = record.settings.expect("the template writes its settings");
        assert!(settings.contents.contains("[projects.\"/work\"]\ntrust_level = \"trusted\""), "{}", settings.contents);
    }

    #[test]
    fn verified_kimi_code_facts() {
        let record = HarnessKind::KimiCode.record();
        assert_eq!(record.command, "kimi");
        // `--yolo` still asks before what it counts as risky; only `--auto` never asks.
        assert_eq!(record.auto_run, ["--auto"]);
        assert!(record.install.iter().any(|step| step.contains("@moonshot-ai/kimi-code")));
        // The token alone is never looked for: the provider entry that names its slot is in the
        // configuration, so both go, the token first because it is the proof of a sign-in.
        assert_eq!(record.identity, [".kimi-code/credentials/kimi-code.json", ".kimi-code/config.toml"]);
        assert_eq!(record.settings, None, "the configuration is part of the login and no template writes it");
        assert_eq!(record.accounts, [AccountKind::Subscription, AccountKind::Provider]);
        let mcp = record.mcp.expect("it reads servers");
        assert_eq!((mcp.path, mcp.shape), (".kimi-code/mcp.json", McpShape::Kimi));
    }

    #[test]
    fn kimi_code_opens_on_its_prompt_in_the_workspace_it_trusts() {
        let trust = HarnessKind::KimiCode.record().first_start.expect("the trust question is answered");
        // The file's name is Kimi Code's own key for the folder: `wd_`, the folder's last name, and
        // the first twelve hexadecimal digits of the SHA-256 of `/work`. The live test has the
        // installed harness answer the question and compares the name it wrote.
        assert_eq!(crate::base::paths::CODE_DIR, "/work");
        assert_eq!(trust.path, ".kimi-code/workspace-trust/wd_work_0c9a453fad61");
        let answer: serde_json::Value = serde_json::from_str(trust.contents).expect("the answer is JSON");
        assert_eq!(answer["root"], crate::base::paths::CODE_DIR);
    }

    #[test]
    fn verified_qwen_code_facts() {
        let record = HarnessKind::QwenCode.record();
        assert_eq!(record.command, "qwen");
        assert_eq!(record.auto_run, ["--approval-mode=yolo"]);
        assert!(record.install.iter().any(|step| step.contains("@qwen-code/qwen-code")));
        // Its own sign-in was discontinued and a key it keeps lives in its settings, so a provider
        // of one's own is the one way in, and there is no login to carry.
        assert_eq!(record.accounts, [AccountKind::Provider]);
        assert!(record.identity.is_empty());
        let settings: serde_json::Value =
            serde_json::from_str(record.settings.expect("the template writes settings").contents).expect("JSON");
        assert_eq!(settings["security"]["folderTrust"]["enabled"], false);
        assert_eq!(settings["privacy"]["usageStatisticsEnabled"], false);
        assert_eq!(record.mcp.expect("it reads servers").shape, McpShape::Gemini);
    }
}
