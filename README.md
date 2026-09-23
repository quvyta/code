# qcode

**Run Claude Code, opencode, Gemini CLI and Codex side by side in Podman or Docker containers, with tabs like a browser, from one terminal app.**

![qcode in forty seconds: Continue opens a workspace, a shell lists its files, an opencode tab answers a question about the workspace's own code while a Claude Code tab waits beside it on the strip, a new tab lists each profile's recent conversations, the side panel shows the containers and a file made in the shell appearing in the tree, three files are selected with their menu open, and the README opens in a tab of its own](https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/qcode.gif)

**quvyta-code** runs coding harnesses inside containers, from the terminal. You set up a
profile once, sign it in, and from then on open that harness in any of your workspaces in a few
seconds, moving between harnesses and shells the way you move between tabs. Nothing the harness
runs ever runs on your machine itself. qcode is part of the Quvyta family of terminal
applications, is built on [quvyta-framework](https://github.com/quvyta/framework) and is open
source under the MIT licence.

> **Beta.** qcode is new. It works on Linux with both Podman and Docker, but the interface, the
> files it writes and the way it names images and containers may still change between releases.
> Please report anything that looks wrong at <https://github.com/quvyta/code/issues>.

## What it does

- **Harnesses in containers.** Each harness runs in a container of its own, built from an image
  made for it. The container sees the workspace folder and, if you allow it, the workspace's assets
  folder and the network. Because the container is what keeps the work apart from your machine,
  the harness is set up to work without stopping to ask for permission.
- **Profiles.** A profile is one harness with its settings: which harness, which template (the
  bare harness, **QCode basic** or **QCode high**), what it signs in with and what its containers
  may reach. Building a profile builds its image; the build can be stopped at any time and a
  half-made image is removed.
- **Signing in once.** A profile signs in through the harness's own sign-in flow, run in a
  terminal inside a container. qcode then checks that the login file is really there before it
  keeps it. Each workspace that uses the profile gets its own copy of the login, so chat history,
  memory and settings never leak from one workspace into another. A copy can be refreshed from the
  profile later, and the profile can be signed out. A profile that runs on one of your own
  providers has nothing to sign in to.
- **Providers of your own.** An ollama server on your network or an OpenRouter account can stand
  in for a harness's own account, for Claude Code and opencode. The provider's key stays on your
  machine and never enters a container. See [Providers of your own](#providers-of-your-own).
- **Workspaces.** A workspace starts empty, from a copy of a folder, or from a git address (the clone
  runs inside a container, so git does not have to be installed on your machine). The workspace
  screen has tabs for shells, harnesses and files, and a side panel with the workspace's files, its
  details and its containers, which can be stopped and restarted from there.
- **Built-in apps.** A file opened from the file tree opens in a tab of its own, in the workspace's
  base container, so the programs that open it live in that container and none of them has to be
  installed on your machine. See [Built-in apps](#built-in-apps).
- **Containers stop when you are done.** When the last QCode closes, the containers it started
  are stopped, and an optional background service does the same after a crash. See
  [When QCode closes](#when-qcode-closes).
- **Podman or Docker.** Either engine works. qcode finds it, tells you when it is missing or not
  running, and works out the command that installs or starts it. You either run that command
  yourself or let qcode run it on a terminal inside the setup, where you watch it. Either way it
  is the one command you chose, run with the rights you already have; qcode never raises its own.

The harnesses qcode knows today:

| Harness | Account types |
|---|---|
| Claude Code | subscription, API key, a provider of your own |
| opencode | free models, subscription, API key, a provider of your own |
| Gemini CLI | API key |
| Codex CLI | subscription, API key |

Google closed Gemini CLI's "Login with Google" to personal accounts (Code Assist for
individuals, Google AI Pro and Ultra) on 18 June 2026, so a new Gemini CLI profile signs in with
an API key. A profile made earlier with a Google sign-in still loads, with a note saying so: that
sign-in keeps working for Gemini Code Assist Standard and Enterprise.

Each harness is installed from its own published package when a profile's image is built; qcode
does not ship or change any of them. The interface follows your system language (English and
Turkish are included) and uses the family's themes, icons, keys and mouse behaviour.

## Why qcode, and what else there is

qcode is an everyday place to work with several harnesses, not a security product. What it adds
is the whole working surface around the containers: profiles you sign in once, a login copy per
workspace, tabs, going back to an earlier conversation, a file tree and the containers' state in
one screen. Other good tools solve neighbouring problems:

| Tool | What it does | How qcode differs |
|---|---|---|
| Docker Sandboxes (`docker sandbox`) | Runs harnesses in Docker's microVMs | Its isolation is stronger than a container's; it needs Docker Desktop and is a command-line tool, without tabs, a workspace screen or a list of earlier conversations |
| Dagger container-use | Gives each task of a harness its own container and git branch, over MCP | A base for running tasks in parallel, with no interface of its own; needs Dagger and git |
| Dev containers | The route the Claude Code documentation suggests | Tied to an editor such as VS Code and set up by hand for each workspace |
| claude-squad and similar | Several harnesses side by side with tmux and git worktrees | No container: the harness still runs on your machine |
| Single-image scripts (claudebox and others) | One harness in one Docker image | No interface, profiles, per-workspace logins or conversations to go back to |

If what you need is the strongest possible wall between a harness and your machine, a microVM is
the better tool. If you want to use several harnesses every day without handing them your
machine, qcode is made for that.

## What the container protects, and what it does not

The harnesses run in an unattended mode, without asking before each command, because the
container is what keeps them away from your machine. It helps to know exactly where that wall is.

The container keeps the harness away from:

- your home folder, your other workspaces and every file qcode did not mount: a container sees
  only its workspace's `Work/` folder, its `Assets/` folder (read-only unless the profile allows
  writing) and its own home;
- other workspaces' logins and conversations: each workspace has its own copy of a profile's home;
- the container engine itself: its socket is never mounted, containers are not privileged, and
  processes inside run as your own user id, never as root on your machine.

It does not protect:

- **the workspace folder.** The harness can change or delete anything in `Work/`, and it is
  mounted straight from your disk. Keep your work in git and push it somewhere.
- **your data from leaving over the network.** A profile with network access (the default) can
  send anything it can read, the workspace included, anywhere. A profile can be set to have no
  network, but most harnesses need it to reach their model. A profile that runs on one of your
  providers can work with no network at all, and even then what the harness puts in its requests,
  which can be anything in the workspace, goes to that provider: qcode carries those requests
  there, and to nowhere else.
- **the logins.** A profile's login lives in the engine's volumes. Anyone who can use your
  container engine can read them.
- **against the engine or the kernel.** A container shares your machine's kernel; a flaw there
  or in the engine is a way out that a virtual machine would not have.

## No telemetry, and what goes over the network

qcode collects no statistics and sends nothing about you, your machine or your work anywhere.

It asks one question of its own accord: whether a newer qcode is out. When qcode starts, at most
once a day, it reads the list of published versions of `quvyta-code` from crates.io, the same file
`cargo install` reads: one HTTPS `GET` of `https://index.crates.io/qu/vy/quvyta-code`. The request
carries no cookie and no identifier; its headers are `Host: index.crates.io`, `User-Agent:
quvyta-code/<the version you run>`, `Accept: */*` and `Accept-Encoding: gzip`. crates.io sees, as with any connection, the
address it comes from. When a newer version is out, a notice says which one and how to update. When
there is no network, or crates.io does not answer within ten seconds, nothing is said and the next
day asks again. Nothing is asked while the first-run setup is open. The time of the last question is
kept in `~/.local/state/quvyta/code/update-check` on Linux.

To turn it off, switch off **Say when an update is out** in **Settings**. The switch belongs to the
whole Quvyta family: it is `update-notice = false` in `~/.config/quvyta/quvyta.conf`, and turning it
off stops the question in every Quvyta application. While it is off, qcode asks nothing at all.

Apart from that question, qcode connects to the network itself only after you have added a
provider on the **Providers** page, and then only to that provider's address:

- **When you ask on the Providers page.** **Try the connection**, **Ask what it offers** and
  **Measure the real window** each send their requests at the moment you press them, never
  because the page was opened or qcode started. Measuring sends up to four prompts, of 400 to
  48 000 words, and reads back how many tokens the provider counted; a provider that charges by
  the token charges for them like for any other prompt.
- **While a tab of a profile that runs on that provider is open.** The harness's requests for an
  answer and for the list of models are carried from its container to the provider by qcode,
  which adds the key on the way out. Nothing else the container asks for is carried, and the key
  never enters the container.

Two providers are ready-made: picking one fills in its address and you paste only your key. They
are the only addresses qcode knows of its own, and nothing is sent to either until you have added
it: **Xiaomi MiMo Token Plan** at `token-plan-ams.xiaomimimo.com`, `token-plan-sgp.xiaomimimo.com`
or `token-plan-cn.xiaomimimo.com`, whichever your subscription names, and **Kimi Code** at
`api.kimi.com` or `api.kimi.ai`. For them, **Try the connection** is a `GET` of the service's
model list with your key, which spends nothing.

A version of qcode before 0.1.13 said here that it had no network code at all. That stopped being
true in 0.1.12, which added providers, and the sentence was not changed with it.

All other traffic comes from programs you can see qcode start: your container engine, when it
builds an image (the base image, the harness packages and, for QCode high, what that template
adds) or clones a workspace from a git address; the command that installs a container engine,
when you let qcode run it in the setup; and your own browser, when a sign-in page is handed to
it. Inside the containers, the harnesses keep their own behaviour, including any telemetry of
their own; their documentation says what that is. Two are switched off by qcode's templates:
QCode basic turns off Antigravity's telemetry, and QCode high turns off that of oh-my-openagent,
which it adds.

## Screens

![A workspace open in qcode: a shell tab beside a Claude Code and an opencode tab, and the panel with the file tree, the workspace and its containers](https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/workspace.png)

<p>
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/new-tab.png" width="49%" alt="A new tab offering a shell and each profile's earlier conversations">
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/panel.png" width="49%" alt="The panel with the workspace's details and its containers">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/home.png" width="49%" alt="The home screen, ready to continue with the workspaces left open">
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/workspaces.png" width="49%" alt="The list of workspaces">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/apps.png" width="49%" alt="The workspace's README read in a Markdown tab, opened from the file tree">
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/files.png" width="49%" alt="Three files selected in the file tree, with the menu that cuts or deletes them">
</p>

## Requirements

- **A container engine:** [Podman](https://podman.io/docs/installation) (recommended: rootless,
  with no background service) or [Docker](https://docs.docker.com/engine/install/) with its daemon
  running.
- **An account** with the harness you want to use: a subscription or an API key from its provider,
  or, for Claude Code and opencode, a model service of your own (an ollama server or OpenRouter).
- **Disk space and a network connection** for the first images. The base image is Debian with
  Node.js (about 520 MB); each profile adds its harness on top of it. A profile can instead be
  built on Arch Linux (about 810 MB), Ubuntu 24.04 LTS (about 510 MB) or Alpine (about 310 MB,
  not recommended: Gemini CLI and Antigravity IDE do not run on it).
- Rust 1.95 or later to install from source.

qcode is developed and tested on Linux. The paths, engine checks and container settings for macOS
and Windows are written in, but have not yet been tried on those systems.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/quvyta/quvyta/main/install.sh | sh -s -- code
```

Or with Cargo:

```sh
cargo install quvyta-code
qcode
```

If the shell cannot find `qcode`, add `~/.cargo/bin` to your `PATH` (fish: `fish_add_path ~/.cargo/bin`).

The program is installed as `qcode` and also as `quvyta-code`.

## Using it

1. **Setup.** The first time qcode opens it asks three things: the language, the container engine
   and where the QCode folder goes. It checks each answer before going on, and checks them
   again every time it starts.
2. **A profile.** Open **Profiles** and make a new profile: pick the harness, the template, the
   account type and the permissions, then build the image. When it is built, sign in in the
   terminal that opens and press **I have signed in**.
3. **A workspace.** Open **Workspaces** and make a new workspace, empty, from a folder or from a git
   address.
4. **Tabs.** In the workspace, the `+` after the last tab (or `ctrl+t`) opens a new tab at once.
   It lists the shell of the workspace's own container and, for every profile, **New chat** and
   the profile's latest conversations in this workspace, newest first, with when each was last
   used. Choosing a conversation opens the harness on it again, where it left off; Claude Code,
   opencode, Gemini CLI and Codex CLI all resume a conversation this way. A profile the workspace
   does not have yet is added to it the moment you open it, and is written into the workspace's
   `workspace.qcode`. With no profile at all, the list offers **New profile**, which leads to the
   profiles screen.
5. **Continue.** The rail on the left holds the workspaces you have open, like the windows of a
   browser: `+` at its end adds another one, and each can be closed. **Continue** on the home
   screen brings back the open workspaces with their tabs as you left them, even after qcode was
   closed; a tab's container starts when you first switch to that tab.

| Key | What it does |
|---|---|
| `←` `→` (or `h` `l`) | Move between tabs, while the tab strip has the keyboard |
| `ctrl+shift+←` `ctrl+shift+→` | Move the open tab left or right |
| `ctrl+t` | Open a new tab |
| `ctrl+w` | Close the tab |
| `ctrl+alt+space` | Inside a harness or shell tab: leave it for the tab strip. Anywhere else on the workspace screen: go back into the open tab's terminal |
| `alt+b` | Show or hide the side panel |
| `tab` `shift+tab` | Move to the next or previous control; `shift+tab` also leaves a terminal |
| `ctrl+p` | Command palette |
| `esc` | Leave the screen that is open |
| `?` or `f1` | The list of every key |
| `ctrl+q` | Quit |

In the file tree of the side panel:

| Key | What it does |
|---|---|
| `↑` `↓` | Move between entries |
| `→` (or `l`) | Open a folder, or step into it when it is open |
| `←` (or `h`) | Close a folder, or step up to the folder above |
| `enter` | Open a folder, or open a file in a tab |
| `space` | Add the entry to the selection, or take it out |
| `shift+↑` `shift+↓` | Select a range of entries |
| `shift+f10` or the menu key | The entry's context menu |

While a harness or shell tab has the keyboard, keys go to it, `esc` and `?` included; `f1`, `alt+b`, `ctrl+alt+space`, `shift+tab` and `ctrl+q` still reach qcode. The mouse reaches a harness that uses it.

## Built-in apps

Every workspace has one small container of its own, the base container, which is also where the
shell tab runs. It starts the first time something needs it, so a session that opens no shell and
no file starts nothing. A file chosen in the file tree (`enter` or a click) opens in a new
tab named after the file; choosing it again goes back to that tab.

| File | Opens in |
|---|---|
| Text: `txt`, source code, configuration and data files, and files such as `README`, `LICENSE`, `Makefile` or `Dockerfile` | The editor chosen in **Settings**, **Built-in apps**: `nano` (the default) or `vim`. When the editor exits, the tab offers to open the file again |
| Markdown: `md`, `markdown` | qcode itself, as a formatted page with no container at all. **Edit** above the page opens the same file in the editor |
| Pictures: `png`, `jpg`, `jpeg`, `gif`, `webp` | `chafa`, which draws the picture in the tab with full colour. **Redraw** draws it again at the tab's new size |
| PDF: `pdf` | Its text, taken out by `pdftotext` and shown by qcode like a document. **Page picture** draws a page with `chafa`, and **Previous** and **Next** walk through the pages; **Text** goes back. A PDF with no text, such as a scan, opens on its first page picture |
| Word and OpenDocument text: `docx`, `odt` | Its text, taken out by `docx2txt` or `odt2txt` and shown by qcode like a document |
| Sound: `mp3`, `ogg`, `oga`, `opus`, `flac`, `wav` | `sox`, which plays it in the tab and shows how far it has got; **Play again** plays it once more. `m4a` and `aac` are not supported |

A sound does not play in the base container. Each time it plays, qcode starts a container of its
own for it, which sees the workspace read-only, has no network and reaches only this machine's
sound server (PulseAudio, or PipeWire through its PulseAudio socket). On a machine with no such
server, which includes macOS and Windows, the tab shows the sound's details (length, rate,
channels) instead. If no container should ever reach the sound server, set **Sounds** to
**Details only** in **Settings**, **Built-in apps**: every sound then shows its details and
nothing plays. The default is **Play**.

![The manual of a rain gauge read as text in a PDF tab, opened from the file tree](https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/pdf.png)

Other kinds of files (spreadsheets, presentations, archives) have no app yet; choosing one says
so. The base image also carries `unzip`, `zip`, `xz`, `bzip2` and `7z`, ready in the shell tab.
Every program runs inside the container, on the file's path there, and is handed that path as one
word, never through a shell. Open file tabs come back with **Continue** like any other tab; a file
that has gone since is reported in its tab.

## Desktop harnesses

Most harnesses draw in the tab's terminal. One does not: **Antigravity IDE** is a desktop
application, and qcode runs it the same way it runs the others — in a container, on your workspace
and nothing else — except that its window opens on your own screen instead of in a tab.

A profile for it is made like any other, in **Profiles**. The image is built on qcode's base image
and downloads the application from Google's own address while it builds; nothing of the
application is carried inside qcode. That download is about 230 MiB and the finished image about
1.4 GiB, so it is much larger than a command-line harness's, and the application uses well over a
gigabyte of memory while its window is open. The version is fixed in qcode, so updating the
application means building the image again.

The tab is one line of status with two things you can do to the window:

| The tab says | What it means |
|---|---|
| **Opening the window…** | The container is starting. The first time takes a few seconds longer |
| **Window open** | The window is on your screen. **Bring to front** asks it to show itself, **Close the window** closes it |
| **Window closed** | It is not open. **Open the window** opens it again. Nothing was lost: the settings, the history and the sign-in are in the workspace's home volume |
| **The window did not open** | Why, in the engine's own words |

The window is a container's, which is what makes it worth having and also where its limits come
from:

- **It needs Wayland.** The container is given the one socket file of your compositor and nothing
  else that lives beside it — not the engine's own socket, not the session bus, not the keyring.
  Because it is that one file, a compositor that restarts cuts an open window off; opening the tab
  again is the way back. X11 is not offered: there every program on the screen could read this
  one's windows and keypresses.
- **Graphics.** If your machine has `/dev/dri`, the container is given it and the application may
  use the card; without it, and on cards the application does not trust, it draws in software,
  which works and is not noticeably slower in the editor. NVIDIA's closed driver is not supported
  yet.
- **The application's own sandbox stays on.** qcode never passes `--no-sandbox`. Podman needs
  nothing extra for that. Docker's default seccomp profile refuses the calls the sandbox is built
  from, so qcode hands docker a profile of its own: docker's default plus `clone`, `setns` and
  `unshare`. It is in `assets/seccomp/desktop.json` with a comment saying what it costs.
- **Signing in happens inside the container.** The application cannot be used without a Google
  account. When it asks to sign in, the page opens in a small window inside the container, so
  Google's answer comes back to the application there and the sign-in completes; you sign in once
  per profile and it is remembered in that workspace. Your own browser is only the fallback when
  that window cannot start, and a sign-in from there cannot reach the application. A profile made
  with qcode 0.1.13 or earlier has no such window in its image: **Rebuild image** on the Profiles
  screen gives it one.
- **A profile with the network off makes no sense here.** The application does all its work on its
  maker's servers; the tab says so if you try.

Closing the tab closes the window and removes its container; closing the window ends the
container, which the tab notices and offers to open again. qcode quitting stops any open window,
like every other container it started. A container left over from a crash is found by name the
next time and either taken over, if its window is still up, or cleared away.

## File manager

The file tree of the side panel is also a file manager. It works on the workspace's own folder
directly on your machine, so it needs no container and works with no engine running.

- **Context menu.** Right-click an entry, or select it and press `shift+f10` or the menu key. On a
  folder: **New file**, **New folder**, **Rename**, **Cut**, **Paste here** (once something is cut)
  and **Delete**. On a file: **Rename**, **Cut** and **Delete**. The first row of the tree is the
  workspace folder itself; its menu has **New file**, **New folder**, **Paste here** and **Refresh**.
  With several entries selected, the menu cuts or deletes all of them. Folders and files also
  offer **Don't back up** (or **Back up again**), and files **Earlier versions**; see
  [Backups](#backups).
- **Names** are asked for in a small dialog and checked as you type: not empty, no `/`, not `.` or
  `..`, and not a name the folder already has. A rename opens with the name before its extension
  selected.
- **Moving** is **Cut**, then **Paste here** on the folder it goes to. A cut entry is drawn faded
  until it is pasted; `esc` or **Cancel the move** in the menu lets it stay. A folder cannot go
  into itself, and nothing is ever written over: when the target already has that name, nothing
  moves and the reason is shown.
- **Deleting** asks first and cannot be undone; for a folder the question says that everything in
  it goes too.
- **Several entries.** `ctrl`+click adds or removes an entry, `shift`+click selects a range, and
  `shift` with the arrow keys extends it; `space` adds or removes the entry under the cursor and
  `esc` goes back to one. Cut, paste, delete and dragging act on all of them.
- **Dragging** entries onto a folder, or onto the workspace folder's row, moves them there.
- **Live.** The tree follows the disk: the folders on screen are watched, and when something
  changes in one, by qcode, a harness or any other program, only that folder is read again.
  Nothing is read on a timer. Where the system has no watch to give (so far, anywhere but Linux),
  the tree is read again after qcode's own changes, when you come back to the screen, and on
  **Refresh**.

A link inside the workspace is handled as an entry of its own: deleting or moving it touches the
link, never what it points to.

## Backups

qcode keeps copies of every open workspace's `Work/` folder in `Backup/`, next to it in the
workspace's own folder, so the backup moves and is copied with the workspace.

- **How often.** Every 15 minutes while the workspace is open, once more when you close it from the
  rail, and once more when qcode quits. **Settings**, **Back up open workspaces** chooses **Off**,
  **5 min**, **15 min** or **1 hour**. A round in which nothing changed writes nothing.
- **How.** Each backup is a git commit in `Backup/Code.git`, made by git in a short-lived
  container of the base image, so nothing runs on your machine and your machine needs no git.
  Your own repository inside `Work/`, if you have one, is never touched, and what your
  `.gitignore` leaves out stays out.
- **Leaving things out.** Right-click a folder or a file in the file tree and choose
  **Don't back up**; **Back up again** takes it back in. The list is kept in `workspace.qcode`.
  Left-out entries are drawn faded with a coloured icon, and the **Workspace** part of the side panel
  names them.
- **Assets.** `Assets/` is left out unless you turn on **Back up Assets too** in the **Workspace**
  part of the side panel. It is then backed up in every round into `Backup/Assets.git`, apart
  from the workspace, and the choice is kept in `workspace.qcode`.
- **Conversations.** Each round also backs up the conversations of every profile whose tab was
  open since the round before, each profile into `Backup/Conversations/<profile>.git`. Only the
  harness's conversation files are taken, never its login. opencode keeps its conversations in a
  database, so they are backed up only while its container is stopped: when qcode quits and
  stops it.
- **Bringing things back.** **Backups** in the **Workspace** part of the side panel lists every
  backup with its time and how many files it changed; choose one to bring the workspace back to it.
  **Earlier versions** in a file's menu does the same for that one file. The choice at the top of
  the list switches it to the assets, when they are backed up, or to a profile's conversations.
  qcode asks first, then backs up how things are now, so bringing something back can be undone
  the same way. Nothing is deleted: a file made after that backup stays where it is.
  Conversations are brought back only while the profile's container is stopped; if it runs, qcode
  offers to stop it first.
- **What it is for.** A backup protects against a wrong delete, a change that breaks things or a
  harness scattering files. It sits on the same disk as the workspace, so it does not protect
  against losing the disk.

The **Workspace** part of the side panel also shows when the last backup was made and how much
`Backup/` holds. If a backup fails, qcode says so once for that workspace, not at every round.

![The list of a workspace's backups, the newest taken just before a restore, with the choice of the workspace's files, its assets or a profile's conversations above it](https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/backups.png)

## Tabs talking to each other

The agents in a workspace's harness tabs can hand each other work: the one in a Claude Code tab can
ask the one in a Codex tab to write a test, and hear back. qcode gives every harness two tools for
this, `list_tabs` and `send_message`, through a small MCP server it registers in each harness's
own settings, next to anything you added there. The server runs inside the container and talks
to qcode through a socket in the workspace's `Containers/MCP/` folder, so it works in a profile
without the network too.

Messages go from tab to tab without asking you: you set the agents to work, and handing it to
each other is part of that work. Two rules hold for every message all the same, and no setting
turns them off:

- **A tab without the network never sends to a tab with it.** The second tab could carry out what
  it is given, which is what taking the network away was meant to prevent. The other way round is
  allowed.
- **Loops stop.** An exchange between tabs ends after 6 messages, and one tab sends at most 5
  messages a minute, so two agents cannot keep each other busy, and spend your balance, forever.
  The sending agent is told why its message was refused. When an exchange is ended, both tabs say
  so in a line under their terminal until you press **Got it**, and a notice tells you once,
  whichever tab you are looking at.

If you would rather approve the first message between two tabs, turn on **Ask before the first
message** under **Messages between tabs** in **Settings**. The first message from one tab to
another then asks you, with the message shown. Your answer holds for those two tabs, in that
direction, until qcode closes, and is never written to disk. Esc denies. A pair you denied stays
denied until qcode closes, even if you turn asking off again.

A message that is taken is typed into the receiving harness's own prompt, on a line that says
which tab sent it, as soon as that tab is quiet: you are not typing in it and its program has
stopped writing. Until then, or while the harness is not running, it waits in the tab, where
**Read** shows it and **Discard** throws it away. The sending agent is told whether its message
went in or still waits. A desktop window (Antigravity) can send messages but never receives any,
because it has no prompt to type into.

## Providers of your own

**Providers**, beside **Profiles**, lists the model services you already have: an ollama server
on your own network, or OpenRouter. Each gets a tag of your choosing, and a key where the service
needs one, pasted into the dialog. The page shows, in a line of its own, which file the keys are
kept in and that a backup of your home folder carries them in plain text.

For each provider the page can try the connection, ask which models it offers and what window
each one claims, and measure the window the server really gives: a model may say 262 144 tokens
while the server quietly keeps three thousand and drops the front of everything larger. The page
shows both numbers. Each of these goes out only when you press its button; see
[what goes over the network](#no-telemetry-and-what-goes-over-the-network).

A Claude Code or opencode profile can then sign in with **a provider of your own** and one of its
models. Its tab talks to a small relay that runs inside the container, on the container's own
loopback address; the relay hands each request to qcode through a socket in the workspace's
`Containers/MCP/` folder, and qcode sends it on to the provider with the key added. So the
container never holds the key and needs no network of its own, and the harness is told the window
that was measured, so it does not assume room the server does not give. Only two kinds of request
are carried: a message and the list of models. Gemini CLI and Codex do not offer a provider yet,
because pointing them at another address has not been checked.

## When QCode closes

**Settings**, **When QCode closes** decides what the containers QCode started do once no QCode is
open any more: **Stop** (the default) or **Keep running**. Stopped containers are not removed;
the same container starts again the next time it is needed. Only containers QCode itself started
are ever stopped; one you run by hand, or another program's, is never touched.

Several QCodes can be open at once. When the last one closes normally, it stops the containers
itself and says which on the terminal it was started from.

**The background service** covers what a normal close cannot: a QCode that crashed or whose
terminal was killed. It is optional, and **Settings**, **Background service** installs and removes
it. On Linux it is a systemd user path unit that watches the list of the containers QCode
started; on macOS it is a launchd job that watches the same file. It runs only when that list
changes, then waits, without polling and without using the processor, until no QCode is open,
applies the setting, and exits. On a day QCode is never opened it never runs.

On Windows there is no background service, and the last QCode to close cannot tell it is the last,
so the containers keep running; Settings says so.

## Where things live

| What | Where |
|---|---|
| Settings | `code.conf` in the Quvyta folder of the platform's configuration folder: `~/.config/quvyta/code.conf` on Linux, `~/Library/Application Support/Quvyta/code.conf` on macOS, `%APPDATA%\Quvyta\code.conf` on Windows. Settings from before (`~/.config/quvyta/code/settings.toml`) move there once, at start |
| Open workspaces and tabs | `session.toml` in the platform's data folder, `~/.local/share/quvyta/code` on Linux |
| Containers QCode started | `containers.toml` in the same data folder, with `instances.lock`, which every open QCode holds |
| Background service | `~/.config/systemd/user/qcode-reaper.service` and `qcode-reaper.path` on Linux, `~/Library/LaunchAgents/io.quvyta.code.reaper.plist` on macOS, while it is installed |
| QCode folder | `Quvyta/Code` in your Documents folder by default (`~/Documents/Quvyta/Code`, or `~/Belgeler/Quvyta/Code` where the desktop names it so), or the folder you chose; a folder chosen before stays where it is |
| Profiles | `Profiles/<profile>.toml` in the QCode folder |
| Workspaces | `Workspaces/<workspace>/` in the QCode folder: `workspace.qcode`, the code in `Work/`, your material in `Assets/` |
| Tabs talking to each other | `Workspaces/<workspace>/Containers/MCP/` in the QCode folder: the server the harnesses start and, while the workspace is open, the socket qcode listens on; each harness's own settings in `qcode-home-<workspace>-<profile>` hold the entry `qcode` |
| Providers | `providers.toml` in the data folder, `~/.local/share/quvyta/code` on Linux, readable only by you in a folder only you can open. It holds the keys in plain text; they are never written to the settings file, the QCode folder or a container |
| The relay to a provider | `relay.sock` and `qcode-relay.mjs` in `Workspaces/<workspace>/Containers/MCP/`, beside the bridge's socket and server |
| Backups | `Workspaces/<workspace>/Backup/` in the QCode folder: `Code.git`, the backups of `Work/`; `Assets.git`, those of `Assets/` when it is backed up; `Conversations/<profile>.git`, each profile's conversations; and the lock files that keep two QCodes from backing up the same thing at once |
| Images | `qcode/base` and `qcode/profile/<profile>`, in the engine |
| Logins | engine volumes: `qcode-cred-<profile>` for the profile, `qcode-home-<workspace>-<profile>` for each workspace's copy |

Files are plain TOML. A file qcode cannot read is reported with its line and column instead of
stopping the program, and a broken workspace stays on the list so it can be repaired.

## Building from source

The toolchain is pinned by `rust-toolchain.toml`.

```sh
git clone https://github.com/quvyta/code
cd code
cargo run --bin qcode
```

`cargo test` needs no container engine. The tests that build images and run containers are
ignored by default; run them with a working engine:

```sh
QCODE_CONTAINER_TESTS=1 cargo test -- --ignored
```

Before your first commit, enable the checks (formatting, clippy, tests and docs):

```sh
git config core.hooksPath .githooks
```

[CONTRIBUTING.md](CONTRIBUTING.md) says more about tests and pull requests, and
[CHANGELOG.md](CHANGELOG.md) lists what changed in each release.

## Licence

MIT. See [LICENSE](LICENSE).
