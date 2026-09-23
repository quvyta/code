//! The requests QCode makes of an engine, and the exact command each one becomes.
//!
//! Every function here is pure: it reads a request and returns a program with an argument list,
//! so the commands are checked in tests on a machine with no container runtime at all. Running
//! them is [`run`](super::run)'s job, and handing an interactive one to a pseudo-terminal is the
//! terminal layer's.

use super::Engine;
use super::dialect::{Relabel, Sandboxing, TmpfsOwner, UserMapping};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// A command to run: the engine binary and its arguments, ready for
/// [`std::process::Command`] or for `qframe`'s `TerminalSession::spawn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCommand {
    /// The engine binary.
    pub program: PathBuf,
    /// Its arguments, in order.
    pub args: Vec<OsString>,
}

/// Collects the arguments of one command.
struct Args(Vec<OsString>);

impl Args {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, arg: impl AsRef<OsStr>) {
        self.0.push(arg.as_ref().to_os_string());
    }

    fn extend<'a>(&mut self, args: impl IntoIterator<Item = &'a &'a str>) {
        self.0.extend(args.into_iter().map(OsString::from));
    }
}

impl Engine {
    fn command(&self, args: Args) -> EngineCommand {
        EngineCommand { program: self.bin.clone(), args: args.0 }
    }

    /// Builds the image `image` from `containerfile`.
    ///
    /// The build prints as it goes, so this is the one command
    /// [`run::stream`](super::run::stream) exists for.
    #[must_use]
    pub fn build_image(&self, request: &ImageBuild<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("build");
        args.push("--tag");
        args.push(request.image);
        args.push("--file");
        args.push(request.containerfile);
        args.push(request.context);
        self.command(args)
    }

    /// Builds the image `image` from `containerfile` again from its first step, reusing no layer
    /// an earlier build left behind.
    ///
    /// A layer is reused whenever the step that made it reads the same, and a step such as
    /// `npm install -g <harness>` reads the same while what it installs moves on; so an image
    /// the person asked to have built again would come back as it was. Neither engine looks for
    /// the image it starts `FROM` on a registry because of this: measured on both, `--no-cache`
    /// only stops the reuse.
    #[must_use]
    pub fn rebuild_image(&self, request: &ImageBuild<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("build");
        args.push("--no-cache");
        args.push("--tag");
        args.push(request.image);
        args.push("--file");
        args.push(request.containerfile);
        args.push(request.context);
        self.command(args)
    }

    /// Removes the image `image`, running containers and all.
    #[must_use]
    pub fn remove_image(&self, image: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("image");
        args.push("rm");
        args.push("--force");
        args.push(image);
        self.command(args)
    }

    /// Removes the image `image` only when no container is made from it: the engine refuses
    /// otherwise, and that refusal is the answer wanted. It is how an image a rebuild replaced
    /// goes once nothing uses it any more.
    #[must_use]
    pub fn remove_unused_image(&self, image: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("image");
        args.push("rm");
        args.push(image);
        self.command(args)
    }

    /// Asks for the value of one label of an image. The image being absent is a failure; a
    /// label it does not carry comes back empty or as `<no value>`, depending on the engine.
    #[must_use]
    pub fn image_label(&self, image: &str, label: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("image");
        args.push("inspect");
        args.push("--format");
        args.push(format!("{{{{index .Config.Labels \"{label}\"}}}}"));
        args.push(image);
        self.command(args)
    }

    /// How many bytes an image takes, as the engine reports it. Both engines answer the same
    /// field of the same command.
    ///
    /// It is what keeps the size QCode promises a person honest: a record that says how large a
    /// desktop image is can go stale silently, and a stale number is a promise broken before the
    /// download starts.
    #[must_use]
    pub fn image_size(&self, image: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("image");
        args.push("inspect");
        args.push("--format");
        args.push("{{.Size}}");
        args.push(image);
        self.command(args)
    }

    /// Asks after an image. It answers only when the image is there, which is how a profile
    /// knows whether it still has one to start containers from.
    #[must_use]
    pub fn image_exists(&self, image: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("image");
        args.push("inspect");
        args.push("--format");
        args.push("{{.Id}}");
        args.push(image);
        self.command(args)
    }

    /// Creates a container without starting it.
    #[must_use]
    pub fn create_container(&self, request: &ContainerCreate<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("create");
        args.push("--name");
        args.push(request.name);
        // Spelled the same by both engines, like the network below.
        args.push("--hostname");
        args.push(request.hostname);
        for (key, value) in request.labels {
            args.push("--label");
            args.push(format!("{key}={value}"));
        }
        self.contained(
            &mut args,
            &Contained {
                image: request.image,
                mounts: request.mounts,
                network: request.network,
                user: request.user,
                workdir: request.workdir,
                command: request.command,
            },
        );
        self.command(args)
    }

    /// Runs a container that exists only for as long as its command does, and lets
    /// [`run::capture`](super::run::capture) read what the command printed.
    ///
    /// A lasting container takes its mounts when it is created, so a job that needs other
    /// mounts than the workspace's containers have — a backup writing to `Backup/` — would mean
    /// making those containers again. A container that removes itself when its command ends
    /// needs nothing made again, and nothing is left behind to be cleared away.
    #[must_use]
    pub fn run_once(&self, request: &RunOnce<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("run");
        args.push("--rm");
        self.contained(
            &mut args,
            &Contained {
                image: request.image,
                mounts: request.mounts,
                network: request.network,
                user: request.user,
                workdir: request.workdir,
                command: request.command,
            },
        );
        self.command(args)
    }

    /// [`Engine::run_once`] with the command's standard input handed to the program, for a
    /// program that reads what another one writes: the half of a copy between two engines that
    /// unpacks what the other half packed.
    #[must_use]
    pub fn run_fed(&self, request: &RunOnce<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("run");
        args.push("--rm");
        args.push("--interactive");
        self.contained(
            &mut args,
            &Contained {
                image: request.image,
                mounts: request.mounts,
                network: request.network,
                user: request.user,
                workdir: request.workdir,
                command: request.command,
            },
        );
        self.command(args)
    }

