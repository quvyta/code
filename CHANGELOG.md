# Changelog

Every release of quvyta-code, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0, a minor release may
change files qcode writes, and the notes say so when it does.

## 0.1.11 - 2026-09-21

### Added

- The first-run wizard has a step for the container engine. It ticks the engine your machine already has, and when there is none it either installs one for you, in a terminal inside the wizard, or shows you the command to run yourself. Nothing is installed without you asking for it.

### Changed

- The first-run wizard opens in the middle of the screen and already speaks the language your machine is set to; the language list starts with that language and the rest follow in alphabetical order.
- The way back, the settings and the list of keys are three small buttons at the foot of the workspace rail. The Back row at the top and the Keys row at the bottom are gone, so a terminal has two more lines than before.
- Making a profile from inside a workspace hands you back to that workspace when you are done, instead of leaving you on the list of profiles.
- A project is called a **workspace** now, everywhere: on every screen, in all nine languages, in the files qcode writes and in the folders it keeps them in. `Projects/` is `Workspaces/`, a workspace's `project.qcode` is `workspace.qcode`, the folder your code lives in is `Work/` rather than `Project/`, and its backup history is `Backup/Code.git`. The folder that holds all of them, which used to be called the workspace, is the **QCode folder**.
- Nothing of an older installation is lost on the way. The first qcode that opens gives every old name the name it has today, one rename at a time, so an interruption leaves the old name whole rather than half a move; a name that is already taken is left alone and said on the settings screen. The settings file keeps the place you chose and the list of what you opened last, a profile file that still says `mounts.project` is read without a complaint, and a session written before this release still brings back your workspaces and their tabs.
- Inside a container your workspace is at `/work` and its material at `/assets`, each a folder of its own at the root. The folder is called `Work/` at home and `/work` in the container, so it is one word in both places and the word "project" is gone from there too. A backup container sees the workspace at `/work` and its backups at `/backup`.
- Conversations you had before this release are still in the list. Every harness writes the directory it worked in into what it records, and yours say `/work/Project`; each of the four history readers is told that name as well as today's, so nothing you talked about disappears. A disk whose workspaces still keep your files in `Code/` is carried over to `Work/` the first time qcode opens, one rename at a time, like the names before it.
- **Updating from an older qcode: nothing of yours is lost and nothing needs doing.** The first time this release opens it renames what is on your disk to the words above, one rename at a time: `Projects/` becomes `Workspaces/`, each workspace's `project.qcode` becomes `workspace.qcode`, and a workspace's `Code/` folder becomes `Work/`. A name that is already taken is left exactly as it is and said on the settings screen rather than written over. Your settings, your list of what was open, your files and your earlier conversations all come through.
- The page a window asks to have opened now goes to your browser without qcode leaving the screen at all. It used to step aside for a shell, which made the screen blink; the browser is started beside qcode instead, so the page you were reading stays on screen while a browser that has yet to start comes up.
- The question mark at the foot of the rail and the mark of a workspace come from the framework's own icon set, so they look the same in qcode as in every other Quvyta application. A workspace has a mark of its own now, different from the one a project has.

## 0.1.10 - 2026-09-20

### Added

- qcode speaks nine languages: English, Turkish, German, Spanish, French, Italian, Portuguese, Russian and Chinese. The language is asked for on the first screen and changed in **Settings**; the one you pick is the one qcode opens in next time. A gate keeps every language complete and keeps each one in its own words, so a file that quietly held English would not pass.
- Signing in from a desktop window works. The window's application asks its desktop to open a page, as every Linux program does; qcode takes that address through a folder the container shares with it and opens it in your own browser, where your accounts already are. The address is always shown on the tab as well, so a machine with no browser to be had can still be signed in from somewhere else. Only `http` and `https` addresses are opened; anything else is refused and said aloud.
- Closing qcode and opening it again finds everything where you left it: the projects that were open, their tabs in their order, and the tab you were on. A window tab comes back with the others and waits to be asked before it opens its window again, because opening qcode is not asking for that window.

