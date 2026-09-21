# The Alpine image a profile can be built on instead of the Debian one (`base.Containerfile`).
# QCode lists it as not recommended, and this is why: Alpine is built on musl instead of glibc,
# and what the harnesses install was measured on it one by one (the note of 2026-09-22 has the
# results). Where a harness does not run here, QCode does not offer it with this image.
#
# It carries what the Debian image carries as far as Alpine packages it and ends with the same
# user, the same variables and the same directories, because everything above an image reads those
# and nothing else. Only a profile's own containers start from it: a workspace's shell, the
# built-in apps and every helper container keep starting from the Debian image.
#
# The Node project's own Alpine image, the same Node 24 the Debian image starts from, built
# for musl.
FROM docker.io/library/node:24-alpine

# bash, because the image's user logs in with it everywhere else and Alpine's own shell is
# BusyBox's; shadow, for the same useradd and userdel the other images use.
#
# The built-in apps, the same programs as on Debian for the same reasons (the Debian file says why
# each is there), under Alpine's names, with two gaps: Alpine packages no docx2txt, so a Word file
# cannot be read as text inside this image, and its sox reads mp3 and opus but plays through ALSA
# or libao rather than PulseAudio. `--no-cache` keeps the package index out of the image, as
# dropping the lists does on Debian.
RUN apk add --no-cache bash shadow ca-certificates git \
    nano vim chafa unzip zip xz bzip2 7zip poppler-utils odt2txt sox

# The user the image belongs to. The Node image keeps a user called node at uid 1000, so that one
# goes first, as in the Debian image.
RUN if id node > /dev/null 2>&1; then userdel --remove node; fi \
 && groupadd --gid 1000 qcode \
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
