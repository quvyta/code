# qcode

![A project open in qcode: a shell tab beside a Claude Code and an opencode tab, and the panel with the file tree, the project and its containers](https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/project.png)

**quvyta-code** runs coding agent harnesses inside containers, from the terminal. You set up a
profile once, sign it in, and from then on open that harness in any of your projects in a few
seconds, moving between harnesses and shells the way you move between tabs. Nothing the harness
runs ever runs on your machine itself. qcode is part of the Quvyta family of terminal
applications, is built on [quvyta-framework](https://github.com/quvyta/framework) and is open
source under the MIT licence.

> **Beta.** qcode is new. It works on Linux with both Podman and Docker, but the interface, the
> files it writes and the way it names images and containers may still change between releases.
> Please report anything that looks wrong at <https://github.com/quvyta/code/issues>.

## What it does

- **Harnesses in containers.** Each harness runs in a container of its own, built from an image
  made for it. The container sees the project folder and, if you allow it, the project's assets
  folder and the network. Because the container is what keeps the work apart from your machine,
  the harness is set up to work without stopping to ask for permission.
- **Profiles.** A profile is one harness with its settings: which harness, how much of qcode's
  recommended configuration goes into the image, what it signs in with and what its containers
  may reach. Building a profile builds its image; the build can be stopped at any time and a
  half-made image is removed.
- **Signing in once.** A profile signs in through the harness's own sign-in flow, run in a
  terminal inside a container. qcode then checks that the login file is really there before it
  keeps it. Each project that uses the profile gets its own copy of the login, so chat history,
  memory and settings never leak from one project into another. A copy can be refreshed from the
  profile later, and the profile can be signed out.
- **Projects.** A project starts empty, from a copy of a folder, or from a git address (the clone
  runs inside a container, so git does not have to be installed on your machine). The project
  screen has tabs for shells and harnesses, and a side panel with the project's files, its details
  and its containers, which can be stopped and restarted from there.
- **Podman or Docker.** Either engine works. qcode finds it, tells you when it is missing or not
  running, and shows the command that installs or starts it. It never installs anything and never
  raises its own rights.

The harnesses qcode knows today:

| Harness | Account types |
|---|---|
| Claude Code | subscription, API key |
| opencode | subscription, API key |
| Gemini CLI | subscription, API key |
| Codex CLI | subscription, API key |

Each harness is installed from its own published package when a profile's image is built; qcode
does not ship or change any of them. The interface follows your system language (English and
Turkish are included) and uses the family's themes, icons, keys and mouse behaviour.

## Screens

<p>
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/new-tab.png" width="49%" alt="A new tab offering a shell and each profile's earlier conversations">
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/panel.png" width="49%" alt="The panel with the project's details and its containers">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/home.png" width="49%" alt="The home screen, ready to continue with the projects left open">
  <img src="https://raw.githubusercontent.com/quvyta/code/main/docs/screenshots/projects.png" width="49%" alt="The list of projects">
</p>

## Requirements

- **A container engine:** [Podman](https://podman.io/docs/installation) (recommended: rootless,
  with no background service) or [Docker](https://docs.docker.com/engine/install/) with its daemon
  running.
- **An account** with the harness you want to use: a subscription or an API key from its provider.
- **Disk space and a network connection** for the first images. The base image is Debian with
  Node.js; each profile adds its harness on top of it.
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
   and where the workspace folder goes. It checks each answer before going on, and checks them
   again every time it starts.
2. **A profile.** Open **Profiles** and make a new profile: pick the harness, the template, the
   account type and the permissions, then build the image. When it is built, sign in in the
   terminal that opens and press **I have signed in**.
3. **A project.** Open **Projects** and make a new project, empty, from a folder or from a git
   address.
4. **Tabs.** In the project, the `+` after the last tab (or `ctrl+t`) opens a new tab at once.
   It lists the shell of the project's own container and, for every profile, **New chat** and
   the profile's latest conversations in this project, newest first, with when each was last
   used. Choosing a conversation opens the harness on it again, where it left off; Claude Code,
   opencode, Gemini CLI and Codex CLI all resume a conversation this way. A profile the project
   does not have yet is added to it the moment you open it, and is written into the project's
   `project.qcode`. With no profile at all, the list offers **New profile**, which leads to the
   profiles screen.
5. **Continue.** The rail on the left holds the projects you have open, like the windows of a
   browser: `+` at its end adds another one, and each can be closed. **Continue** on the home
   screen brings back the open projects with their tabs as you left them, even after qcode was
   closed; a tab's container starts when you first switch to that tab.

| Key | What it does |
|---|---|
| `←` `→` | Move between tabs (project screen) |
| `ctrl+t` | Open a new tab |
| `ctrl+w` | Close the tab |
| `alt+b` | Show or hide the side panel |
| `ctrl+p` | Command palette |
| `esc` | Leave the screen that is open |
| `ctrl+alt+space` | From a harness or shell tab back to the tab strip |
| `f1` | The list of every key |
| `ctrl+q` | Quit |

While a harness or shell tab has the keyboard, keys go to it, `esc` and `?` included; `f1`, `alt+b`, `ctrl+alt+space`, `shift+tab` and `ctrl+q` still reach qcode. The mouse reaches a harness that uses it.

## Where things live

| What | Where |
|---|---|
| Settings | `settings.toml` in the platform's configuration folder, `~/.config/quvyta/code` on Linux |
| Open projects and tabs | `session.toml` in the platform's data folder, `~/.local/share/quvyta/code` on Linux |
| Workspace | `QCode` in your documents folder by default (`~/Documents/QCode`), or the folder you chose |
| Profiles | `Profiles/<profile>.toml` in the workspace |
| Projects | `Projects/<project>/` in the workspace: `project.qcode`, the code in `Project/`, your material in `Assets/` |
| Images | `qcode/base` and `qcode/profile/<profile>`, in the engine |
| Logins | engine volumes: `qcode-cred-<profile>` for the profile, `qcode-home-<project>-<profile>` for each project's copy |

Files are plain TOML. A file qcode cannot read is reported with its line and column instead of
stopping the program, and a broken project stays on the list so it can be repaired.

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

## Licence

MIT. See [LICENSE](LICENSE).