### Changed

- The size promised for a desktop image is the size it really builds to. The figure was the download, not the image; it is now measured and shown as what the image costs on disk.
- Longer words keep their ends: drop-downs are as wide as what they hold, and the buttons of a narrow panel stack instead of being cut.
- The page a window asks to have opened is handed to your desktop through the framework's handoff, in a shell that starts the browser in the background and ends at once. qcode never spawns it itself any more, which is also why a test run of qcode cannot open anything on your screen.

## 0.1.9 - 2026-09-20

### Added

- Desktop harnesses, starting with Antigravity IDE. A profile of one opens its window from a container of its own: the window appears on your desktop, the project is at `/work/Project` inside it, and the profile's home volume keeps its settings and sign-in between starts. The tab has no terminal; it says where the window stands and offers the two things that can be done to it.
- Closing the tab closes the window and takes the container away, and closing the window ends the container, which the tab notices and offers to open it again. Quitting qcode closes any window it opened.
- The image is built on your own machine and fetches the application from its maker at install time, checked against the length and the SHA-256 the record names. It is about 1.4 GB, an order of magnitude larger than a command-line harness, and you are told so before you ask for it.
- The window is given one file of your machine: the compositor's socket, inside a runtime folder of the container's own. It gets the graphics device when there is one, and draws in software when there is not. The application's own sandbox runs inside the container on both engines; on Docker qcode passes a seccomp profile that allows just what the sandbox needs, and never `--no-sandbox`.

### Known gaps

- Bringing a window to the front reaches the running application but cannot actually raise it: on Wayland that needs an activation token from the compositor, and a terminal application has no way to obtain one.
- Signing in is not wired up yet. The application needs a browser, and a container has none; the note in the repository describes the remaining work.
- A desktop harness cannot be used with a profile that has no network, and it has no agent of qcode's in it, so it takes no part in tabs talking to each other.

## 0.1.8 - 2026-09-20

### Added

- Tabs can talk to each other. The agent in one harness tab lists the project's other agent tabs and sends them a message, through a small server qcode registers in each harness's own settings, beside anything you put there. It runs inside the container and reaches qcode through a socket in the project's `Containers/MCP/` folder, so a profile without the network can use it too. You approve the first message between any two tabs; a tab without the network never sends to one with it; and an exchange stops after 6 messages, with each tab sending at most 5 a minute. A message that is taken waits in the tab it was sent to, with **Read** and **Discard**; qcode does not type it into the receiving harness's prompt yet.

### Changed

- The picture that opens the README now shows an opencode tab answering a question about the project, with a Claude Code tab beside it, and is recorded with the framework's own recorder: 411 KB instead of 3.8 MB.

## 0.1.7 - 2026-09-19

### Added

- A PDF opened from the file tree shows its text in a tab (`pdftotext`). **Page picture** draws a page with `chafa`, **Previous** and **Next** walk through the pages, and a PDF with no text, such as a scan, opens on its first page picture.
- A Word or OpenDocument text (`docx`, `odt`) opens in a tab as its text (`docx2txt`, `odt2txt`).
- A sound (`mp3`, `ogg`, `oga`, `opus`, `flac`, `wav`) plays in a tab with `sox`, which shows how far it has got; **Play again** plays it once more. It plays in a short-lived container of its own that sees the project read-only, has no network and reaches only this machine's sound server (PulseAudio, or PipeWire through its PulseAudio socket). With no such server, as on macOS and Windows, the tab shows the sound's length, rate and channels instead.
- **Sounds** in **Settings**, **Built-in apps**: **Play** (the default) or **Details only**, for when no container should ever reach the sound server.

### Changed

- The base image carries `poppler-utils`, `docx2txt`, `odt2txt` and `sox` with its mp3, opus and PulseAudio formats, about 47 MB more. It is built again once, by itself, the first time it is needed.

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
