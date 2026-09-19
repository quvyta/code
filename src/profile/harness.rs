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
//! four installed clean with the plain command, and the two that need a script (Claude Code and
//! opencode copy a native binary over a stub in `postinstall`) answered afterwards, so no
//! `--allow-scripts` is written here; the live test is what would notice if npm stopped running
//! them.

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
}

/// A configuration file a template writes into the image, by its path under the home directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFile {
    /// Where the file goes, relative to the harness's home directory in the container.
    pub path: &'static str,
    /// What the file holds, written as the harness expects to read it.
    pub contents: &'static str,
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
    /// The shell commands that install the harness into the image.
    pub install: &'static [&'static str],
    /// The program that starts the harness in the container.
    pub command: &'static str,
    /// The arguments that let the harness work without asking for permission. The container is
    /// the isolation, so they are always passed.
    pub auto_run: &'static [&'static str],
    /// Environment variables the container must set for the harness to behave as described here.
    pub environment: &'static [(&'static str, &'static str)],
    /// The files the login lives in, relative to the home directory. Each one is a file, not a
    /// directory, so a copy never drags settings along with the login.
    pub identity: &'static [&'static str],
    /// The configuration the `recommended` template writes, when the harness reads one.
    pub settings: Option<ConfigFile>,
    /// How the harness is told to open a conversation it had before, by that conversation's id.
    pub resume: Resume,
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
    /// Every harness, in the order the profile wizard offers them.
    pub const ALL: [Self; 4] = [Self::ClaudeCode, Self::OpenCode, Self::GeminiCli, Self::Codex];

    /// What QCode knows about this harness.
    #[must_use]
    pub fn record(self) -> &'static Harness {
        match self {
            Self::ClaudeCode => &CLAUDE_CODE,
            Self::OpenCode => &OPENCODE,
            Self::GeminiCli => &GEMINI_CLI,
            Self::Codex => &CODEX,
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
    /// started as if none had been asked for rather than with an argument nobody chose.
    #[must_use]
    pub fn command_line(self, conversation: Option<&str>) -> Vec<String> {
        let record = self.record();
        let auto_run = record.auto_run.iter().map(|arg| (*arg).to_owned());
        let mut line = vec![record.command.to_owned()];
        match conversation.filter(|id| super::history::is_safe_id(id)) {
            None => line.extend(auto_run),
            Some(id) => match record.resume {
                Resume::Option(option) => {
                    line.extend(auto_run);
                    line.extend([option.to_owned(), id.to_owned()]);
                }
                Resume::Subcommand(command) => {
                    line.push(command.to_owned());
                    line.extend(auto_run);
                    line.push(id.to_owned());
                }
            },
        }
        line
    }
}

impl AccountKind {
    /// Every account type. The wizard offers each harness's own list, in that harness's order.
    pub const ALL: [Self; 3] = [Self::Free, Self::Subscription, Self::ApiKey];

    /// How the account type is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Subscription => "subscription",
            Self::ApiKey => "api-key",
        }
    }

    /// The account type written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|account| account.id() == id)
    }

    /// Whether a profile with this account has a login at all. One that has none is ready as
    /// soon as its image is, and nothing about it waits for a credentials volume.
    #[must_use]
    pub fn needs_login(self) -> bool {
        self != Self::Free
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
static CLAUDE_CODE: Harness = Harness {
    id: "claude-code",
    display_name: "Claude Code",
    accounts: &[AccountKind::Subscription, AccountKind::ApiKey],
    withdrawn: &[],
    install: &["npm install -g @anthropic-ai/claude-code"],
    command: "claude",
    auto_run: &["--dangerously-skip-permissions"],
    environment: &[],
    identity: &[".claude/.credentials.json"],
    settings: Some(ConfigFile {
        path: ".claude/settings.json",
        contents: "{\n  \"permissions\": {\n    \"defaultMode\": \"bypassPermissions\"\n  }\n}\n",
    }),
    resume: Resume::Option("--resume"),
};

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
static OPENCODE: Harness = Harness {
    id: "opencode",
    display_name: "opencode",
    accounts: &[AccountKind::Free, AccountKind::Subscription, AccountKind::ApiKey],
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
    resume: Resume::Option("--session"),
};

/// Gemini CLI. Install and start command from the project's readme, the argument from
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
/// reading `settings.security?.folderTrust?.enabled ?? true`. The project directory of a fresh container is not trusted, so without the file
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
/// `gemini --approval-mode=yolo --resume <id> --prompt hi` with an id the project's
/// conversations do not have answers `Error resuming session: Invalid session identifier`, and
/// with the id [`history`](super::history) lists it goes on to ask for an auth method.
///
/// Google stopped serving Gemini CLI to personal Google accounts (Code Assist for individuals and
/// the paid Pro and Ultra plans) and removed "Login with Google" for them on 2026-06-18
/// (`developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals`). Code
/// Assist Standard and Enterprise still sign in that way, so a profile made with a sign-in keeps
/// loading; a new one is offered an API key.
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
    resume: Resume::Option("--resume"),
};

