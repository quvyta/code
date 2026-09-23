# Changelog

Every release of quvyta-code, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0, a minor release may
change files qcode writes, and the notes say so when it does.

## 0.1.16 - 2026-09-23

### Changed

- **Antigravity IDE signs in from your own browser.** Google refused the small sign-in window inside the container ("Couldn't sign you in — This browser or app may not be secure"). Now the sign-in page opens in your own browser, and when Google sends the browser back to `localhost`, qcode listens on that port on `127.0.0.1` (and `[::1]`) and carries the answer to the application in its container, without giving the container any network. The tab says which port it listens on, that the sign-in arrived, or that nothing came back within ten minutes. If another program already uses the port, the page is not opened and the tab says what to do. The window inside the container is still there as **Use the sign-in window here**.

### Verified

- **Kimi Code answers live.** Claude Code and opencode each answered through the ready-made Kimi Code provider (`kimi-for-coding`) in a container with no network, and the key was found nowhere inside it. 0.1.15 could only show the road, because the subscription's usage limit was reached that day.

## 0.1.15 - 2026-09-23

### Added

- **Rebuild a profile's image.** Every profile on the Profiles screen has **Rebuild image**. It asks once, then builds the image again from what qcode puts in it today, with the build shown as it runs. The workspaces' homes, the login and the conversations are kept; if the build fails or you stop it, the old image stays. Tabs that are open keep running on the old image until they are opened again. An image built by an earlier qcode says so. This is how an Antigravity profile made with 0.1.13 or earlier gets the sign-in window 0.1.14 added.
- **Two ready-made providers: Xiaomi MiMo Token Plan and Kimi Code.** On the Providers page, pick one and paste only your key: the address, the region, the header the key goes in and the models are filled in. The key stays in `providers.toml` and is added on its way out, never inside a container. Claude Code and opencode were run against MiMo in a container with no network; Kimi's road was checked up to the service, which answered with the subscription's usage limit that day.

### Changed

- **Gemini CLI asks only for an API key.** Google closed Gemini CLI's sign-in with a personal Google account ("This client is no longer supported for Gemini Code Assist for individuals"). A Gemini CLI profile's sign-in now offers only the key, and a profile made earlier with a Google sign-in says so in the list and when its tab opens, with the ways on: an API key, or Antigravity IDE.
- **Ctrl+Q asks once while an agent is at work.** When a tab of any open workspace has a harness running or starting, or a harness's window open, quitting (Ctrl+Q or **Quit** on the home screen) asks first, because quitting stops the containers and cuts off whatever the agents are doing. Their conversations are kept either way. **Stay** keeps everything; pressing Ctrl+Q again while the question is on screen quits. With no agent at work qcode quits at once, as before.
- **Continue shows the right count in every language.** With 21 workspaces open, Russian lost the "+20"; with one, Japanese showed "+0".

### Fixed

- **The README says what 0.1.14 already did.** Its Desktop harnesses section still said Antigravity's sign-in did not finish; it does, inside the container. The update check's headers now list `Host` too.
- A build's log is cleaned by the framework's own `printable` rather than a copy of it in qcode. Built against quvyta-framework 0.1.19.

## 0.1.14 - 2026-09-22

### Added