    /// Runs a program in a container of its own, attached to a terminal and removed as soon as
    /// the program ends.
    ///
    /// It is [`Engine::run_once`] for a program a person watches and stops: a tab spawns it in a
    /// pseudo-terminal, the program draws there and reads its keys, and `ctrl+c` reaches it. The
    /// container has a name so that closing the tab can take it away while the program still
    /// runs: an engine client that is killed leaves its container behind, running.
    #[must_use]
    pub fn run_attached(&self, request: &RunAttached<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("run");
        args.push("--rm");
        args.push("--interactive");
        args.push("--tty");
        args.push("--name");
        args.push(request.name);
        for (key, value) in request.env {
            args.push("--env");
            args.push(format!("{key}={value}"));
        }
        for socket in request.sockets {
            args.push("--volume");
            args.push(socket.spelled());
        }
        let once = &request.once;
        self.contained(
            &mut args,
            &Contained {
                image: once.image,
                mounts: once.mounts,
                network: once.network,
                user: once.user,
                workdir: once.workdir,
                command: once.command,
            },
        );
        self.command(args)
    }

    /// Runs a program in a container of its own that draws a window on the person's desktop, in
    /// the background, and answers with the container's id.
    ///
    /// It is neither [`Engine::run_once`] nor [`Engine::run_attached`]: nothing reads what it
    /// prints and nothing types at it, because the person works in the window. The container is
    /// not removed when the program ends either — the tab wants to see that the window closed, and
    /// the exit code with it, before the container goes away.
    ///
    /// What each option is for is in [`RunWindow`]'s fields. The container's first process is the
    /// engine's own init rather than the program, so that the program's children are reaped and
    /// the stop signal reaches it.
    #[must_use]
    pub fn run_window(&self, request: &RunWindow<'_>) -> EngineCommand {
        let dialect = self.kind.dialect();
        let mut args = Args::new();
        args.push("run");
        args.push("--detach");
        args.push("--init");
        args.push(format!("--shm-size={}", request.shm));
        args.push("--name");
        args.push(request.name);
        if dialect.sandboxing == Sandboxing::NeedsProfile
            && let Some(profile) = request.seccomp
        {
            let mut option = OsString::from("seccomp=");
            option.push(profile);
            args.push("--security-opt");
            args.push(option);
        }
        for tmpfs in request.tmpfs {
            args.push("--tmpfs");
            args.push(tmpfs.spelled(dialect.tmpfs_owner, request.once.user));
        }
        for device in request.devices {
            args.push("--device");
            args.push(device);
        }
        for (key, value) in request.env {
            args.push("--env");
            args.push(format!("{key}={value}"));
        }
        for socket in request.sockets {
            args.push("--volume");
            args.push(socket.spelled());
        }
        let once = &request.once;
        self.contained(
            &mut args,
            &Contained {
                image: once.image,
                mounts: once.mounts,
                network: once.network,
                user: once.user,
                workdir: once.workdir,
                command: once.command,
            },
        );
        self.command(args)
    }

    /// What a container printed, which is the only account of why a window never came up: the
    /// application writes its complaints to its output, and once the container is gone so are
    /// they. Both engines spell it the same way.
    #[must_use]
    pub fn container_logs(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("logs");
        args.push(name);
        self.command(args)
    }

    /// Waits for a container to end and answers its exit code.
    ///
    /// This is how QCode learns that a window was closed by the person: the container's only
    /// program is the application, so the container ends exactly when the window does, and no
    /// compositor has to be asked anything. Both engines spell it the same way.
    #[must_use]
    pub fn wait_container(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("wait");
        args.push(name);
        self.command(args)
    }

    /// Stops a running container, giving its program `seconds` to close by itself before it is
    /// killed.
    ///
    /// [`Engine::stop_container`] leaves the engine's own default in place, which is fine for a
    /// container running nothing but a keep-alive loop. A window has state to write out, so it is
    /// given a stated amount of time rather than whatever the engine on the machine defaults to.
    #[must_use]
    pub fn stop_container_within(&self, name: &str, seconds: u32) -> EngineCommand {
        let mut args = Args::new();
        args.push("stop");
        args.push("--time");
        args.push(seconds.to_string());
        args.push(name);
        self.command(args)
    }

    /// The part of `create` and `run` both engines take the same way: who the container runs
    /// as, what it reaches, what it sees, and the image with its command last.
    fn contained(&self, args: &mut Args, request: &Contained<'_>) {
        let dialect = self.kind.dialect();
        match (dialect.user_mapping, &request.user) {
            (UserMapping::KeepId, HostUser::Ids { .. }) => args.push("--userns=keep-id"),
            (UserMapping::Ids, HostUser::Ids { uid, gid }) => {
                args.push("--user");
                args.push(format!("{uid}:{gid}"));
            }
            (_, HostUser::ImageDefault) => {}
        }
        // Both engines spell the network the same way, and full access is the default, so this
        // is a property of the request rather than a difference between the engines.
        match request.network {
            Network::Full => {}
            Network::None => args.push("--network=none"),
        }
        for mount in request.mounts {
            args.push("--volume");
            args.push(mount.spelled(dialect.relabel));
        }
        if let Some(workdir) = request.workdir {
            args.push("--workdir");
            args.push(workdir);
        }
        // Every image a container of QCode's is made from is one QCode built on this machine, so
        // an engine that does not have it must say so rather than look for it elsewhere. Without
        // this both engines go to a registry: podman then refuses with a sentence about a
        // "short-name" and a missing registries.conf that tells the person nothing, and docker
        // asks Docker Hub for a repository of that name. Naming the image `localhost/...` instead
        // does not help: podman then retries a registry at localhost:443 three times, and docker
        // takes `localhost` for a registry host and no longer finds images tagged without it.
        args.push("--pull=never");
        args.push(request.image);
        args.extend(request.command);
    }

    /// Starts an existing container.
    #[must_use]
    pub fn start_container(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("start");
        args.push(name);
        self.command(args)
    }

    /// Stops a running container.
    #[must_use]
    pub fn stop_container(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("stop");
        args.push(name);
        self.command(args)
    }

    /// Removes a container, stopping it first if it runs.
    #[must_use]
    pub fn remove_container(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("rm");
        args.push("--force");
        args.push(name);
        self.command(args)
    }

    /// Runs a command inside a running container, attached to a terminal.
    ///
    /// This is what a tab spawns in a pseudo-terminal, so it always asks for an interactive
    /// session: the harness inside draws and reads keys itself.
    #[must_use]
    pub fn exec(&self, request: &Exec<'_>) -> EngineCommand {
        self.exec_with_env(request, &[])
    }