/// Codex CLI. Install and start command from the project's readme, the argument from the source
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
static CODEX: Harness = Harness {
    id: "codex",
    display_name: "Codex",
    accounts: &[AccountKind::Subscription, AccountKind::ApiKey],
    withdrawn: &[],
    install: &["npm install -g @openai/codex"],
    command: "codex",
    auto_run: &["--dangerously-bypass-approvals-and-sandbox"],
    environment: &[],
    identity: &[".codex/auth.json"],
    settings: Some(ConfigFile {
        path: ".codex/config.toml",
        contents: "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
    }),
    resume: Resume::Subcommand("resume"),
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
            assert!(!record.install.is_empty(), "{harness:?} must say how it is installed");
            assert!(!record.command.is_empty(), "{harness:?} must say how it is started");
            assert!(!record.auto_run.is_empty(), "{harness:?} must say how unattended mode is turned on");
            assert!(!record.identity.is_empty(), "{harness:?} must say where its identity lives");
        }
    }

    #[test]
    fn identity_paths_stay_inside_the_home_directory() {
        for harness in HarnessKind::ALL {
            for path in harness.record().identity {
                assert!(!path.starts_with('/') && !path.starts_with('~'), "{harness:?}: {path}");
                assert!(!path.split('/').any(|part| part == ".." || part.is_empty()), "{harness:?}: {path}");
            }
        }
    }

    #[test]
    fn configuration_a_template_writes_also_stays_inside_the_home_directory() {
        for harness in HarnessKind::ALL {
            if let Some(file) = harness.record().settings {
                assert!(!file.path.starts_with('/') && !file.path.starts_with('~'), "{harness:?}");
                assert!(!file.contents.is_empty(), "{harness:?}");
            }
        }
    }

    #[test]
    fn identity_is_never_the_file_a_template_writes() {
        // A profile's settings are generated; overwriting them with a copied identity would
        // silently undo the template.
        for harness in HarnessKind::ALL {
            let record = harness.record();
            if let Some(file) = record.settings {
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
        assert_eq!(seen, ["claude-code", "opencode", "gemini-cli", "codex"]);
        assert_eq!(HarnessKind::parse("Claude-Code"), None, "identifiers are written one way only");
        assert_eq!(HarnessKind::parse("cursor"), None);
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
    fn only_opencode_is_offered_for_free_and_offers_it_first() {
        assert_eq!(HarnessKind::OpenCode.record().accounts.first(), Some(&AccountKind::Free));
        for harness in [HarnessKind::ClaudeCode, HarnessKind::GeminiCli, HarnessKind::Codex] {
            assert!(!harness.supports(AccountKind::Free), "{harness:?}");
        }
        assert!(!AccountKind::Free.needs_login());
        assert!(AccountKind::Subscription.needs_login() && AccountKind::ApiKey.needs_login());
    }

    #[test]
    fn a_new_conversation_is_the_program_in_unattended_mode() {
        assert_eq!(HarnessKind::ClaudeCode.command_line(None), ["claude", "--dangerously-skip-permissions"]);
        assert_eq!(HarnessKind::OpenCode.command_line(None), ["opencode", "--auto"]);
        assert_eq!(HarnessKind::GeminiCli.command_line(None), ["gemini", "--approval-mode=yolo"]);
        assert_eq!(HarnessKind::Codex.command_line(None), ["codex", "--dangerously-bypass-approvals-and-sandbox"]);
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
    fn verified_gemini_cli_facts() {
        let record = HarnessKind::GeminiCli.record();
        assert_eq!(record.command, "gemini");
        assert_eq!(record.auto_run, ["--approval-mode=yolo"]);
        assert_eq!(record.identity, [".gemini/gemini-credentials.json", ".gemini/google_accounts.json"]);
        // Without this variable the credentials may go to an OS keyring the copy cannot reach.
        assert_eq!(record.environment, [("GEMINI_FORCE_FILE_STORAGE", "true")]);
        assert!(record.install.iter().any(|step| step.contains("@google/gemini-cli")));
        // Without this file the harness puts the approval mode back to "default" in a folder
        // nobody has trusted, which every fresh container's project directory is.
        let settings = record.settings.expect("the folder trust has to be turned off");
        assert_eq!(settings.path, ".gemini/settings.json");
        assert!(settings.contents.contains("\"folderTrust\""), "{}", settings.contents);
        assert!(settings.contents.contains("\"enabled\": false"), "{}", settings.contents);
    }

    #[test]
    fn verified_codex_facts() {
        let record = HarnessKind::Codex.record();
        assert_eq!(record.command, "codex");
        assert_eq!(record.auto_run, ["--dangerously-bypass-approvals-and-sandbox"]);
        assert_eq!(record.identity, [".codex/auth.json"]);
        assert!(record.install.iter().any(|step| step.contains("@openai/codex")));
    }
}
