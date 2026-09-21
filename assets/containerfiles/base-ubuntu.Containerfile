# The Ubuntu LTS image a profile can be built on instead of the Debian one (`base.Containerfile`).
#
# It carries what the Debian image carries and ends with the same user, the same variables and the
# same directories, because everything above an image reads those and nothing else. Only a
# profile's own containers start from it: a workspace's shell, the built-in apps and every helper
# container keep starting from the Debian image.
#
# Ubuntu 24.04, the LTS release, by its version rather than `latest`: `latest` moves to the next
# LTS on its own, and a person who chose Ubuntu chose the release they know.
FROM docker.io/library/ubuntu:24.04

# Node, copied from the Node project's own Debian image, the very one the Debian base starts from.
#
# Ubuntu 24.04 packages Node 18, and Claude Code needs 22 or later, so Ubuntu's own Node cannot run
# the harnesses. The other ways to a current Node each add something to trust or keep up: the
# NodeSource repository is a third party's apt source and signing key inside the image, and the
# release tarball needs its checksum written here and raised by hand with every Node release. The
# image copy needs neither. It is the Node project's own build (their image unpacks their release
# after checking its signature), it is built for glibc 2.28 and later and Ubuntu 24.04 has 2.39, and
# because it is the same tag the Debian image uses, both systems run the same Node and a rebuild
# takes Node's updates the same way. Only Node and its bundled npm and corepack are taken; the npm
# and npx commands are the same links the Node image makes.
COPY --from=docker.io/library/node:24-trixie-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=docker.io/library/node:24-trixie-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
 && ln -s ../lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack

# git and certificates, and the built-in apps: the same packages as on Debian, for the reasons the
# Debian file gives, except one. Ubuntu 24.04 has no libsox-fmt-opus; its sox reads no opus, and
# the other formats are there as on Debian.
RUN apt-get update \
 && apt-get install --yes --no-install-recommends ca-certificates git \
    nano vim chafa unzip zip xz-utils bzip2 7zip poppler-utils docx2txt odt2txt \
    sox libsox-fmt-mp3 libsox-fmt-pulse \
 && rm -rf /var/lib/apt/lists/*

# The user the image belongs to. Ubuntu's image keeps a user called ubuntu at uid 1000, so that one
# goes first, the way the Debian image removes the Node image's.
RUN if id ubuntu > /dev/null 2>&1; then userdel --remove ubuntu; fi \
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