- **Choose the system a profile is built on.** The profile wizard has a System step: Debian (as before, and the default), Arch Linux, Ubuntu 24.04 LTS, or Alpine. Each shows the size of its image. Alpine is the smallest but is marked *not recommended*: it is built on musl rather than glibc, and Gemini CLI and Antigravity IDE do not run on it, so the wizard does not offer them there. Where a system lacks a tool, the wizard says so: Arch has no sound player (its package would bring about 470 MB of ffmpeg with it), Alpine has no reader for Word files, and Ubuntu's sound player does not read opus. Files you open from the file tree still open in the workspace's own Debian container, so none of this changes how they open. Profiles you already have stay exactly as they are.
- **When a profile's image is missing, qcode offers to build it.** Opening a tab whose image the engine does not have used to end in the engine's raw error, such as `short-name "qcode/profile/claude-code" did not resolve to an alias`. Now the tab says the image is not there, offers **Build it now**, shows the build, and opens the tab when it is done. qcode also never looks for its own images on the internet any more.
- **Switching between Podman and Docker takes your things with you.** When you change the engine in Settings, or when qcode starts and finds that the saved engine no longer answers but the other one does, a page lists the profiles whose images the new engine lacks, each with **Build**, and offers to **Copy** each profile's home, where its settings and sign-ins live, to the new engine. The copy on the old engine is never deleted, so you can go back. It also says how many qcode containers are left on the old engine and how much room they take; removing them is your choice.
- **When the engine refuses for a known reason, you read what to do.** An image that is missing, an engine that is not running, an account that is not in the `docker` group, or a Podman account without id ranges now gets a plain sentence and the line that fixes it, with the engine's own words underneath.
- **Antigravity signs in inside its own container.** The sign-in page opens in a small window inside the container instead of your browser, so Google's return to the application arrives and the sign-in completes. The first time, you sign in to Google once for that profile; after that it is remembered. Antigravity profiles made before this release still send the page to your browser, where the sign-in cannot come back; make the profile again to get the window.
- **qcode says when a newer version is out.** At start, at most once a day, it asks crates.io for the published versions of `quvyta-code` and, when one is newer than yours, shows which and how to update. The request carries the package name and, as its `User-Agent`, `quvyta-code/<your version>`; nothing about you or your machine. It is on by default and is turned off with **Say when an update is out** in **Settings**, a switch shared by every Quvyta application; while it is off, nothing is asked. It is the only connection qcode makes of its own accord, and the README's network section now says exactly what goes out.

### Changed

- Installing Docker from the first-run wizard now also starts its service and adds you to the `docker` group, and says that you need to log out and back in. A Podman account without the id ranges it needs is shown the line that adds them.
- Tabs send each other messages without asking you first. **Ask before the first message**, under **Messages between tabs** in **Settings**, brings the question back; a settings file written before it existed reads as not asking. A tab without the network still never sends to one with it, and an exchange still ends after 6 messages. When it does, both tabs now say so in a line under their terminal until you press **Got it**, and a notice tells you once, whichever tab you are looking at.
- The three buttons at the foot of the workspace rail have room around them: an empty row above the question mark keeps them apart from the last workspace, and an empty row between the settings and the way back keeps a hand going for one from landing on the other. The workspaces take every other row of the rail.

### Fixed

- Building a profile image with Docker could close qcode: Docker's progress lines carry carriage returns, and the build log fell over them. It no longer does.
- The Chinese and Russian screens called the Providers page by one name on the page and by another in the profile wizard that sends you there. Each language now uses one name everywhere: 提供方 in Chinese and Провайдеры in Russian, the words the wizard already used.
- The notes for 0.1.10 named Italian among the nine languages; the ninth is Japanese, and those notes now say so.

## 0.1.13 - 2026-09-21

### Fixed

- **The README said something that was not true, and it is corrected.** Its "No telemetry" section promised that qcode had no network code and no network library. That stopped being true in 0.1.12, which added providers, but the sentence stayed in 0.1.12's README on crates.io and GitHub: for one release, qcode described itself wrongly on a point of privacy. The section now says exactly what happens: qcode collects no statistics, checks for no updates and sends nothing of its own accord; it connects to the network only after you add a provider, only to that provider, and only when you press a button on the Providers page or while a tab of a profile that runs on that provider is open. Measuring a window sends up to four prompts, which a paid provider charges for, and the README now says that too.
- **OpenRouter really works.** A tab running on OpenRouter, and the "Measure the real window" button, were sending their requests to OpenRouter's website instead of its API, which answered with a web page. They now go to the API, whichever of OpenRouter's two addresses you wrote. Listing the models and trying the connection were already right, which is why the page looked fine. A Claude Code tab on one of OpenRouter's free models was then watched answering and using its tools in a container with no network, on Podman and on Docker, with the key never inside the container.
- When a free OpenRouter model is busy, the Providers page says the service is asking you to wait, in the provider's own words, instead of reporting a refusal.
- **Typing fast no longer loses letters.** When several keys arrived at once — a quick typist, a terminal multiplexer, a slow connection — only the last reached the field: a workspace named "demo" was made as "o". Every key now arrives.
- **Choose folder takes the folder you opened.** In "From a folder", opening a folder and pressing **Choose folder** took the first folder inside it. It now takes the one you opened, unless you moved to one inside it yourself.
- A click is only a click where it began. Releasing the mouse over a button that appeared under it while the button was held, such as **Finish** on the step that "Change" in the settings opens, no longer presses that button.

### Added

