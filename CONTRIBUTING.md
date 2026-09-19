# Contributing to qcode

Thank you for taking the time. Bug reports, ideas and pull requests are all welcome.

## Reporting a bug or asking for something

Open an issue at <https://github.com/quvyta/code/issues> and pick the template that fits. For a
bug, the engine and its version, whether it runs rootless, the harness and your distribution
usually decide where the problem is, so the template asks for them. Please check that a
screenshot or log shows nothing private (a login, an e-mail address, a project you cannot share)
before you attach it.

## Building and testing

The toolchain is pinned by `rust-toolchain.toml`; `rustup` picks it up by itself.

```sh
git clone https://github.com/quvyta/code
cd code
git config core.hooksPath .githooks   # once: formatting, clippy, tests and docs before every commit
cargo run --bin qcode
cargo test
```

`cargo test` needs no container engine. The tests that build images and run containers are
ignored by default; with Podman or Docker running, run them with:

```sh
QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
```

They create images, containers and volumes whose names start with `qcode`, and remove what they
create.

## Pull requests

- Keep one change per pull request, and say in the description what it changes for the person
  using qcode.
- The commit hook must pass: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`
  and `cargo doc` without warnings. Please do not skip it.
- A bug fix comes with a test that fails without it.
- Text the person reads lives in `assets/locales/en.toml` and `assets/locales/tr.toml`, never in
  the code. If you do not speak Turkish, add the English line and say so in the pull request.
- qcode never runs a harness, a build or a clone on the machine itself: whatever runs, runs in a
  container. A change that would break that promise will not be merged.
- The interface comes from [quvyta-framework](https://github.com/quvyta/framework). A widget or
  behaviour every Quvyta application would need belongs there; open an issue in that repository
  first.

## Licence

By contributing you agree that your contribution is licensed under the MIT licence of this
repository.
