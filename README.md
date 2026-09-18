# qcode

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
cargo install quvyta-code
qcode
```

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
4. **The profile in the project.** In this beta a project is given a profile by naming it in the
   project's `project.qcode`, below the lines that are already there:

   ```toml
   [[profile]]
   name = "claude-sub"
   ```

   A screen for this is planned.
5. **Tabs.** In the project, open a new tab: a shell in the project's own container, or a harness
   from one of its profiles. The first time a profile opens in a project, the project is given its
   own copy of the profile's login.

| Key | What it does |
|---|---|
| `←` `→` | Move between tabs (project screen) |
| `ctrl+w` | Close the tab |
| `alt+b` | Show or hide the side panel |
| `ctrl+p` | Command palette |
| `esc` | Leave the screen that is open |
| `ctrl+q` | Quit |

While a harness or shell tab has the keyboard, keys go to it; `esc` included.

## Where things live

| What | Where |
|---|---|
| Settings | `settings.toml` in the platform's configuration folder, `~/.config/quvyta/code` on Linux |
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