    /// [`Engine::exec`] with environment variables for the program, as `(name, value)`.
    ///
    /// A harness tab is started this way: the variable that says which tab it is reaches the
    /// harness and, through it, whatever the harness starts.
    #[must_use]
    pub fn exec_with_env(&self, request: &Exec<'_>, env: &[(&str, &str)]) -> EngineCommand {
        let mut args = Args::new();
        args.push("exec");
        args.push("--interactive");
        args.push("--tty");
        for (key, value) in env {
            args.push("--env");
            args.push(format!("{key}={value}"));
        }
        args.push(request.container);
        args.extend(request.command);
        self.command(args)
    }

    /// Runs a command inside a running container without a terminal but with its standard input
    /// open, for [`run::feed`](super::run::feed) to write into.
    ///
    /// A file is written into a home volume this way: the text goes through the pipe, where an
    /// argument would be cut off at the system's limit on one argument's length.
    #[must_use]
    pub fn exec_reading(&self, request: &Exec<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("exec");
        args.push("--interactive");
        args.push(request.container);
        args.extend(request.command);
        self.command(args)
    }

    /// Runs a command inside a running container without a terminal, and lets
    /// [`run::capture`](super::run::capture) read its output and its exit code.
    ///
    /// [`Engine::exec`] is for a tab: the harness draws and reads keys, so it needs a terminal
    /// and QCode gives it a pseudo-terminal. Everything else — a check, a copy, a clone — wants
    /// the exit code and the output, and asking for a terminal there costs a pseudo-terminal or,
    /// on docker, fails outright, because docker refuses `--tty` when the stream behind it is
    /// not a terminal. Neither `--interactive` nor `--tty` is passed: nothing types into such a
    /// command, and what it prints is read from the pipes.
    #[must_use]
    pub fn exec_without_terminal(&self, request: &Exec<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("exec");
        args.push(request.container);
        args.extend(request.command);
        self.command(args)
    }

    /// Asks for one container's state; the output is a word
    /// [`ContainerState::parse`](super::ContainerState::parse) reads.
    #[must_use]
    pub fn container_state(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("container");
        args.push("inspect");
        args.push("--format");
        args.push("{{.State.Status}}");
        args.push(name);
        self.command(args)
    }

    /// Asks which image a container was made from, by the image's identity rather than its
    /// name: the same thing [`image_exists`](Self::image_exists) answers about a name, spelled
    /// the same way on each engine (measured: podman gives the bare digest to both questions,
    /// Docker `sha256:` and the digest to both).
    #[must_use]
    pub fn container_image(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("container");
        args.push("inspect");
        args.push("--format");
        args.push("{{.Image}}");
        args.push(name);
        self.command(args)
    }

    /// Asks for the value of one label of a container. A label the container does not carry
    /// comes back empty or as `<no value>`, depending on the engine.
    #[must_use]
    pub fn container_label(&self, name: &str, label: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("container");
        args.push("inspect");
        args.push("--format");
        args.push(format!("{{{{index .Config.Labels \"{label}\"}}}}"));
        args.push(name);
        self.command(args)
    }

    /// Lists every container, running or not, as
    /// [`Container::parse_list`](super::Container::parse_list) reads it.
    #[must_use]
    pub fn list_containers(&self) -> EngineCommand {
        let mut args = Args::new();
        args.push("ps");
        args.push("--all");
        args.push("--format");
        args.push("{{.Names}}\t{{.State}}");
        self.command(args)
    }

    /// Lists the containers that carry `label`, one name a line: the containers QCode made
    /// from a plan carry [`PLAN_LABEL`](crate::ui::workspace::PLAN_LABEL), which tells them apart
    /// from anything else on the engine that merely has a name beginning like theirs.
    #[must_use]
    pub fn list_labelled(&self, label: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("ps");
        args.push("--all");
        args.push("--filter");
        args.push(format!("label={label}"));
        args.push("--format");
        args.push("{{.Names}}");
        self.command(args)
    }

    /// How many bytes a container holds of its own, above its image, as both engines report it
    /// in bytes.
    #[must_use]
    pub fn container_size(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("container");
        args.push("inspect");
        args.push("--size");
        args.push("--format");
        args.push("{{.SizeRw}}");
        args.push(name);
        self.command(args)
    }

    /// Creates a named volume.
    #[must_use]
    pub fn create_volume(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("volume");
        args.push("create");
        args.push(name);
        self.command(args)
    }

    /// Removes a named volume. A volume still in use by a container fails loudly rather than
    /// taking the container with it.
    #[must_use]
    pub fn remove_volume(&self, name: &str) -> EngineCommand {
        let mut args = Args::new();
        args.push("volume");
        args.push("rm");
        args.push(name);
        self.command(args)
    }

    /// Lists the names of every volume, one per line.
    #[must_use]
    pub fn list_volumes(&self) -> EngineCommand {
        let mut args = Args::new();
        args.push("volume");
        args.push("ls");
        args.push("--format");
        args.push("{{.Name}}");
        self.command(args)
    }

    /// Copies from the host into a container, which is how content reaches a volume: a container
    /// with the volume mounted is the volume's door.
    #[must_use]
    pub fn copy_in(&self, request: &CopyIn<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("cp");
        args.push(request.host);
        args.push(at(request.container, request.target));
        self.command(args)
    }

    /// Copies out of a container onto the host, the other direction of [`Engine::copy_in`].
    #[must_use]
    pub fn copy_out(&self, request: &CopyOut<'_>) -> EngineCommand {
        let mut args = Args::new();
        args.push("cp");
        args.push(at(request.container, request.source));
        args.push(request.host);
        self.command(args)
    }
}

/// `container:path`, how both engines name a place inside a container in a copy.
fn at(container: &str, path: &Path) -> OsString {
    let mut spelled = OsString::from(container);
    spelled.push(":");
    spelled.push(path);
    spelled
}

/// Building the image a profile or the base lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageBuild<'a> {
    /// The name the image gets, e.g. `qcode/profile/claude-sub`.
    pub image: &'a str,
    /// The Containerfile to build.
    pub containerfile: &'a Path,
    /// The folder the build can read files from.
    pub context: &'a Path,
}

