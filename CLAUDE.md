# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

ssspam is a Discord "say sound" bot written in Rust (edition 2024). Users in a bound text channel type sound names (optionally with modifiers), and the bot plays the matching MP3s into the voice channel it has joined.

Workspace crates:
- `ssspam-bot`: the bot itself (lib + `ssspam-bot` main binary) plus helper binaries `gen_viewer` (generates the static sound-list HTML viewer, see `dist/`) and `stats` (usage stats from a message log).
- `ssspam-proto`: protobuf definitions (`src/ss.proto`) compiled by `prost-build` in `build.rs`. Building requires `protoc` (`protobuf-compiler`).
- `ssspam-util`: `encode_sounds` binary that dumps the sound list as a `ssspam.ss.Sounds` protobuf.
- `scripts/sscompose.py` (not part of the workspace; run with `uv run --script`): turns a MIDI/note-text score into sslang messages that play a song by pitch-shifting one sample. It mirrors the parsing/timing rules of `sslang.rs` and `play.rs`, so keep it in sync when those change.

## Commands

```sh
cargo build
cargo test                                   # what CI runs (stable toolchain)
cargo test -p ssspam-bot sslang::test::test_parse_single_command_with_options   # single test
just lint   # nightly clippy (-D clippy::all -W clippy::nursery) + nightly fmt check
just fix    # apply clippy fixes and format
cargo run --bin ssspam-bot                   # reads config from env / .env
```

Formatting uses nightly rustfmt (`rustfmt.toml`: `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`, `reorder_impl_items`), so always run `cargo +nightly fmt`, not stable fmt. CI enforces both the clippy flags above and the fmt check.

Runtime requirements: `ffmpeg`/`ffprobe` on PATH (used for decoding every sound) and `libopus`. Configuration comes from CLI flags or env vars (loaded via `dotenvy` from `.env`): `DISCORD_TOKEN`, `SOUND_DIR`, `CONFIG_DIR`, `COMMAND_PREFIX` (default `~`), optional `OTLP_ENDPOINT` (also exports the traces by OpenTelemetry; the logs go to stdout either way, filtered by `RUST_LOG`). GCS access (`upload`/`delete`) uses Application Default Credentials, i.e. `GOOGLE_APPLICATION_CREDENTIALS` pointing to a service account key JSON.

Tests use the fixture MP3s in `tests/sound/` (referenced via `CARGO_MANIFEST_DIR/../tests/sound`).

## Architecture