- **opencode can run on a provider of your own.** Like Claude Code, an opencode profile can use your ollama server or OpenRouter through the relay, in a container with no network and without the key, and it is told the window that was measured. It was watched answering and using its tools on an ollama server and on an OpenRouter free model. Gemini CLI and Codex do not offer it yet.
- The README describes the Providers page, which profiles can use a provider, where the key file lives, and how the relay keeps the key out of the container. The parts about tabs talking to each other and about signing in to a desktop window were out of date and now say what qcode does today.

## 0.1.12 - 2026-09-21

### Added

- **Tabs can give each other work.** An agent in one tab can see the other tabs of its workspace and hand one of them a task; the message lands in that agent's own prompt, the way you would have typed it, and it says which tab it came from. The first time one tab writes to another, qcode asks you, and your answer holds for those two tabs until qcode closes. It works for opencode and Claude Code, and was watched working in containers with no network at all. A desktop window (Antigravity) can send work to the tabs but cannot receive any, because it has no prompt to write into.
- **Two ready-made ways to set up a harness: QCode basic and QCode high.** QCode basic (the recommended one) writes the harness's settings into the image the way qcode runs it, so a new Claude Code tab opens straight at its prompt instead of stopping at questions about trusting the folder. QCode high adds the tools qcode's author works with: graphify, a map of your code the agent asks before it reads files, five Claude Code plugins (superpowers, context7, code-review, security-guidance, block-no-verify) and, for opencode, oh-my-openagent. Each of them is on by default and each can be switched off in the profile wizard; what is off is neither installed nor set up. The wizard says what is downloaded and that building the image needs the network, even when the profile's containers have none.
- With QCode high, graphify starts building its map of your workspace in the background the first time a tab opens, so the map the agent is told about is really there.
- **Providers of your own.** A new page beside Profiles: you write down the model services you already have — an ollama server on your own network, or OpenRouter — give each one a tag of your choosing, paste its key where one is needed, try the connection and read back the models it offers. The key is kept in a file of its own (`providers.toml` in qcode's data folder, readable only by you) and never in the settings file; the page says so in a line you cannot miss, because a backup of your home folder carries that file in plain text.
- qcode measures what a server really gives, rather than repeating what a model claims. A model's own record may say 262 144 tokens while the server quietly accepts three thousand and drops the front of everything larger, which makes an agent look forgetful for no reason anyone can see. The page shows both numbers.
- **A profile can sign in with one of your providers.** Beside a subscription, an API key and a free tier, a Claude Code profile can run on a provider you added and a model of it. The tab then talks to that provider through a relay: the request leaves your machine from qcode, not from the container, so the provider's key never enters the container at all — a container with no network can still use a model service on your network. The harness is also told the window that was measured, so it stops assuming room the server does not give.
- The choice is offered for Claude Code only. It is the one harness whose redirection is verified to work; offering it for the others would be a claim nobody checked.
- In the profile wizard, a model whose window was never measured gets a line saying that Claude Code will assume 200 000 tokens until you measure it on the Providers page.

### Changed

- Harness images are smaller: npm no longer leaves its download cache inside them. An opencode image loses more than 300 MB.
- The tagline under the logo says what qcode is for: coding agents, always inside a container.
- The folder your code lives in is called **Work** on the profile screens too, the same name it has on disk and (as `/work`) inside the container.

### Fixed

- The key file is kept the way it was promised: readable only by you, in a folder only you can open. A folder that already existed is narrowed when the file is saved, a warning no longer appears before there is any key to protect, a warning that was put right disappears, and the warning speaks your language.
- The Providers page keeps its buttons on screen when a server offers many models; the list of models scrolls inside the room that is left.
- The server address offered in the new-provider dialog is replaced when you type your own, and an address qcode could not send a request to is refused beside the field instead of failing later.
- Adding a provider from inside the profile wizard ("Go to Providers") and coming back now offers that provider, as the wizard says it will.
- Choosing **New profile** from a workspace's new tab opens the profile wizard at once, and a finished profile is the one selected in the list.
- The workspace panel shows a container as soon as a tab brings it up, and its "no container yet" sentence is no longer cut off at the panel's edge.
- The **Add a provider** button showed a broken icon name instead of its icon.
- The first-run engine step fits a terminal of 80 by 24 in every language, with no sentence cut off.

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

- qcode speaks nine languages: English, Turkish, German, Spanish, French, Japanese, Portuguese, Russian and Chinese. The language is asked for on the first screen and changed in **Settings**; the one you pick is the one qcode opens in next time. A gate keeps every language complete and keeps each one in its own words, so a file that quietly held English would not pass.
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