/// Creating the lasting container of a workspace, on its own or with a profile in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerCreate<'a> {
    /// The container's name, e.g. `qcode-myworkspace-claude-sub`.
    pub name: &'a str,
    /// What the container reports as its machine name. QCode always passes
    /// [`names::HOSTNAME`](super::names::HOSTNAME), so that a login keyed to the machine name
    /// survives the move from the sign-in container to a workspace's.
    pub hostname: &'a str,
    /// Labels the container carries, as `(name, value)`; a workspace's container carries the
    /// digest of the request it was made from, which is how a changed request is noticed.
    pub labels: &'a [(&'a str, &'a str)],
    /// The image it starts from.
    pub image: &'a str,
    /// What the container can see of the host and of its volumes.
    pub mounts: &'a [Mount<'a>],
    /// Whether the harness inside reaches the network.
    pub network: Network,
    /// Who the container runs as.
    pub user: HostUser,
    /// Where a shell inside starts.
    pub workdir: Option<&'a Path>,
    /// What the container runs. It has to be something long-lived: tabs `exec` into the
    /// container that is already up rather than starting one each time.
    pub command: &'a [&'a str],
}

/// Running a command in a container of its own that is removed as soon as the command ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOnce<'a> {
    /// The image it starts from.
    pub image: &'a str,
    /// What the container can see of the host and of its volumes.
    pub mounts: &'a [Mount<'a>],
    /// Whether the command reaches the network.
    pub network: Network,
    /// Who the container runs as.
    pub user: HostUser,
    /// Where the command starts.
    pub workdir: Option<&'a Path>,
    /// The command and its arguments.
    pub command: &'a [&'a str],
}

/// Running a program in a container of its own, attached to a terminal, removed when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAttached<'a> {
    /// The container's name, by which it is removed when the program is not waited for.
    pub name: &'a str,
    /// The image, mounts, network, user, directory and command, as for a one-off container.
    pub once: RunOnce<'a>,
    /// Environment variables the program is given, as `(name, value)`.
    pub env: &'a [(&'a str, &'a str)],
    /// Sockets of this machine the program may talk to.
    pub sockets: &'a [Socket<'a>],
}

/// Running a program in a container of its own that draws a window on the person's desktop.
///
/// Every field beyond [`RunOnce`] is there because a window needs it, and each one is as narrow as
/// it can be: the container is given one socket, not the folder it sits in, and one device, not the
/// machine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunWindow<'a> {
    /// The container's name, by which the window is raised, stopped and waited for.
    pub name: &'a str,
    /// The image, mounts, network, user, directory and command, as for a one-off container.
    pub once: RunOnce<'a>,
    /// Environment variables the program is given, as `(name, value)`.
    pub env: &'a [(&'a str, &'a str)],
    /// Sockets of this machine the program may talk to: for a window, the compositor's, and
    /// nothing else that lives beside it.
    pub sockets: &'a [Socket<'a>],
    /// Private, writable filesystems the container gets of its own, which is where the sockets
    /// above are mounted into: a folder of this machine's own would bring everything else in it.
    pub tmpfs: &'a [Tmpfs<'a>],
    /// Devices of this machine the program may use, by their path, such as a graphics card.
    pub devices: &'a [&'a Path],
    /// How large the container's shared memory is, spelled as the engines take it (`1g`).
    ///
    /// Both engines give 64 MB, which a browser engine fills in seconds: the trial watched the
    /// window stop answering, the update check fail for want of resources and the program ignore
    /// the stop signal, all from that one number.
    pub shm: &'a str,
    /// The seccomp profile to run under. The engine that needs none ignores it.
    pub seccomp: Option<&'a Path>,
}

/// A private, writable filesystem a container gets of its own at a path inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tmpfs<'a> {
    /// Where it appears inside the container.
    pub target: &'a Path,
    /// Its permissions, spelled as `mount` takes them (`0700`).
    pub mode: &'a str,
}

impl Tmpfs<'_> {
    /// The filesystem as an engine spells its option: `target:options`.
    ///
    /// It has to belong to whoever runs inside — an application refuses a runtime directory that
    /// is not its own — and the two engines are told that differently.
    fn spelled(&self, owner: TmpfsOwner, user: HostUser) -> OsString {
        let mut spelled = self.target.as_os_str().to_os_string();
        spelled.push(format!(":rw,mode={}", self.mode));
        match (owner, user) {
            (TmpfsOwner::Mapped, HostUser::Ids { .. }) => spelled.push(",U"),
            (TmpfsOwner::Ids, HostUser::Ids { uid, gid }) => spelled.push(format!(",uid={uid},gid={gid}")),
            (_, HostUser::ImageDefault) => {}
        }
        spelled
    }
}

/// A socket of this machine, made reachable inside a container at a path of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Socket<'a> {
    /// Where the socket is on this machine.
    pub host: &'a Path,
    /// Where it appears inside the container.
    pub target: &'a Path,
}

impl Socket<'_> {
    /// The socket as both engines spell its mount: `host:target:rw`.
    ///
    /// Never relabelled, unlike a folder: relabelling changes the label of the file on this
    /// machine, and a socket another program serves is that program's, not the container's to
    /// relabel. Writable, because talking to a socket is writing to it.
    fn spelled(&self) -> OsString {
        let mut spelled = self.host.as_os_str().to_os_string();
        spelled.push(":");
        spelled.push(self.target);
        spelled.push(":rw");
        spelled
    }
}

/// What a created container and a one-off one are both made of.
struct Contained<'a> {
    image: &'a str,
    mounts: &'a [Mount<'a>],
    network: Network,
    user: HostUser,
    workdir: Option<&'a Path>,
    command: &'a [&'a str],
}

/// Running a command in a container that is already up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exec<'a> {
    /// The running container.
    pub container: &'a str,
    /// The command and its arguments.
    pub command: &'a [&'a str],
}

/// Copying from the host into a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyIn<'a> {
    /// What to copy. A folder path ending in `/.` copies the folder's content rather than the
    /// folder itself, which is what refreshing a credential from a profile wants.
    pub host: &'a Path,
    /// The container to copy into.
    pub container: &'a str,
    /// Where the copy lands inside it.
    pub target: &'a Path,
}

/// Copying out of a container onto the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyOut<'a> {
    /// The container to copy from.
    pub container: &'a str,
    /// What to copy out; `/.` at the end copies content rather than the folder.
    pub source: &'a Path,
    /// Where it lands on the host.
    pub host: &'a Path,
}

/// One thing the container can see at a path of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mount<'a> {
    /// What is mounted.
    pub source: MountSource<'a>,
    /// Where it appears inside the container.
    pub target: &'a Path,
    /// Whether the container may write to it.
    pub access: Access,
}

