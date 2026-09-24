//! The operating systems a profile's image can be built on: Debian, which every image was built on
//! before the choice existed and which stays the default, Arch, Ubuntu LTS, and Alpine.
//!
//! Each system has a base image of its own, described by a Containerfile of its own, and every
//! one of them ends with the same user, the same variables and the same directories, because
//! everything above an image — the mounts, [`super::paths`], a profile's recipe — reads those and
//! nothing else. What differs is the package manager, the names of the packages, and what a
//! system cannot carry or run; that is written down here, measured rather than assumed, so the
//! wizard can say it before anything is built.
//!
//! Only a profile's own containers start from the image of its system. A workspace's shell, the
//! built-in apps that open its files, a sound being played, a backup, and every other helper
//! container keep starting from the Debian image, [`crate::engine::names::BASE_IMAGE`], whatever
//! the profiles of the workspace run on: a workspace holds profiles of several systems at once,
//! and a file has to open the same way in all of them.

use crate::profile::HarnessKind;

/// A system a profile's image is built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Os {
    /// Debian 13, from the Node project's own Debian image. The default, and what a profile file
    /// that names no system is.
    #[default]
    Debian,
    /// Arch Linux, rolling.
    Arch,
    /// Ubuntu 24.04 LTS.
    Ubuntu,
    /// Alpine, on musl rather than glibc; offered, and said to be not recommended.
    Alpine,
}

/// Why a harness is not offered on a system: what was measured when it was tried there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The harness installs, and the terminal library it opens every shell command through is a
    /// glibc build that takes the whole program down on musl. Gemini CLI on Alpine: its
    /// `@lydell/node-pty-linux-x64` has no musl build, and loading it into Alpine's Node ended the
    /// process with a segmentation fault (measured 2026-09-22 with 0.60.0). Qwen Code, Gemini CLI's
    /// fork, carries the same library: on Alpine it started, and the first shell command typed into
    /// it ended the program (measured 2026-09-23 with 0.24.4; loading the library alone faulted).
    TerminalLibrary,
    /// The application is a glibc program and the system has no glibc loader, so it does not
    /// start at all. Antigravity IDE on Alpine: its program asks for `/lib64/ld-linux-x86-64.so.2`,
    /// which Alpine does not have, and the shell answers `not found` (measured 2026-09-22 with
    /// 2.5.5).
    GlibcProgram,
    /// The application opens a window, and that window has been measured on Debian only: the
    /// packages it needs, the image's size, and that the window really comes up. On another
    /// system none of that has been seen, so QCode does not build it there.
    WindowOnDebianOnly,
}

/// A program the built-in apps or the image's tools use that a system's image does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// Alpine packages no docx2txt, so a Word document cannot be read as text inside the image.
    Docx2txt,
    /// Arch's sox depends on ffmpeg, which is more than half of the image on its own; the image
    /// carries no sox, so there is no `play` or `soxi` inside it.
    Sox,
    /// Ubuntu 24.04 packages no opus format for sox, so sox inside the image reads no `.opus`.
    SoxOpus,
}

impl Gap {
    /// The programs of [`super::apps::PROGRAMS`] this gap takes away, by the name each is looked
    /// up by, so the live test asks an image for exactly what it promises and no more.
    #[must_use]
    pub fn programs(self) -> &'static [&'static str] {
        match self {
            Self::Docx2txt => &["docx2txt"],
            Self::Sox => &["play", "soxi"],
            Self::SoxOpus => &[],
        }
    }
}

impl Os {
    /// Every system, in the order the wizard offers them: the default first, the one said to be
    /// not recommended last.
    pub const ALL: [Self; 4] = [Self::Debian, Self::Arch, Self::Ubuntu, Self::Alpine];

