# The image every QCode container starts from: a project's plain shell runs in it, a git clone
# runs in it, and every profile image is built on top of it.
#
# Nothing is here that a harness does not need. Each block says why it is here.

# Debian trixie with the current Node LTS, from the Node project's own image.
#
# Node, because all four harnesses are installed with `npm install -g` (see `profile/harness.rs`)
# and the highest version any of them asks for is Node 22 (@anthropic-ai/claude-code, "engines":
# {"node": ">=22.0.0"}); @google/gemini-cli asks for >=20 and @openai/codex for >=16. Node 24 is
# the LTS line, so it clears all four and keeps clearing them for years.
#
# Debian rather than Alpine, because glibc is what the native parts of those packages are built
# for: @google/gemini-cli pulls @lydell/node-pty-linux-x64, whose prebuilt binary declares no
# musl variant, and @openai/codex ships one linux-x64 binary with no musl build beside it. Only
# opencode publishes musl builds. Alpine would be the smaller image and the wrong one.
#
# The slim variant, because the full one carries a build toolchain no harness uses. The image
# comes to about 360 MB, of which Node is the larger half; it is pulled once per machine and
# every profile image shares it. The tag is a major version, not a digest, so rebuilding the
# image is how a machine takes Debian's and Node's security updates.
FROM docker.io/library/node:24-trixie-slim

# git, because a project can be made by cloning and because harnesses read and write history
# themselves; ca-certificates, because every install and every sign-in goes over TLS. Both are
# missing from the slim image. The package lists are dropped again: they are only useful to an
# `apt-get install` that will never run here.
RUN apt-get update \
 && apt-get install --yes --no-install-recommends ca-certificates git \
 && rm -rf /var/lib/apt/lists/*

# The user the image belongs to.
#
# On Linux the container is run as the person's own user (podman `--userns=keep-id`, docker
# `--user uid:gid`), so this entry is not who runs; it is who owns what the build leaves behind,
# and a name and a home for the very common case of uid 1000. Tools that look their own user up
# — Node's `os.userInfo()` throws without an entry — then find one. The Node image already keeps
# a user at uid 1000, so that one goes first rather than QCode's landing at 1001 and leaving the
# common case unnamed.
RUN if id node > /dev/null 2>&1; then userdel --remove node; fi \
 && groupadd --gid 1000 qcode \
 && useradd --uid 1000 --gid 1000 --shell /bin/bash --home-dir /home/qcode --create-home qcode

# The home directory is named in the image itself, because docker takes HOME from the password
# file of the user it is given and falls back to `/` for a uid it does not know; a harness would
# then write its login into the root of the container and lose it on the next start.
ENV HOME=/home/qcode
# A UTF-8 default, so a harness printing Turkish through the terminal is not at the mercy of an
# unset locale. C.UTF-8 needs no locale package.
ENV LANG=C.UTF-8
# The global npm prefix is QCode's own directory rather than /usr/local, so that making it
# writable by whoever builds a profile image does not make the distribution's own bin directory
# writable too.
ENV NPM_CONFIG_PREFIX=/usr/local/npm
# The npm cache lives outside the home directory. Everything under the home directory is copied
# into a project's home volume the first time a container starts; a cache does not belong there,
# and a cache owned by the build would be unwritable to the user the container runs as.
ENV NPM_CONFIG_CACHE=/var/cache/npm
# A harness runs unattended; an update notice interleaved with its output helps no one.
ENV NPM_CONFIG_UPDATE_NOTIFIER=false
ENV PATH=/usr/local/npm/bin:$PATH

# The directories of the path contract (`base::paths`), and the two that profile images and
# harnesses write into.
#
# They are all open to everyone on purpose. The container runs as whichever uid the host user
# has, which the image cannot know, so anything an image step leaves behind has to be writable
# by a uid the image never saw. /work/Project and /work/Assets are covered by the mounts over
# them, and are made here so that a container started without mounts still has them.
RUN mkdir -p /usr/local/npm/bin /usr/local/npm/lib /var/cache/npm /work/Project /work/Assets \
 && chmod 0777 /usr/local/npm /usr/local/npm/bin /usr/local/npm/lib /var/cache/npm \
               /home/qcode /work /work/Project /work/Assets

# The one step an image built on this one has to end with.
#
# A Containerfile that adds a harness also writes that harness's configuration into the home
# directory, and the directories it makes on the way belong to the user that built them, with
# the usual permissions. The container is then run as the host user's uid, which on a machine
# where that is not 1000 cannot write into them: the harness starts, signs in, and cannot store
# the login next to the settings file. Running this afterwards opens what the build left.
RUN printf '%s\n' '#!/bin/sh' 'set -e' 'chmod -R a+rwX "$HOME"' > /usr/local/bin/qcode-open-home \
 && chmod 0755 /usr/local/bin/qcode-open-home

# Where a shell starts when no working directory is given.
WORKDIR /work/Project

# Not root, so a harness image is built and run with no more than it needs. It also means a
# harness is installed with `npm install -g` as this user, which is why the npm prefix above is
# open to everyone, and that an image built on this one cannot install system packages.
USER qcode