impl Mount<'_> {
    /// The mount as the engine spells it: `source:target:options`.
    fn spelled(&self, relabel: Relabel) -> OsString {
        let mut spelled = match self.source {
            MountSource::Path(path) => path.as_os_str().to_os_string(),
            MountSource::Volume(name) => OsString::from(name),
        };
        spelled.push(":");
        spelled.push(self.target);
        spelled.push(match self.access {
            Access::ReadWrite => ":rw",
            Access::ReadOnly => ":ro",
        });
        if relabel == Relabel::Shared {
            spelled.push(",z");
        }
        spelled
    }
}

/// What a mount brings into the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountSource<'a> {
    /// A folder on the host, e.g. the workspace's own files.
    Path(&'a Path),
    /// A named volume, e.g. a profile's credential or a workspace's home.
    Volume(&'a str),
}

/// Whether a mount may be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The container may write.
    ReadWrite,
    /// The container may only read.
    ReadOnly,
}

/// Whether a container reaches the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Everything the host reaches.
    Full,
    /// Nothing.
    None,
}

/// Who a container runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostUser {
    /// The person's own ids. Files the harness writes into the workspace then belong to them and
    /// not to root.
    Ids {
        /// User id.
        uid: u32,
        /// Group id.
        gid: u32,
    },
    /// No POSIX ids to map: on Windows the engine's virtual machine owns that side of the
    /// mount, and the container runs as the user its image names.
    ImageDefault,
}

impl HostUser {
    /// Who QCode is running as.
    ///
    /// # Errors
    ///
    /// On Unix, when the temporary folder cannot be written to, which is also where a build
    /// would fail later.
    pub fn current() -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            use std::time::{SystemTime, UNIX_EPOCH};

