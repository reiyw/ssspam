# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

ssspam is a Discord "say sound" bot written in Rust (edition 2024). Users in a bound text channel type sound names (optionally with modifiers), and the bot plays the matching MP3s into the voice channel it has joined.

Workspace crates:
- `ssspam-bot`: the bot itself (lib + `ssspam-bot` main binary) plus helper binaries `gen_viewer` (generates the static sound-list HTML viewer, see `dist/`) and `stats` (usage stats from a message log).
- `ssspam-proto`: protobuf definitions (`src/ss.proto`) compiled by `prost-build` in `build.rs`. Building requires `protoc` (`protobuf-compiler`).
- `ssspam-util`: `encode_sounds` binary that dumps the sound list as a `ssspam.ss.Sounds` protobuf.

## Commands

```sh
cargo build
cargo test                                   # what CI runs (stable toolchain)
cargo test -p ssspam-bot sslang::test::test_parse_single_command_with_options   # single test
just lint   # nightly clippy (-D clippy::all -W clippy::nursery) + nightly fmt check + hadolint
just fix    # apply clippy fixes and format
cargo run --bin ssspam-bot                   # reads config from env / .env
```

Formatting uses nightly rustfmt (`rustfmt.toml`: `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`, `reorder_impl_items`), so always run `cargo +nightly fmt`, not stable fmt. CI enforces both the clippy flags above and the fmt check.

Runtime requirements: `ffmpeg`/`ffprobe` on PATH (used for decoding every sound) and `libopus`. Configuration comes from CLI flags or env vars (loaded via `dotenvy` from `.env`): `DISCORD_TOKEN`, `SOUND_DIR`, `CONFIG_DIR`, `COMMAND_PREFIX` (default `~`), optional `OTLP_ENDPOINT` (enables OpenTelemetry tracing; without it no tracing subscriber is installed at all).

Tests use the fixture MP3s in `tests/sound/` (referenced via `CARGO_MANIFEST_DIR/../tests/sound`).

## Architecture

**Shared state via Serenity's TypeMap.** `main.rs` inserts all long-lived state into `client.data`; every handler/command fetches it with `ctx.data.read().await.get::<T>()`. Each type implements `TypeMapKey`:
- `SoundStorage` (`sound.rs`, `Arc<RwLock<_>>`): map of lowercased sound name → `SoundFile`, loaded by globbing `SOUND_DIR/**/*.mp3`. `watch_sound_storage` keeps it in sync with the filesystem via `notify`. MP3 metadata is parsed lazily (`OnceLock`).
- `ChannelManager` (`core.rs`): per-guild (voice channel, text channel) binding set by `join`; persisted to `CONFIG_DIR/channel_state.json` and used on `cache_ready` to rejoin voice channels after restart.
- `ChannelUserManager`: tracks who is in the bot's voice channel to detect joins/leaves for join/leave sounds.
- `Configs` (`config.rs`): PickleDb JSON store at `CONFIG_DIR/config.json` (global settings and per-user `joinsound`/`leavesound`). Marked in-code as needing a rewrite.
- `SaySoundCache` (`play.rs`): LRU cache of decoded audio keyed by the full `SayCommand` (name + modifiers).
- `GuildBroadcast`: per-guild `tokio::broadcast` channel; the `stop` command sends `OpsMessage::Stop`, which cancels in-flight playback through a `tokio::select!` in `core::process_message`.

**Message → audio pipeline.**
1. `main.rs` `EventHandler::message` → `core::process_message`: ignores messages unless they are in the bound text channel and the author is in the bot's voice channel.
2. `sslang.rs` parses the message with `nom` into `SayCommands`. A parse failure is the normal case for chat and is silently ignored. Syntax: `name [speed] [pN pitch] [wN wait-sec] [sN start-sec] [dN duration-sec] [s|stop] [af=<ffmpeg filter>]`, separated by `;` (overlap; next starts after `wait`) or `|` (concatenate; next starts after this one finishes).
3. `play.rs` decodes each command by shelling out to `ffmpeg` (speed/pitch via `asetrate`/`atempo`, plus any user `af=`) into an in-memory songbird `Memory`, then plays the tracks on the songbird `Call` at a fixed low volume. Total playback is capped at 180s (`MAX_PLAYABLE_DURATION`).

**Prefix commands** live in `command.rs` as `poise` commands and must also be registered in the `commands` vec in `main.rs`. Owner-only commands (`upload`, `delete`) use the owner IDs hardcoded in `main.rs`. `upload`/`delete` also mirror sound files to the Google Cloud Storage bucket `surfpvparena` (`dist/sound/…`) and regenerate `dist/sounds.bin` (`web.rs`), which backs the external sound-list viewer. `restart` just exits with code 1 and relies on the container being restarted.

`songbird` is pulled from a fork (`github.com/reiyw/songbird`, branch `current`), not crates.io.

## Deployment

Pushes to `main` build and push the Docker image `reiyw/ssspam:latest` (multi-stage `cargo-chef` build; the runtime image bundles a static ffmpeg). Tags `v*` create a GitHub release with the `ssspam-bot` Linux binary.