    /// How the system is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Debian => "debian",
            Self::Arch => "arch",
            Self::Ubuntu => "ubuntu",
            Self::Alpine => "alpine",
        }
    }

    /// The system written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|os| os.id() == id)
    }

    /// The system's own name and release, which is the same in every language.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Debian => "Debian 13",
            Self::Arch => "Arch Linux",
            Self::Ubuntu => "Ubuntu 24.04 LTS",
            Self::Alpine => "Alpine 3.24",
        }
    }

    /// Whether QCode recommends the system. Alpine is offered for other work and said not to be:
    /// of the five harnesses, two do not run on it at all (see [`Os::refuses`]).
    #[must_use]
    pub fn recommended(self) -> bool {
        self != Self::Alpine
    }

    /// The name of the system's base image. Debian's is the name every image had before there
    /// was a choice, so the images already on a machine stay the ones it finds.
    #[must_use]
    pub fn image(self) -> &'static str {
        match self {
            Self::Debian => crate::engine::names::BASE_IMAGE,
            Self::Arch => "qcode/base-arch",
            Self::Ubuntu => "qcode/base-ubuntu",
            Self::Alpine => "qcode/base-alpine",
        }
    }

    /// The Containerfile the system's base image is built from, carried inside the binary.
    #[must_use]
    pub fn containerfile(self) -> &'static str {
        match self {
            Self::Debian => super::CONTAINERFILE,
            Self::Arch => include_str!("../../assets/containerfiles/base-arch.Containerfile"),
            Self::Ubuntu => include_str!("../../assets/containerfiles/base-ubuntu.Containerfile"),
            Self::Alpine => include_str!("../../assets/containerfiles/base-alpine.Containerfile"),
        }
    }

    /// How large the system's base image is, in megabytes as podman reports them, so the wizard
    /// can say what is being chosen before it is built.
    ///
    /// Measured on 2026-09-22 with podman on images built from the Containerfiles as they are
    /// now; a change to a Containerfile is a reason to measure again (`podman image inspect
    /// --format '{{.Size}}'`, and the live test in `live.rs` prints it).
    #[must_use]
    pub fn image_mb(self) -> u64 {
        match self {
            Self::Debian => 517,
            Self::Arch => 809,
            Self::Ubuntu => 508,
            Self::Alpine => 312,
        }
    }

    /// What the system's image does not carry that the Debian image does.
    #[must_use]
    pub fn gaps(self) -> &'static [Gap] {
        match self {
            Self::Debian => &[],
            Self::Arch => &[Gap::Sox],
            Self::Ubuntu => &[Gap::SoxOpus],
            Self::Alpine => &[Gap::Docx2txt],
        }
    }

    /// Why `harness` is not offered on this system, or `None` where it is.
    ///
    /// Each answer was measured on the system's own base image, by installing the harness the
    /// way its recipe does and running it; the variants of [`Refusal`] say what was seen. Where a
    /// harness is offered here, it was seen to install and start there: on Alpine, Claude Code
    /// and opencode install their own musl builds and Codex's program is a static musl build; Kimi
    /// Code CLI is JavaScript and runs its shell commands without a terminal library, and a command
    /// typed into it ran on all four systems (measured 2026-09-23 with 2.1.0), as one typed into
    /// Qwen Code did on Debian, Arch and Ubuntu.
    #[must_use]
    pub fn refuses(self, harness: HarnessKind) -> Option<Refusal> {
        match (self, harness) {
            (Self::Alpine, HarnessKind::GeminiCli | HarnessKind::QwenCode) => Some(Refusal::TerminalLibrary),
            (Self::Alpine, HarnessKind::AntigravityIde) => Some(Refusal::GlibcProgram),
            (Self::Arch | Self::Ubuntu, HarnessKind::AntigravityIde) => Some(Refusal::WindowOnDebianOnly),
            _ => None,
        }
    }

    /// The shell command that installs `packages` from the system's own repositories and leaves
    /// no package lists or downloads behind in the layer. It runs as root.
    ///
    /// Arch installs with `-Syu`: it supports no partial upgrade, so installing from a freshly
    /// read package list means upgrading to it.
    #[must_use]
    pub fn install(self, packages: &[&str]) -> String {
        let packages = packages.join(" ");
        match self {
            Self::Debian | Self::Ubuntu => format!(
                "apt-get update \\\n && apt-get install --yes --no-install-recommends {packages} \\\n \
                 && rm -rf /var/lib/apt/lists/*"
            ),
            Self::Arch => format!(
                "pacman -Syu --noconfirm --needed {packages} \\\n \
                 && rm -rf /var/cache/pacman/pkg/* /var/lib/pacman/sync/*"
            ),
            Self::Alpine => format!("apk add --no-cache {packages}"),
        }
    }

    /// The packages graphify needs from the system: Python 3.10 or later, and pipx to install it
    /// with, under each system's own names.
    #[must_use]
    pub fn python(self) -> &'static [&'static str] {
        match self {
            Self::Debian | Self::Ubuntu | Self::Alpine => &["python3", "pipx"],
            Self::Arch => &["python", "python-pipx"],
        }
    }

    /// How pipx is told to install `package` outside the home directory, into `home`, with its
    /// commands in `/usr/local/bin`.
    ///
    /// Ubuntu 24.04 packages pipx 1.4.3, which has no `--global` (it came with 1.5, and 1.4.3
    /// answers `unrecognized arguments: --global`), so there the location is given the older way,
    /// through `PIPX_HOME` and `PIPX_BIN_DIR`; it lands in the same places.
    #[must_use]
    pub fn pipx(self, home: &str, package: &str) -> String {
        match self {
            Self::Ubuntu => format!("PIPX_HOME={home} PIPX_BIN_DIR=/usr/local/bin pipx install {package}"),
            Self::Debian | Self::Arch | Self::Alpine => {
                format!("PIPX_GLOBAL_HOME={home} PIPX_GLOBAL_BIN_DIR=/usr/local/bin pipx install --global {package}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Gap, Os, Refusal};
    use crate::base::apps::PROGRAMS;
    use crate::engine::names::BASE_IMAGE;
    use crate::profile::HarnessKind;

    #[test]
    fn every_system_reads_back_as_itself_and_debian_is_the_default() {
        for os in Os::ALL {
            assert_eq!(Os::parse(os.id()), Some(os));
        }
        assert_eq!(Os::default(), Os::Debian);
        assert_eq!(Os::ALL[0], Os::Debian, "the default is offered first");
        assert_eq!(Os::parse("gentoo"), None);
    }

    #[test]
    fn debian_keeps_the_image_name_every_image_had_before_the_choice() {
        // A machine that already has `qcode/base` must find it under the same name, or every
        // profile made before this release would build a second copy of it.
        assert_eq!(Os::Debian.image(), BASE_IMAGE);
        assert_eq!(Os::Debian.image(), "qcode/base");
        assert_eq!(Os::Debian.containerfile(), crate::base::CONTAINERFILE);
        let names: Vec<&str> = Os::ALL.map(Os::image).to_vec();
        for (index, name) in names.iter().enumerate() {
            assert!(name.starts_with("qcode/base") && name.chars().all(|c| c.is_ascii_lowercase() || "/-".contains(c)));
            assert!(!names[..index].contains(name), "{name} is the image of two systems");
        }
    }

    #[test]
    fn every_system_starts_from_its_own_distribution_and_brings_node() {
        let from = |os: Os| {
            os.containerfile().lines().filter(|line| line.starts_with("FROM ")).map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(from(Os::Debian), ["FROM docker.io/library/node:24-trixie-slim"]);
        assert_eq!(from(Os::Arch), ["FROM docker.io/library/archlinux:latest"]);
        assert_eq!(from(Os::Ubuntu), ["FROM docker.io/library/ubuntu:24.04"]);
        assert_eq!(from(Os::Alpine), ["FROM docker.io/library/node:24-alpine"]);
        // Ubuntu's own Node is 18 and cannot run Claude Code; the Node project's build is copied
        // in from the same image the Debian base starts from.
        assert!(
            Os::Ubuntu
                .containerfile()
                .contains("COPY --from=docker.io/library/node:24-trixie-slim /usr/local/bin/node")
        );
        assert!(!Os::Ubuntu.containerfile().contains(" nodejs"), "Ubuntu's own Node is never installed");
        // Arch's own npm 12 runs no install script it was not told to, and Claude Code puts its
        // program in place in one; the npm bundled with Node 24 is taken instead, as on Ubuntu.
        assert!(Os::Arch.containerfile().contains("--needed nodejs-lts-krypton ca-certificates"));
        assert!(
            Os::Arch
                .containerfile()
                .contains("COPY --from=docker.io/library/node:24-trixie-slim /usr/local/lib/node_modules/npm ")
        );
    }

    #[test]
    fn every_system_ends_with_the_same_user_variables_and_directories() {
        // Everything above the base image reads these and nothing else, so a system whose image
        // drifted from Debian's here would lose a login or a mount without a word.
        let tail = |text: &'static str| -> Vec<&'static str> {
            text.lines()
                .filter(|line| !line.starts_with('#'))
                .filter(|line| {
                    line.starts_with("ENV ")
                        || line.starts_with("WORKDIR ")
                        || line.starts_with("USER ")
                        || line.contains("useradd ")
                        || line.contains("qcode-open-home")
                        || line.contains("mkdir -p /usr/local/npm")
                        || line.contains("chmod 0777")
                })
                .map(str::trim)
                .collect()
        };
        let debian = tail(Os::Debian.containerfile());
        assert!(debian.len() >= 12, "{debian:?}");
        for os in Os::ALL {
            assert_eq!(tail(os.containerfile()), debian, "{os:?}");
        }
    }

    #[test]
    fn a_system_installs_with_its_own_package_manager() {
        assert!(Os::Debian.install(&["python3"]).starts_with("apt-get update"));
        assert!(Os::Ubuntu.install(&["python3"]).starts_with("apt-get update"));
        assert!(Os::Arch.install(&["python"]).starts_with("pacman -Syu --noconfirm --needed python"));
        assert_eq!(Os::Alpine.install(&["python3", "pipx"]), "apk add --no-cache python3 pipx");
        for os in Os::ALL {
            assert!(os.containerfile().contains(os.install(&[]).split(' ').next().unwrap_or_default()), "{os:?}");
        }
    }

    #[test]
    fn ubuntus_pipx_is_told_where_to_install_without_the_option_it_does_not_have() {
        assert!(!Os::Ubuntu.pipx("/opt/pipx", "graphifyy").contains("--global"));
        assert!(Os::Ubuntu.pipx("/opt/pipx", "graphifyy").contains("PIPX_HOME=/opt/pipx PIPX_BIN_DIR=/usr/local/bin"));
        for os in [Os::Debian, Os::Arch, Os::Alpine] {
            assert!(os.pipx("/opt/pipx", "graphifyy").contains("pipx install --global graphifyy"), "{os:?}");
        }
    }

    #[test]
    fn what_a_system_lacks_is_named_and_leaves_its_program_out_of_the_image() {
        for os in Os::ALL {
            for gap in os.gaps() {
                for program in gap.programs() {
                    assert!(
                        PROGRAMS.iter().any(|command| command.iter().any(|word| word.contains(program))),
                        "{program} is a program the built-in apps know"
                    );
                }
            }
        }
        assert!(!Os::Alpine.containerfile().contains("docx2txt odt2txt"), "Alpine packages no docx2txt");
        assert!(!Os::Arch.containerfile().lines().any(|line| line.starts_with("    ") && line.contains(" sox")));
        assert_eq!(Os::Debian.gaps(), []);
        assert_eq!(Os::Alpine.gaps(), [Gap::Docx2txt]);
    }

    #[test]
    fn alpine_refuses_the_three_harnesses_that_do_not_run_on_musl_and_nothing_else() {
        let refused: Vec<HarnessKind> =
            HarnessKind::ALL.into_iter().filter(|harness| Os::Alpine.refuses(*harness).is_some()).collect();
        assert_eq!(refused, [HarnessKind::GeminiCli, HarnessKind::QwenCode, HarnessKind::AntigravityIde]);
        assert_eq!(Os::Alpine.refuses(HarnessKind::GeminiCli), Some(Refusal::TerminalLibrary));
        assert_eq!(Os::Alpine.refuses(HarnessKind::QwenCode), Some(Refusal::TerminalLibrary));
        assert_eq!(Os::Alpine.refuses(HarnessKind::KimiCode), None);
        assert!(!Os::Alpine.recommended());
        for os in [Os::Debian, Os::Arch, Os::Ubuntu] {
            assert!(os.recommended(), "{os:?}");
            for harness in HarnessKind::TERMINAL {
                assert_eq!(os.refuses(harness), None, "{os:?} {harness:?}");
            }
        }
        for harness in HarnessKind::ALL {
            assert_eq!(Os::Debian.refuses(harness), None, "Debian runs everything: {harness:?}");
        }
    }
}