**Shared state via Serenity's TypeMap.** `main.rs` inserts all long-lived state into `client.data`; every handler/command fetches it with `ctx.data.read().await.get::<T>()`. Each type implements `TypeMapKey`:
- `SoundStorage` (`sound.rs`, `Arc<RwLock<_>>`): map of lowercased sound name → `SoundFile`, loaded by globbing `SOUND_DIR/**/*.mp3`. `watch_sound_storage` keeps it in sync with the filesystem via `notify`. MP3 metadata is parsed lazily (`OnceLock`, shared by a `SoundFile`'s clones), and for every sound in the background at startup (`preload_metadata`).
- `ChannelManager` (`core.rs`): per-guild (voice channel, text channel) binding set by `join`; persisted to `CONFIG_DIR/channel_state.json` and used on `cache_ready` to rejoin voice channels after restart.
- `ChannelUserManager`: tracks who is in the bot's voice channel to detect joins/leaves for join/leave sounds.
- `Configs` (`config.rs`): PickleDb JSON store at `CONFIG_DIR/config.json` (global settings and per-user `joinsound`/`leavesound`). Marked in-code as needing a rewrite.
- `SaySoundCache` (`play.rs`): LRU cache of decoded audio keyed by the full `SayCommand` (name + modifiers), up to 64 MB of it, each sound weighed by the size its duration gives.
- `GuildBroadcast`: per-guild `tokio::broadcast` channel; the `stop` command sends `OpsMessage::Stop`, which cancels in-flight playback through a `tokio::select!` in `core::process_message`.

**Message → audio pipeline.**
1. `main.rs` `EventHandler::message` → `core::process_message`: ignores messages unless they are in the bound text channel and the author is in the bot's voice channel.
2. `sslang.rs` parses the message with `nom` into `SayCommands`. A parse failure is the normal case for chat and is silently ignored. Syntax: `name [speed] [pN pitch] [wN wait-sec] [sN start-sec] [dN duration-sec] [s|stop] [af=<ffmpeg filter>]`, separated by `;` (overlap; next starts after `wait`) or `|` (concatenate; next starts after this one finishes).
3. `play.rs` decodes each command by shelling out to `ffmpeg` (speed/pitch via `asetrate`/`atempo`, plus any user `af=`), whose output streams into an in-memory songbird `Memory`, and plays the tracks on the songbird `Call` at a fixed low volume. A track starts as soon as ffmpeg has decoded its beginning, not all of it; the commands of a message are decoded a few ahead of the one playing (`DECODE_AHEAD`), by at most four ffmpeg at once (`FFMPEG_SLOTS`). Each track is parsed before it is handed to the mixer (`play_sound`), which then mixes it from its next 20 ms tick instead of parsing it on its thread pool over two or three ticks. `keep_warm` decodes and parses a random sound at startup and once a minute, with nobody listening, so that ffmpeg (a 200 MB static binary) stays in the page cache: after 10-25 minutes without a say sound, the first waited 0.8-5.5 s for ffmpeg to be read from disk again. Total playback is capped at 180s (`MAX_PLAYABLE_DURATION`).
4. For latency, each message logs how long Discord took to deliver it (`delivery_ms`, from the time in its ID) and how long `play_say_commands` then took to hand its first track to the mixer and for its first 20 ms frame to be mixed (`elapsed_ms`).

**Prefix commands** live in `command.rs` as `poise` commands and must also be registered in the `commands` vec in `main.rs`. Owner-only commands (`upload`, `delete`) use the owner IDs hardcoded in `main.rs`. `upload`/`delete` also mirror sound files to the Google Cloud Storage bucket `surfpvparena` (`dist/sound/…`) and regenerate `dist/sounds.bin` (`web.rs`), which backs the external sound-list viewer. `restart` just exits with code 1 and relies on the container being restarted.

`songbird` is crates.io's 0.6.1, with ssspam's changes in `patches/songbird.patch` (its own clipping of the mixed audio, at its `Config::clip_threshold`, 0.01 by default, and a fix to the RTP timestamp of a parked mixer). Only the monorepo's Bazel build applies the patch; cargo cannot, so a cargo build, including the public repository's releases, has upstream songbird.

## Where this is developed

ssspam is developed in a private monorepo, as its `projects/ssspam/`, and exported from there to the public repository github.com/reiyw/ssspam by Copybara: one commit there per monorepo commit, and commits made in the public repository directly are overwritten by the next export. The export leaves out the monorepo's files: the Bazel `BUILD.bazel` files and the deployment (`k8s/`, `skaffold.yaml`).

In the monorepo, Bazel builds the same workspace from the same `Cargo.lock` (its `@ssspam_crates`), so keep the cargo build working too:

```sh
bazel build //projects/ssspam/...
bazel test //projects/ssspam/...
```

- A crate's dependencies come from its `Cargo.toml` (`all_crate_deps`); after changing them, update `Cargo.lock` with cargo and the BUILD files only when targets change (a new binary, a new crate).
- `patches/songbird.patch` is applied to songbird by a `crate.annotation` in `MODULE.bazel`. To change it, edit the unpacked crate (`songbird-0.6.1.crate` from crates.io) and regenerate the patch with `diff -ruN` or `git diff` (`-p1`, paths from the crate root); after a songbird bump, check it still applies.
- libopus is Bazel's (the monorepo's `MODULE.bazel` annotates `libopus_sys`), not the system's; `ssspam-proto/build.rs` runs with Bazel's `protoc`.
- The BUILD targets are tagged `norustfmt`: this directory's `rustfmt.toml` (nightly `cargo fmt`) formats them, not the monorepo's.

## Deployment

The monorepo builds the image (`//projects/ssspam/ssspam-bot:ssspam-bot_image`: the bot linked against the glibc of its Debian 13 base, plus static ffmpeg and ffprobe) and deploys it to its k3s cluster with `skaffold run`. Tags `v*` create a GitHub release with the `ssspam-bot` Linux binary.
