# Changelog

Every release of quvyta-code, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0, a minor release may
change files qcode writes, and the notes say so when it does.

## 0.1.6 - 2026-09-19

### Added

- Project backups. Every open project's `Project/` folder is backed up into `Backup/Project.git`, next to the project, every 15 minutes (5 minutes, 1 hour or off in **Settings**), when it is closed and when qcode quits. git runs in a short-lived container of the base image, so the machine needs no git, and a person's own repository in `Project/` is never touched.
- **Backups** in the side panel lists every backup and brings the project back to one after asking; **Earlier versions** in a file's menu does the same for one file. How things were just before is backed up first, and nothing is deleted.
- **Don't back up** in the file tree leaves a folder or a file out; the list is kept in `project.qcode`.
- **Back up Assets too** in the side panel backs `Assets/` up apart, into `Backup/Assets.git`.
- Each profile's conversations in a project are backed up into `Backup/Conversations/<profile>.git`, only the harness's conversation files and never its login, and can be brought back while the profile's container is stopped.
- The side panel shows when the last backup was made and how much `Backup/` holds.

## 0.1.5 - 2026-09-19

### Changed

- A new Gemini CLI profile signs in with an API key only. Google closed "Login with Google" in
  Gemini CLI to personal accounts (Code Assist for individuals, Google AI Pro and Ultra) on
  18 June 2026. A profile made earlier with a Google sign-in still loads and says why it may no
  longer work; the sign-in keeps working for Gemini Code Assist Standard and Enterprise.

### Added

- `qcode --version` (and `-V`) prints the version.
- README: a half-minute moving picture drawn from the demo workspace, a one-line summary, a comparison with similar tools, what the container protects and
  what it does not, and a statement that qcode sends no telemetry.
- This changelog, a contributing guide and issue templates.

## 0.1.4 - 2026-09-19

### Added

- A file manager in the project panel's file tree: new file and folder, rename (with the name
  before its extension selected), cut and paste, delete and refresh from the context menu;
  several entries at once with `ctrl`+click and `shift`+click; dragging entries onto a folder.
- The file tree follows the disk: the folders on screen are watched, and a change rereads only
  the folder it happened in. Nothing is read on a timer.
- Built-in apps: a file chosen in the tree opens in a tab of its own. Text opens in `nano` or
  `vim` (chosen in Settings), Markdown is read in qcode itself, and pictures are drawn with
  `chafa`. The base image also carries `zip`, `unzip`, `xz`, `bzip2` and `7z`.
- Settings, "When QCode closes": the containers QCode started are stopped once the last QCode
  closes (the default), or keep running.
- An optional background service (a systemd user path unit on Linux, a launchd job on macOS),
  installed and removed from Settings, that stops QCode's containers after a crash too. Not
  available on Windows.

### Changed

- `ctrl+alt+space` works both ways: from a terminal it leaves for the tab strip, and from
  anywhere else on the project screen it goes back into the open tab's terminal.
- The tab strip draws its own `+` right after the last tab.
- Built on quvyta-framework 0.1.7.

## 0.1.3 - 2026-09-18

### Changed

- Settings move to the Quvyta family's file, `~/.config/quvyta/code.conf` on Linux. Settings
  from `~/.config/quvyta/code/settings.toml` are brought over once, at start.
- A new workspace defaults to `Quvyta/Code` in your Documents folder. A workspace chosen before
  stays where it is.
- The README pictures are drawn from a demo workspace with one command.

## 0.1.2 - 2026-09-18

### Added

- Tabs like a browser: the rail on the left holds the open projects, each with its own tabs, and
  `+` opens another project.
- "Continue" on the home screen goes back to the open project screen, or, after a restart,
  brings back the same projects, tabs and order. Tabs come back lazily, when they are shown.
- A new tab page (`ctrl+t`) lists a shell and, for each profile, "New chat" and its latest
  conversations in the project; choosing one resumes it. Claude Code, opencode, Gemini CLI and
  Codex CLI all resume by id.
- `ctrl+alt+space` leaves a harness or shell tab for the tab strip.

### Changed

- The mouse reaches a harness that uses it, and `f1` and `alt+b` work inside harness tabs.

## 0.1.1 - 2026-09-18

### Added

- Every profile is offered in a new tab, not only the ones the project already carries; opening
  one adds it to the project. With no profile at all, a "New profile" row leads to the profile
  screen.
- opencode can be used with its free models, without an account.

### Changed

- Screens are centred, choices use square marks, and one hint shows the keys of every screen.

## 0.1.0 - 2026-09-18

The first beta release.

### Added

- Profiles: a harness (Claude Code, opencode, Gemini CLI or Codex CLI), a template, an account
  type and what its containers may reach. Building a profile builds its image; the build can be
  stopped and a half-made image is removed.
- Signing in once through the harness's own sign-in flow inside a container, with a per-project
  copy of the login.
- Projects made empty, from a copy of a folder, or from a git address cloned inside a container.
- The project screen: shell and harness tabs, a side panel with the file tree, the project's
  details and its containers.
- Podman and Docker, found and checked at start, with the command that installs or starts the
  engine when it is missing.
- English and Turkish.
