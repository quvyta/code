# The Arch Linux image a profile can be built on instead of the Debian one (`base.Containerfile`).
#
# It carries what the Debian image carries, in Arch's own packages, and ends with the same user,
# the same variables and the same directories, because everything above an image — the mounts, the
# path contract in `base::paths`, the recipes of the profile images — reads those and nothing else.
# Only a profile's own containers start from it: a workspace's shell, the built-in apps and every
# helper container keep starting from the Debian image, so choosing Arch changes where the harness
# runs and nothing else.
#
# Arch's rolling image, because Arch is a rolling distribution: a pinned snapshot would be a
# system nobody runs. Rebuilding the image is how it takes Arch's updates, as it is for Debian.
FROM docker.io/library/archlinux:latest

# Node from Arch's own repository, the LTS line (krypton is Node 24) rather than `nodejs`, which
# follows the newest release: the harnesses test against LTS, and it is the same Node 24 the Debian
# image carries.
#
# The built-in apps, the same programs as on Debian for the same reasons (the Debian file says why
# each is there), under Arch's names: xz and bzip2 are the tools themselves, 7zip is the 7-Zip
# project's own 7z, poppler carries pdftotext and pdftoppm, and docx2txt and odt2txt are in
# Arch's extra repository.
#
# sox is not here. Arch's sox depends on ffmpeg, and ffmpeg brings Mesa, LLVM, SDL and Python with
# it: measured on 2026-09-22, the image was 1284 MB with sox and 815 MB without, so sox alone would
# have been more than half of everything this file adds. A sound opened from the file tree does not
# play in a profile's container anyway: it plays in a container of the Debian image made for it, so
# what goes missing is only a `play` for a program running inside a profile of this system.
#
# `-Syu` rather than `-Sy`: Arch supports no partial upgrade, so installing from a freshly read
# package list means upgrading to it. The package cache and the lists are dropped afterwards; a
# profile image that installs more reads the lists again, the way Debian's `apt-get update` does.
RUN pacman -Syu --noconfirm --needed nodejs-lts-krypton ca-certificates git \
    nano vim chafa unzip zip xz bzip2 7zip poppler docx2txt odt2txt \
 && rm -rf /var/cache/pacman/pkg/* /var/lib/pacman/sync/*

# npm is not Arch's. Arch packages npm 12, which no longer runs a package's install scripts unless
# each package is allowed by name, and Claude Code and opencode put their programs in place in
# such a script: built with Arch's npm, Claude Code answered "Error: claude native binary not
# installed" (measured 2026-09-22). The npm every harness here was checked with is the one the Node
# project bundles with Node 24, so that one is copied in from the image the Debian base starts from,
# with the same links the Node image makes. It is plain JavaScript and runs on Arch's Node.
COPY --from=docker.io/library/node:24-trixie-slim /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# The user the image belongs to, as in the Debian image. Arch's image has no user at uid 1000, so
# there is nothing to take out of the way first.
RUN groupadd --gid 1000 qcode \
 && useradd --uid 1000 --gid 1000 --shell /bin/bash --home-dir /home/qcode --create-home qcode

# From here on, word for word what the Debian image does, for the reasons it gives there.
ENV HOME=/home/qcode
ENV LANG=C.UTF-8
ENV NPM_CONFIG_PREFIX=/usr/local/npm
ENV NPM_CONFIG_CACHE=/var/cache/npm
ENV NPM_CONFIG_UPDATE_NOTIFIER=false
ENV PATH=/usr/local/npm/bin:$PATH
RUN mkdir -p /usr/local/npm/bin /usr/local/npm/lib /var/cache/npm /work /assets \
 && chmod 0777 /usr/local/npm /usr/local/npm/bin /usr/local/npm/lib /var/cache/npm \
               /home/qcode /work /assets
RUN printf '%s\n' '#!/bin/sh' 'set -e' 'chmod -R a+rwX "$HOME"' > /usr/local/bin/qcode-open-home \
 && chmod 0755 /usr/local/bin/qcode-open-home
WORKDIR /work
USER qcode