            // A file QCode creates is owned by QCode: its owner is the answer, and reading it
            // back needs neither libc nor a child process.
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
            let probe = std::env::temp_dir().join(format!("qcode-user-{}-{stamp}", std::process::id()));
            std::fs::File::create(&probe)?;
            let owner = std::fs::metadata(&probe).map(|meta| Self::Ids { uid: meta.uid(), gid: meta.gid() });
            let _ = std::fs::remove_file(&probe);
            owner
        }
        #[cfg(not(unix))]
        {
            Ok(Self::ImageDefault)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Engine, EngineKind};
    use crate::engine::{
        Access, ContainerCreate, CopyIn, CopyOut, Exec, HostUser, ImageBuild, Mount, MountSource, Network, RunAttached,
        RunOnce, RunWindow, Socket, Tmpfs,
    };
    use std::path::Path;

    fn podman() -> Engine {
        Engine::new(EngineKind::Podman, "/usr/bin/podman")
    }

    fn docker() -> Engine {
        Engine::new(EngineKind::Docker, "/usr/bin/docker")
    }

    fn args(command: &crate::engine::EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn builds_an_image_from_a_containerfile() {
        let request = ImageBuild {
            image: "qcode/profile/claude-sub",
            containerfile: Path::new("/build/Containerfile"),
            context: Path::new("/build"),
        };
        let command = podman().build_image(&request);
        assert_eq!(command.program, Path::new("/usr/bin/podman"));
        assert_eq!(
            args(&command),
            ["build", "--tag", "qcode/profile/claude-sub", "--file", "/build/Containerfile", "/build"]
        );
        assert_eq!(args(&docker().build_image(&request)), args(&command));
    }

    #[test]
    fn builds_an_image_again_without_reusing_a_layer() {
        let request = ImageBuild {
            image: "qcode/profile/anti",
            containerfile: Path::new("/build/Containerfile"),
            context: Path::new("/build"),
        };
        let spelled =
            ["build", "--no-cache", "--tag", "qcode/profile/anti", "--file", "/build/Containerfile", "/build"];
        assert_eq!(args(&podman().rebuild_image(&request)), spelled);
        assert_eq!(args(&docker().rebuild_image(&request)), spelled);
    }

    #[test]
    fn removes_an_image_only_when_nothing_is_made_from_it() {
        assert_eq!(args(&podman().remove_unused_image("0123abcd")), ["image", "rm", "0123abcd"]);
    }

    #[test]
    fn asks_an_image_for_a_label_and_a_container_for_its_image() {
        assert_eq!(
            args(&podman().image_label("qcode/profile/anti", "qcode.profile.revision")),
            [
                "image",
                "inspect",
                "--format",
                "{{index .Config.Labels \"qcode.profile.revision\"}}",
                "qcode/profile/anti"
            ]
        );
        assert_eq!(
            args(&docker().container_image("qcode-w-anti")),
            ["container", "inspect", "--format", "{{.Image}}", "qcode-w-anti"]
        );
    }

    #[test]
    fn removes_an_image() {
        assert_eq!(args(&podman().remove_image("qcode/base")), ["image", "rm", "--force", "qcode/base"]);
    }

    #[test]
    fn asks_whether_an_image_is_there() {
        assert_eq!(
            args(&docker().image_exists("qcode/base")),
            ["image", "inspect", "--format", "{{.Id}}", "qcode/base"]
        );
    }

    #[test]
    fn asks_how_large_an_image_is_the_same_way_on_both_engines() {
        let spelled = ["image", "inspect", "--format", "{{.Size}}", "qcode/profile/anti"];
        assert_eq!(args(&docker().image_size("qcode/profile/anti")), spelled);
        assert_eq!(args(&podman().image_size("qcode/profile/anti")), spelled);
    }

    #[test]
    fn podman_maps_the_user_with_keep_id_and_relabels_mounts() {
        let mounts = [
            Mount {
                source: MountSource::Path(Path::new("/home/me/QCode/Workspaces/p/Work")),
                target: Path::new("/work"),
                access: Access::ReadWrite,
            },
            Mount {
                source: MountSource::Volume("qcode-home-p-claude"),
                target: Path::new("/home/qcode"),
                access: Access::ReadOnly,
            },
        ];
        let request = ContainerCreate {
            name: "qcode-p-claude",
            hostname: "qcode",
            labels: &[],
            image: "qcode/profile/claude",
            mounts: &mounts,
            network: Network::Full,
            user: HostUser::Ids { uid: 1000, gid: 1000 },
            workdir: Some(Path::new("/work")),
            command: &["sleep", "infinity"],
        };
        assert_eq!(
            args(&podman().create_container(&request)),
            [
                "create",
                "--name",
                "qcode-p-claude",
                "--hostname",
                "qcode",
                "--userns=keep-id",
                "--volume",
                "/home/me/QCode/Workspaces/p/Work:/work:rw,z",
                "--volume",
                "qcode-home-p-claude:/home/qcode:ro,z",
                "--workdir",
                "/work",
                "--pull=never",
                "qcode/profile/claude",
                "sleep",
                "infinity"
            ]
        );
    }

    #[test]
    fn docker_names_the_user_ids_and_leaves_mounts_plain() {
        let mounts = [Mount {
            source: MountSource::Volume("qcode-home-p-claude"),
            target: Path::new("/home/qcode"),
            access: Access::ReadWrite,
        }];
        let request = ContainerCreate {
            name: "qcode-p-claude",
            hostname: "qcode",
            labels: &[],
            image: "qcode/profile/claude",
            mounts: &mounts,
            network: Network::None,
            user: HostUser::Ids { uid: 1000, gid: 100 },
            workdir: None,
            command: &[],
        };
        assert_eq!(
            args(&docker().create_container(&request)),
            [
                "create",
                "--name",
                "qcode-p-claude",
                "--hostname",
                "qcode",
                "--user",
                "1000:100",
                "--network=none",
                "--volume",
                "qcode-home-p-claude:/home/qcode:rw",
                "--pull=never",
                "qcode/profile/claude"
            ]
        );
    }

    #[test]
    fn a_host_without_posix_ids_gets_no_user_flag() {
        let request = ContainerCreate {
            name: "qcode-p-base",
            hostname: "box",
            labels: &[],
            image: "qcode/base",
            mounts: &[],
            network: Network::Full,
            user: HostUser::ImageDefault,
            workdir: None,
            command: &[],
        };
        let expected = ["create", "--name", "qcode-p-base", "--hostname", "box", "--pull=never", "qcode/base"];
        assert_eq!(args(&podman().create_container(&request)), expected);
        assert_eq!(args(&docker().create_container(&request)), expected);
    }

    #[test]
    fn a_one_off_container_removes_itself_and_maps_the_user_like_a_lasting_one() {
        let mounts = [
            Mount {
                source: MountSource::Path(Path::new("/home/me/QCode/Workspaces/p/Work")),
                target: Path::new("/work"),
                access: Access::ReadOnly,
            },
            Mount {
                source: MountSource::Path(Path::new("/home/me/QCode/Workspaces/p/Backup")),
                target: Path::new("/backup"),
                access: Access::ReadWrite,
            },
        ];
        let request = RunOnce {
            image: "qcode/base",
            mounts: &mounts,
            network: Network::None,
            user: HostUser::Ids { uid: 1000, gid: 100 },
            workdir: Some(Path::new("/work")),
            command: &["sh", "-c", "git status"],
        };
        assert_eq!(
            args(&podman().run_once(&request)),
            [
                "run",
                "--rm",
                "--userns=keep-id",
                "--network=none",
                "--volume",
                "/home/me/QCode/Workspaces/p/Work:/work:ro,z",
                "--volume",
                "/home/me/QCode/Workspaces/p/Backup:/backup:rw,z",
                "--workdir",
                "/work",
                "--pull=never",
                "qcode/base",
                "sh",
                "-c",
                "git status"
            ]
        );
        assert_eq!(
            args(&docker().run_once(&request)),
            [
                "run",
                "--rm",
                "--user",
                "1000:100",
                "--network=none",
                "--volume",
                "/home/me/QCode/Workspaces/p/Work:/work:ro",
                "--volume",
                "/home/me/QCode/Workspaces/p/Backup:/backup:rw",
                "--workdir",
                "/work",
                "--pull=never",
                "qcode/base",
                "sh",
                "-c",
                "git status"
            ]
        );
    }

    #[test]
    fn podman_opens_a_window_with_the_options_the_trial_measured() {
        let command = window(&podman());
        assert_eq!(
            command[..6],
            ["run", "--detach", "--init", "--shm-size=1g", "--name", "qcode-p-anti.desk"],
            "detached because nobody watches it, and init so the stop signal reaches the program"
        );
        // Podman needs no profile of ours: its default already lets the application build its own
        // sandbox, so it is never handed one even when one is offered.
        assert!(!command.contains(&"--security-opt".to_owned()), "{command:?}");
        // The runtime folder is the container's own and belongs to whoever runs inside.
        assert!(window_at(&command, "--tmpfs") == Some("/run/qcode-display:rw,mode=0700,U".to_owned()), "{command:?}");
        assert_eq!(window_at(&command, "--device").as_deref(), Some("/dev/dri"));
        assert_eq!(window_at(&command, "--env").as_deref(), Some("XDG_RUNTIME_DIR=/run/qcode-display"));
        // One socket file of the machine's runtime folder, never the folder: the engine's own
        // socket lives beside it, and a container that had it could start anything on the machine.
        assert!(
            command.contains(&"/run/user/1000/wayland-1:/run/qcode-display/wayland-1:rw".to_owned()),
            "{command:?}"
        );
        assert!(!command.iter().any(|arg| arg == "/run/user/1000:/run/qcode-display:rw"), "{command:?}");
        assert_eq!(&command[command.len() - 3..], ["/opt/app/app", "--ozone-platform=wayland", "/work"]);
        // Never the flag that would give up the application's own sandbox.
        assert!(!command.contains(&"--no-sandbox".to_owned()), "{command:?}");
    }

    #[test]
    fn docker_opens_a_window_with_the_ids_named_and_the_profile_its_default_lacks() {
        let command = window(&docker());
        assert_eq!(command[..6], ["run", "--detach", "--init", "--shm-size=1g", "--name", "qcode-p-anti.desk"]);
        assert_eq!(command[6..8], ["--security-opt", "seccomp=/tmp/qcode-seccomp.json"]);
        assert_eq!(
            window_at(&command, "--tmpfs").as_deref(),
            Some("/run/qcode-display:rw,mode=0700,uid=1000,gid=1000"),
            "docker takes no `U`, so the ids are named as they are everywhere else on it"
        );
        assert!(command.contains(&"--user".to_owned()) && command.contains(&"1000:1000".to_owned()), "{command:?}");
    }

    #[test]
    fn a_window_on_a_machine_without_a_graphics_card_or_a_profile_asks_for_neither() {
        let mounts = [];
        let request = RunWindow {
            name: "qcode-p-anti.desk",
            once: RunOnce {
                image: "qcode/profile/anti",
                mounts: &mounts,
                network: Network::Full,
                user: HostUser::Ids { uid: 1000, gid: 1000 },
                workdir: None,
                command: &["/opt/app/app"],
            },
            env: &[],
            sockets: &[],
            tmpfs: &[],
            devices: &[],
            shm: "1g",
            seccomp: None,
        };
        for engine in [podman(), docker()] {
            let command = args(&engine.run_window(&request));
            assert!(!command.contains(&"--device".to_owned()), "{command:?}");
            assert!(!command.contains(&"--security-opt".to_owned()), "{command:?}");
        }
    }

    #[test]
    fn a_window_is_waited_for_and_given_time_to_close_before_it_is_killed() {
        for engine in [podman(), docker()] {
            assert_eq!(args(&engine.wait_container("qcode-p-anti.desk")), ["wait", "qcode-p-anti.desk"]);
            assert_eq!(
                args(&engine.stop_container_within("qcode-p-anti.desk", 10)),
                ["stop", "--time", "10", "qcode-p-anti.desk"]
            );
        }
    }

    /// The words of the command that opens a window through `engine`, with everything a window can
    /// be given: the machine's compositor socket, its graphics device and a seccomp profile.
    fn window(engine: &Engine) -> Vec<String> {
        let mounts = [Mount {
            source: MountSource::Path(Path::new("/home/me/QCode/Workspaces/p/Work")),
            target: Path::new("/work"),
            access: Access::ReadWrite,
        }];
        let sockets =
            [Socket { host: Path::new("/run/user/1000/wayland-1"), target: Path::new("/run/qcode-display/wayland-1") }];
        let tmpfs = [Tmpfs { target: Path::new("/run/qcode-display"), mode: "0700" }];
        let devices = [Path::new("/dev/dri")];
        args(&engine.run_window(&RunWindow {
            name: "qcode-p-anti.desk",
            once: RunOnce {
                image: "qcode/profile/anti",
                mounts: &mounts,
                network: Network::Full,
                user: HostUser::Ids { uid: 1000, gid: 1000 },
                workdir: Some(Path::new("/work")),
                command: &["/opt/app/app", "--ozone-platform=wayland", "/work"],
            },
            env: &[("XDG_RUNTIME_DIR", "/run/qcode-display"), ("WAYLAND_DISPLAY", "wayland-1")],
            sockets: &sockets,
            tmpfs: &tmpfs,
            devices: &devices,
            shm: "1g",
            seccomp: Some(Path::new("/tmp/qcode-seccomp.json")),
        }))
    }

    /// The word after the first `flag` of `command`.
    fn window_at(command: &[String], flag: &str) -> Option<String> {
        let at = command.iter().position(|arg| arg == flag)?;
        command.get(at + 1).cloned()
    }

    #[test]
    fn starts_stops_and_removes_a_container() {
        let engine = podman();
        assert_eq!(args(&engine.start_container("qcode-p-base")), ["start", "qcode-p-base"]);
        assert_eq!(args(&engine.stop_container("qcode-p-base")), ["stop", "qcode-p-base"]);
        assert_eq!(args(&engine.remove_container("qcode-p-base")), ["rm", "--force", "qcode-p-base"]);
    }

    #[test]
    fn execs_interactively_for_a_pseudo_terminal() {
        let request = Exec { container: "qcode-p-claude", command: &["claude", "--dangerously-skip-permissions"] };
        assert_eq!(
            args(&docker().exec(&request)),
            ["exec", "--interactive", "--tty", "qcode-p-claude", "claude", "--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn a_tab_is_told_which_tab_it_is_through_its_environment() {
        let request = Exec { container: "qcode-p-claude", command: &["claude"] };
        assert_eq!(
            args(&podman().exec_with_env(&request, &[("QCODE_BRIDGE", "abc")])),
            ["exec", "--interactive", "--tty", "--env", "QCODE_BRIDGE=abc", "qcode-p-claude", "claude"]
        );
    }

    #[test]
    fn a_command_that_reads_its_input_keeps_it_open_without_a_terminal() {
        let request = Exec { container: "qcode-p-claude", command: &["sh", "-c", "cat > f"] };
        assert_eq!(
            args(&docker().exec_reading(&request)),
            ["exec", "--interactive", "qcode-p-claude", "sh", "-c", "cat > f"]
        );
    }

    #[test]
    fn a_container_is_made_with_its_labels_and_asked_for_one() {
        let request = ContainerCreate {
            name: "qcode-p-base",
            hostname: "box",
            labels: &[("qcode.plan", "0123")],
            image: "qcode/base",
            mounts: &[],
            network: Network::Full,
            user: HostUser::ImageDefault,
            workdir: None,
            command: &[],
        };
        assert_eq!(
            args(&podman().create_container(&request)),
            [
                "create",
                "--name",
                "qcode-p-base",
                "--hostname",
                "box",
                "--label",
                "qcode.plan=0123",
                "--pull=never",
                "qcode/base"
            ]
        );
        assert_eq!(
            args(&docker().container_label("qcode-p-base", "qcode.plan")),
            ["container", "inspect", "--format", "{{index .Config.Labels \"qcode.plan\"}}", "qcode-p-base"]
        );
    }

    #[test]
    fn execs_without_a_terminal_when_the_answer_is_the_exit_code() {
        // Without a real terminal `docker exec --tty` refuses outright, so a command whose
        // exit code is the whole point asks for no terminal at all.
        let request = Exec { container: "qcode-p-base", command: &["sh", "-c", "test -f /work/marker"] };
        assert_eq!(
            args(&docker().exec_without_terminal(&request)),
            ["exec", "qcode-p-base", "sh", "-c", "test -f /work/marker"]
        );
        assert_eq!(args(&podman().exec_without_terminal(&request)), args(&docker().exec_without_terminal(&request)));
    }

    #[test]
    fn asks_for_one_container_state_and_for_every_one() {
        assert_eq!(
            args(&podman().container_state("qcode-p-base")),
            ["container", "inspect", "--format", "{{.State.Status}}", "qcode-p-base"]
        );
        assert_eq!(args(&docker().list_containers()), ["ps", "--all", "--format", "{{.Names}}\t{{.State}}"]);
    }

    #[test]
    fn creates_removes_and_lists_volumes() {
        let engine = docker();
        assert_eq!(args(&engine.create_volume("qcode-cred-claude")), ["volume", "create", "qcode-cred-claude"]);
        assert_eq!(args(&engine.remove_volume("qcode-cred-claude")), ["volume", "rm", "qcode-cred-claude"]);
        assert_eq!(args(&engine.list_volumes()), ["volume", "ls", "--format", "{{.Name}}"]);
    }

    #[test]
    fn copies_in_both_directions() {
        let engine = podman();
        let into = CopyIn { host: Path::new("/tmp/cred/."), container: "qcode-copy", target: Path::new("/mount") };
        assert_eq!(args(&engine.copy_in(&into)), ["cp", "/tmp/cred/.", "qcode-copy:/mount"]);
        let out = CopyOut { container: "qcode-copy", source: Path::new("/mount/."), host: Path::new("/tmp/cred") };
        assert_eq!(args(&engine.copy_out(&out)), ["cp", "qcode-copy:/mount/.", "/tmp/cred"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_current_user_is_the_one_the_shell_reports() {
        use std::process::Command;

        let reported = |flag: &str| {
            let output = Command::new("id").arg(flag).output().expect("`id` runs on every unix");
            String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().expect("`id` prints a number")
        };
        assert_eq!(
            HostUser::current().expect("the current user is readable"),
            HostUser::Ids { uid: reported("-u"), gid: reported("-g") }
        );
    }

    #[test]
    fn a_program_run_attached_gets_a_terminal_a_name_its_variables_and_an_unlabelled_socket() {
        let mounts = [Mount {
            source: MountSource::Path(Path::new("/home/me/QCode/Workspaces/p/Work")),
            target: Path::new("/work"),
            access: Access::ReadOnly,
        }];
        let sockets = [Socket { host: Path::new("/run/user/1000/pulse/native"), target: Path::new("/run/sound") }];
        let request = RunAttached {
            name: "qcode-p.play-3",
            once: RunOnce {
                image: "qcode/base",
                mounts: &mounts,
                network: Network::None,
                user: HostUser::Ids { uid: 1000, gid: 100 },
                workdir: Some(Path::new("/work")),
                command: &["play", "/work/a song.mp3"],
            },
            env: &[("PULSE_SERVER", "unix:/run/sound")],
            sockets: &sockets,
        };
        let head = [
            "run",
            "--rm",
            "--interactive",
            "--tty",
            "--name",
            "qcode-p.play-3",
            "--env",
            "PULSE_SERVER=unix:/run/sound",
        ];
        let tail = ["--workdir", "/work", "--pull=never", "qcode/base", "play", "/work/a song.mp3"];
        let podman = args(&podman().run_attached(&request));
        assert_eq!(podman[..8], head);
        assert_eq!(
            podman[8..15],
            [
                "--volume",
                "/run/user/1000/pulse/native:/run/sound:rw",
                "--userns=keep-id",
                "--network=none",
                "--volume",
                "/home/me/QCode/Workspaces/p/Work:/work:ro,z",
                "--workdir",
            ],
            "the folder is relabelled like every folder, the socket never"
        );
        assert_eq!(podman[14..], tail);
        let docker = args(&docker().run_attached(&request));
        assert_eq!(docker[..8], head);
        assert_eq!(
            docker[8..16],
            [
                "--volume",
                "/run/user/1000/pulse/native:/run/sound:rw",
                "--user",
                "1000:100",
                "--network=none",
                "--volume",
                "/home/me/QCode/Workspaces/p/Work:/work:ro",
                "--workdir",
            ]
        );
        assert_eq!(docker[15..], tail);
    }

    #[test]
    fn no_container_is_made_from_an_image_the_engine_would_go_and_fetch() {
        // Every command that makes a container of an image says `--pull=never` right before the
        // image, on both engines: an image QCode built and this engine lacks is a missing image,
        // never a name to look up on a registry.
        let once = RunOnce {
            image: "qcode/base",
            mounts: &[],
            network: Network::Full,
            user: HostUser::ImageDefault,
            workdir: None,
            command: &["true"],
        };
        let create = ContainerCreate {
            name: "qcode-p-claude",
            hostname: "qcode",
            labels: &[],
            image: "qcode/profile/claude",
            mounts: &[],
            network: Network::Full,
            user: HostUser::Ids { uid: 1000, gid: 1000 },
            workdir: None,
            command: &["true"],
        };
        let attached = RunAttached { name: "qcode-p.play-1", once, env: &[], sockets: &[] };
        let window = RunWindow {
            name: "qcode-p-anti.desk",
            once,
            env: &[],
            sockets: &[],
            tmpfs: &[],
            devices: &[],
            shm: "1g",
            seccomp: None,
        };
        for engine in [podman(), docker()] {
            for (command, image) in [
                (engine.create_container(&create), "qcode/profile/claude"),
                (engine.run_once(&once), "qcode/base"),
                (engine.run_attached(&attached), "qcode/base"),
                (engine.run_window(&window), "qcode/base"),
            ] {
                let spelled = args(&command);
                let at = spelled.iter().position(|arg| arg == image).expect("the image is named");
                assert_eq!(spelled[at - 1], "--pull=never", "{spelled:?}");
            }
        }
    }

    #[test]
    fn a_fed_run_takes_its_input_and_is_made_like_any_other() {
        let once = RunOnce {
            image: "qcode/base",
            mounts: &[],
            network: Network::None,
            user: HostUser::Ids { uid: 1000, gid: 1000 },
            workdir: None,
            command: &["tar", "-xf", "-"],
        };
        assert_eq!(
            args(&docker().run_fed(&once)),
            [
                "run",
                "--rm",
                "--interactive",
                "--user",
                "1000:1000",
                "--network=none",
                "--pull=never",
                "qcode/base",
                "tar",
                "-xf",
                "-"
            ]
        );
    }

    #[test]
    fn qcodes_own_containers_are_listed_by_their_label_and_sized_in_bytes() {
        assert_eq!(
            args(&podman().list_labelled("qcode.plan")),
            ["ps", "--all", "--filter", "label=qcode.plan", "--format", "{{.Names}}"]
        );
        assert_eq!(
            args(&docker().container_size("qcode-p-base")),
            ["container", "inspect", "--size", "--format", "{{.SizeRw}}", "qcode-p-base"]
        );
    }
}
