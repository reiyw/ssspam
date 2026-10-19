#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=1.26", "mido>=1.3"]
# ///
"""Play a song on ssspam by pitch-shifting a single short sample.

Subcommands:
  prepare    Trim and normalize an audio clip into a sample to upload.
  analyze    Estimate the fundamental frequency of a sample.
  midi-info  List the tracks and channels of a MIDI file.
  gen        Convert a MIDI file or a note text into say-sound messages.

The generator mirrors the bot's semantics (sslang.rs / play.rs):
  - pitch p is clamped to 1..=200 and the playback rate is speed * pitch / 10000.
  - wait / duration are parsed as `(secs * 1000.0) as u32` milliseconds.
  - `;` blocks for `wait`; `|` blocks for `min(len, d) * 100 / speed`.
"""

from __future__ import annotations

import argparse
import bisect
import json
import math
import re
import subprocess
import sys
import wave
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

import numpy as np

SR = 48000
BOT_MAX_MS = 180_000  # MAX_PLAYABLE_DURATION in play.rs
PITCH_MIN, PITCH_MAX = 1, 200  # SayCommands::sanitize
NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
DRUM_CHANNEL = 9


# ---------------------------------------------------------------------------
# Audio helpers


def probe(path: Path) -> tuple[int, int, float]:
    out = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=sample_rate,channels:format=duration",
            "-of", "json", str(path),
        ],
        capture_output=True, check=True, text=True,
    ).stdout
    j = json.loads(out)
    s = j["streams"][0]
    return int(s["sample_rate"]), int(s["channels"]), float(j["format"]["duration"])


def decode_mono(path: Path, af: str | None = None, extra_in: list[str] | None = None) -> np.ndarray:
    cmd = ["ffmpeg", "-v", "error", *(extra_in or []), "-i", str(path), "-f", "f32le", "-ac", "1", "-ar", str(SR)]
    if af:
        cmd += ["-af", af]
    cmd.append("-")
    out = subprocess.run(cmd, capture_output=True, check=True).stdout
    return np.frombuffer(out, dtype=np.float32)


def write_wav(path: Path, x: np.ndarray) -> None:
    pcm = (np.clip(x, -1, 1) * 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


def midi_to_name(m: float) -> str:
    n = round(m)
    cents = (m - n) * 100
    return f"{NOTE_NAMES[n % 12]}{n // 12 - 1}{cents:+.0f}c"


def hz_to_midi(f: float) -> float:
    return 69 + 12 * math.log2(f / 440)


# ---------------------------------------------------------------------------
# Pitch analysis (YIN)


def yin(x: np.ndarray, fmin: float, fmax: float, threshold: float = 0.15) -> tuple[float, float]:
    """Returns (f0, aperiodicity) for one frame of length `win + tau_max`."""
    tau_max = int(SR / fmin) + 1
    tau_min = max(2, int(SR / fmax))
    w = len(x) - tau_max
    x = x.astype(np.float64)
    r = np.correlate(x, x[:w], mode="valid")[:tau_max]
    sq = np.concatenate([[0.0], np.cumsum(x * x)])
    taus = np.arange(tau_max)
    d = sq[w] + (sq[taus + w] - sq[taus]) - 2 * r
    d[0] = 0
    cs = np.cumsum(d[1:])
    cmnd = np.ones(tau_max)
    cmnd[1:] = d[1:] * np.arange(1, tau_max) / np.where(cs > 0, cs, 1)

    below = np.nonzero(cmnd[tau_min:] < threshold)[0]
    if len(below):
        tau = tau_min + int(below[0])
        while tau + 1 < tau_max and cmnd[tau + 1] < cmnd[tau]:
            tau += 1
    else:
        tau = tau_min + int(np.argmin(cmnd[tau_min:]))
    shift = 0.0
    if 1 <= tau < tau_max - 1:
        a, b, c = cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]
        den = a - 2 * b + c
        if den != 0:
            shift = 0.5 * (a - c) / den
    return SR / (tau + shift), float(cmnd[tau])


@dataclass
class Analysis:
    f0: float
    onset_s: float
    spread_cents: tuple[float, float]
    voiced_ratio: float


def analyze_samples(x: np.ndarray, fmin: float = 50, fmax: float = 2000) -> Analysis:
    peak = float(np.max(np.abs(x))) if len(x) else 0.0
    if peak == 0:
        raise SystemExit("error: the sample is silent")
    onset = int(np.argmax(np.abs(x) >= peak * 10 ** (-30 / 20)))

    win, hop = 2048, SR // 100
    flen = win + int(SR / fmin) + 1
    frames = []
    for start in range(onset, len(x) - flen, hop):
        fr = x[start : start + flen]
        rms = float(np.sqrt(np.mean(fr[:win] ** 2)))
        f0, aper = yin(fr, fmin, fmax)
        frames.append((rms, f0, aper))
    if not frames:
        raise SystemExit("error: the sample is too short to analyze")

    rms_max = max(f[0] for f in frames)
    voiced = [(rms, f0) for rms, f0, aper in frames if rms >= rms_max * 0.1 and aper < 0.2]
    if not voiced:
        raise SystemExit("error: no pitched frames found; pass --f0 explicitly")
    w = np.array([v[0] for v in voiced])
    f = np.array([v[1] for v in voiced])
    order = np.argsort(f)
    cw = np.cumsum(w[order])
    f0 = float(f[order][np.searchsorted(cw, cw[-1] / 2)])  # RMS-weighted median
    cents = 1200 * np.log2(f / f0)
    return Analysis(
        f0=f0,
        onset_s=onset / SR,
        spread_cents=(float(np.percentile(cents, 10)), float(np.percentile(cents, 90))),
        voiced_ratio=len(voiced) / len(frames),
    )


def print_analysis(path: Path, a: Analysis) -> None:
    sr, ch, dur = probe(path)
    print(f"file      : {path} ({sr} Hz, {ch} ch, {dur:.3f} s)", file=sys.stderr)
    print(f"f0        : {a.f0:.2f} Hz ({midi_to_name(hz_to_midi(a.f0))})", file=sys.stderr)
    print(f"onset     : {a.onset_s * 1000:.0f} ms", file=sys.stderr)
    lo, hi = a.spread_cents
    print(f"stability : {lo:+.0f} .. {hi:+.0f} cents (p10..p90), voiced {a.voiced_ratio:.0%}", file=sys.stderr)
    if a.onset_s > 0.02:
        print("warning   : leading silence delays every note; run `prepare` to trim it", file=sys.stderr)
    if hi - lo > 40:
        print("warning   : the pitch is unstable (glide or vibrato); melodies may sound off", file=sys.stderr)
    if dur > 3:
        print("warning   : long samples decode slower and may overlap; consider --length in `prepare`", file=sys.stderr)


# ---------------------------------------------------------------------------
# Score input


@dataclass
class Note:
    start: float  # seconds
    dur: float  # seconds
    pitch: float  # MIDI note number
    track: int = 0
    channel: int = 0


def load_midi(path: Path) -> list[Note]:
    import mido

    mid = mido.MidiFile(path)
    tpb = mid.ticks_per_beat
    tempo_events = []
    for track in mid.tracks:
        tick = 0
        for msg in track:
            tick += msg.time
            if msg.type == "set_tempo":
                tempo_events.append((tick, msg.tempo))
    tempo_events.sort()
    ticks, secs, tempos = [0], [0.0], [500_000]
    for tick, tempo in tempo_events:
        secs.append(secs[-1] + (tick - ticks[-1]) * tempos[-1] / 1e6 / tpb)
        ticks.append(tick)
        tempos.append(tempo)

    def t2s(tick: int) -> float:
        i = bisect.bisect_right(ticks, tick) - 1
        return secs[i] + (tick - ticks[i]) * tempos[i] / 1e6 / tpb

    notes = []
    for ti, track in enumerate(mid.tracks):
        tick = 0
        active: dict[tuple[int, int], list[int]] = {}
        for msg in track:
            tick += msg.time
            if msg.type == "note_on" and msg.velocity > 0:
                active.setdefault((msg.channel, msg.note), []).append(tick)
            elif msg.type in ("note_off", "note_on"):
                starts = active.get((msg.channel, msg.note))
                if starts:
                    s = starts.pop(0)
                    notes.append(Note(t2s(s), t2s(tick) - t2s(s), msg.note, ti, msg.channel))
    notes.sort(key=lambda n: (n.start, n.pitch))
    return notes


NOTE_RE = re.compile(r"^([A-Ga-g])([#sb]*)(-?\d+)?$")


def parse_pitch(tok: str) -> float:
    if re.fullmatch(r"\d+(\.\d+)?", tok):
        return float(tok)
    m = NOTE_RE.match(tok)
    if not m:
        raise SystemExit(f"error: invalid note {tok!r}")
    base = NOTE_NAMES.index(m.group(1).upper())
    acc = m.group(2).count("#") + m.group(2).count("s") - m.group(2).count("b")
    octave = int(m.group(3)) if m.group(3) is not None else 4
    return (octave + 1) * 12 + base + acc


def load_text(text: str, bpm: float) -> list[Note]:
    """Tokens: `C4:1`, `F#3:1/2`, `C4+E4+G4:2` (chord), `r:1` (rest), `t=140` (tempo).

    The length is in beats and defaults to the previous one.
    """
    notes = []
    t = 0.0
    beats = Fraction(1)
    for line in text.splitlines():
        for tok in line.split("#", 1)[0].split():
            if tok.startswith("t="):
                bpm = float(tok[2:])
                continue
            head, _, length = tok.partition(":")
            if length:
                beats = Fraction(length)
            sec = float(beats) * 60 / bpm
            if head.lower() not in ("r", "_"):
                for p in head.split("+"):
                    notes.append(Note(t, sec, parse_pitch(p)))
            t += sec
    return notes


def midi_info(notes: list[Note], path: Path) -> None:
    import mido

    mid = mido.MidiFile(path)
    names = {}
    for ti, track in enumerate(mid.tracks):
        names[ti] = track.name
    groups: dict[tuple[int, int], list[Note]] = {}
    for n in notes:
        groups.setdefault((n.track, n.channel), []).append(n)
    print("track ch  notes  range        time            maxpoly  name")
    for (ti, ch), ns in sorted(groups.items()):
        lo, hi = min(n.pitch for n in ns), max(n.pitch for n in ns)
        events = sorted([(n.start, 1) for n in ns] + [(n.start + n.dur, -1) for n in ns], key=lambda e: (e[0], e[1]))
        cur = mx = 0
        for _, d in events:
            cur += d
            mx = max(mx, cur)
        drum = " (drums)" if ch == DRUM_CHANNEL else ""
        print(
            f"{ti:5d} {ch:2d} {len(ns):6d}  {midi_to_name(lo)[:-3]:>4}-{midi_to_name(hi)[:-3]:<4}  "
            f"{ns[0].start:6.1f}-{max(n.start + n.dur for n in ns):6.1f}s  {mx:7d}  {names.get(ti, '')}{drum}"
        )


# ---------------------------------------------------------------------------
# Note selection


def group_onsets(notes: list[Note], eps: float = 0.015) -> list[list[Note]]:
    groups: list[list[Note]] = []
    for n in sorted(notes, key=lambda n: (n.start, -n.pitch)):
        if groups and n.start - groups[-1][0].start <= eps:
            groups[-1].append(n)
        else:
            groups.append([n])
    return groups


def skyline(notes: list[Note]) -> list[Note]:
    """Keeps the highest sounding note at each onset to extract a melody line."""
    kept: list[Note] = []
    for g in group_onsets(notes):
        top = max(g, key=lambda n: n.pitch)
        if kept:
            prev = kept[-1]
            if prev.start + prev.dur > top.start + 1e-6 and prev.pitch > top.pitch:
                continue
            prev.dur = min(prev.dur, top.start - prev.start)
        kept.append(Note(top.start, top.dur, top.pitch, top.track, top.channel))
    return kept


def limit_polyphony(notes: list[Note], n: int) -> list[Note]:
    return [x for g in group_onsets(notes) for x in sorted(g, key=lambda x: -x.pitch)[:n]]


# ---------------------------------------------------------------------------
# Command generation


def fmt_sec(ms: int) -> str:
    """Shortest decimal string `s` such that `(s * 1000.0) as u32 == ms`."""
    for digits, offset in ((3, 0.0), (4, 0.5)):
        s = f"{(ms + offset) / 1000:.{digits}f}".rstrip("0").rstrip(".")
        if s.startswith("0."):
            s = s[1:]
        if int(float(s) * 1000) == ms:
            return s
    raise AssertionError(ms)


def bot_ms(secs_str: str) -> int:
    return int(float(secs_str) * 1000)


@dataclass(frozen=True)
class Rate:
    speed: int
    pitch: int
    cents_err: float


def choose_rate(ratio: float, tol: float, smin: int, smax: int) -> Rate:
    """Picks (speed, pitch) whose product approximates `ratio`, preferring speed 100."""
    best: tuple | None = None
    for s in [100, *sorted(range(smin, smax + 1), key=lambda s: abs(s - 100))]:
        p = round(ratio * 10000 / s)
        if not PITCH_MIN <= p <= PITCH_MAX:
            continue
        err = 1200 * math.log2(s * p / 10000 / ratio)
        key = (abs(err) > tol, abs(s - 100) if abs(err) <= tol else abs(err))
        if best is None or key < best[0]:
            best = (key, Rate(s, p, err))
        if s == 100 and abs(err) <= tol:
            break
    if best is None:
        raise SystemExit(f"error: pitch ratio {ratio:.3f} is out of reach; widen --speed-max or transpose")
    return best[1]


@dataclass
class Cmd:
    rate: Rate
    t_ms: int  # target onset, absolute
    d_ms: int | None  # `d` value in source milliseconds, None to let the sample ring out
    play_ms: int  # how long the bot thinks this command plays

    def render(self, name: str, wait: str | None) -> str:
        opts = ""
        if self.rate.pitch != 100:
            opts += f"p{self.rate.pitch}"
        if wait:
            opts += f"w{wait}"
        if self.d_ms is not None:
            opts += f"d{fmt_sec(self.d_ms)}"
        if self.rate.speed != 100:
            return f"{name}@{self.rate.speed}{opts}"
        return f"{name} {opts}" if opts else name


def bot_play_ms(sample_ms: int, speed: int, d_ms: int | None) -> int:
    dur = sample_ms if d_ms is None else min(sample_ms, d_ms)
    return min(int(dur * (100.0 / speed)), BOT_MAX_MS)


def build_cmds(notes: list[Note], args, sample_ms: int, m0: float) -> list[list[Cmd]]:
    grid = args.grid
    q = lambda sec: int(round(sec * 1000 / grid)) * grid  # noqa: E731
    groups = group_onsets(notes)
    onsets = [q(g[0].start) for g in groups]
    out: list[list[Cmd]] = []
    for gi, g in enumerate(groups):
        t = onsets[gi]
        next_t = onsets[gi + 1] if gi + 1 < len(groups) else None
        cmds = []
        for n in g:
            end = q(n.start + n.dur)
            if args.legato and next_t is not None:
                end = next_t
            dur = max(end - t, grid)
            rate = choose_rate(2 ** ((n.pitch - m0) / 12), args.tolerance, args.speed_min, args.speed_max)
            # Choose `d` so that the bot plays exactly `dur` ms.
            d_ms = None
            want = round(dur * rate.speed / 100)
            if want < sample_ms - args.ring_margin:
                d_ms = min(
                    range(max(1, want - 2), want + 3),
                    key=lambda d: (abs(int(d * (100.0 / rate.speed)) - dur), len(fmt_sec(d))),
                )
            cmds.append(Cmd(rate, t, d_ms, bot_play_ms(sample_ms, rate.speed, d_ms)))
        out.append(cmds)
    return out


@dataclass
class Rendered:
    text: str
    events: list[tuple[int, Cmd]]  # (bot time in ms, command)
    end_ms: int


def render_message(groups: list[list[Cmd]], name: str, overhead: float, sample_ms: int) -> Rendered:
    base = groups[0][0].t_ms
    actual = 0.0
    parts: list[str] = []
    events: list[tuple[int, Cmd]] = []
    end_ms = 0
    for gi, g in enumerate(groups):
        last_group = gi + 1 == len(groups)
        gap = None
        if not last_group:
            gap = max(0, round(groups[gi + 1][0].t_ms - base - actual - overhead * len(g)))
        # Put a command that can block with `|` for exactly `gap` at the end of the group.
        concat = None
        if gap is not None:
            for c in g:
                if c.d_ms is not None and c.play_ms == gap:
                    concat = c
                    break
        order = [c for c in g if c is not concat] + ([concat] if concat else [])
        for ci, c in enumerate(order):
            events.append((round(actual), c))
            end_ms = max(end_ms, round(actual) + c.play_ms)
            is_last = ci + 1 == len(order)
            if not is_last:
                parts.append(c.render(name, None) + ";")
                actual += overhead
            elif last_group:
                parts.append(c.render(name, None))
            elif c is concat:
                parts.append(c.render(name, None) + "|")
                actual += c.play_ms + overhead
            else:
                w = fmt_sec(gap) if gap else None
                parts.append(c.render(name, w) + ";")
                actual += (bot_ms(w) if w else 0) + overhead
    return Rendered("".join(parts), events, end_ms)


def split_messages(groups: list[list[Cmd]], args, sample_ms: int) -> list[Rendered]:
    messages = []
    i = 0
    while i < len(groups):
        best = None
        for j in range(i + 1, len(groups) + 1):
            r = render_message(groups[i:j], args.name, args.overhead_ms, sample_ms)
            if len(r.text) > args.max_chars or r.end_ms > BOT_MAX_MS - 1000:
                break
            best = j
        if best is None:
            raise SystemExit("error: a single chord does not fit into one message")
        if best < len(groups):
            # Prefer splitting at a rest in the last 20% of the message.
            lo = i + max(1, int((best - i) * 0.8))
            def rest_after(k: int) -> int:
                end = max(c.t_ms + c.play_ms for c in groups[k - 1])
                return groups[k][0].t_ms - end
            best = max(range(lo, best + 1), key=lambda k: (rest_after(k), k))
        messages.append(render_message(groups[i:best], args.name, args.overhead_ms, sample_ms))
        i = best
    return messages


# ---------------------------------------------------------------------------
# Preview: replay the bot's decode pipeline locally


def rust_f64(x: float) -> str:
    s = repr(x)
    return s[:-2] if s.endswith(".0") else s


class BotDecoder:
    def __init__(self, path: Path):
        self.path = path
        self.sr, _, _ = probe(path)
        self.cache: dict[tuple[int, int, int | None], np.ndarray] = {}

    def decode(self, c: Cmd) -> np.ndarray:
        key = (c.rate.speed, c.rate.pitch, c.d_ms)
        if key not in self.cache:
            sm, pm = c.rate.speed / 100, c.rate.pitch / 100
            af = f"asetrate={rust_f64(self.sr * sm * pm)},atempo={rust_f64(1 / pm)},aresample={self.sr}"
            t = f"{c.d_ms}ms" if c.d_ms is not None else "0"
            self.cache[key] = decode_mono(self.path, af=af, extra_in=["-ss", "0ms", "-t", t])
        return self.cache[key]


def render_preview(messages: list[Rendered], sample: Path, gap_s: float = 1.5) -> np.ndarray:
    dec = BotDecoder(sample)
    chunks = []
    for m in messages:
        buf = np.zeros(int((m.end_ms / 1000 + 0.5) * SR), dtype=np.float32)
        for t_ms, c in m.events:
            x = dec.decode(c)
            s = t_ms * SR // 1000
            n = min(len(x), len(buf) - s)
            buf[s : s + n] += x[:n]
        chunks += [buf, np.zeros(int(gap_s * SR), dtype=np.float32)]
    out = np.concatenate(chunks)
    peak = float(np.max(np.abs(out))) or 1.0
    return out * (0.9 / peak)


# ---------------------------------------------------------------------------
# CLI


def cmd_prepare(args) -> None:
    filters = [
        "silenceremove=start_periods=1:start_threshold=-40dB:start_silence=0.005",
        f"atrim=0:{args.length}",
        f"afade=t=out:st={max(0.0, args.length - args.fade):.3f}:d={args.fade:.3f}",
        "loudnorm=I=-14:TP=-1",
    ]
    subprocess.run(
        [
            "ffmpeg", "-v", "error", "-y", "-ss", str(args.start), "-i", str(args.input),
            "-af", ",".join(filters), "-ac", "1", "-ar", "44100", "-b:a", "192k", str(args.output),
        ],
        check=True,
    )
    print_analysis(args.output, analyze_samples(decode_mono(args.output)))


def cmd_analyze(args) -> None:
    print_analysis(args.sample, analyze_samples(decode_mono(args.sample), args.fmin, args.fmax))


def cmd_midi_info(args) -> None:
    midi_info(load_midi(args.midi), args.midi)


def load_score(args) -> list[Note]:
    path: Path = args.score
    if path.suffix.lower() in (".mid", ".midi"):
        notes = load_midi(path)
        if args.tracks is not None:
            notes = [n for n in notes if n.track in args.tracks]
        if args.channels is not None:
            notes = [n for n in notes if n.channel in args.channels]
        elif not args.drums:
            notes = [n for n in notes if n.channel != DRUM_CHANNEL]
    else:
        notes = load_text(path.read_text(), args.bpm)
    notes = [n for n in notes if n.start >= args.begin and (args.end is None or n.start < args.end)]
    if not notes:
        raise SystemExit("error: no notes selected")
    if args.melody:
        notes = skyline(notes)
    elif args.melody_tracks:
        top = skyline([n for n in notes if n.track in args.melody_tracks])
        notes = sorted(top + [n for n in notes if n.track not in args.melody_tracks], key=lambda n: (n.start, n.pitch))
    elif args.max_poly:
        notes = limit_polyphony(notes, args.max_poly)
    t0 = notes[0].start
    for n in notes:
        n.start = (n.start - t0) / args.tempo
        n.dur /= args.tempo
    return notes


def cmd_gen(args) -> None:
    sr, _, dur = probe(args.sample)
    sample_ms = int(dur * 1000)
    f0 = args.f0 or analyze_samples(decode_mono(args.sample)).f0
    m0 = hz_to_midi(f0)
    notes = load_score(args)

    if args.transpose == "auto":
        med = float(np.median([n.pitch for n in notes]))
        transpose = 12 * round((m0 - med) / 12)
    else:
        transpose = int(args.transpose)
    for n in notes:
        n.pitch += transpose
        if args.floor is not None:
            while n.pitch < parse_pitch(args.floor):
                n.pitch += 12

    groups = build_cmds(notes, args, sample_ms, m0)
    messages = split_messages(groups, args, sample_ms)

    cmds = [c for g in groups for c in g]
    errs = [abs(c.rate.cents_err) for c in cmds]
    uniq = {(c.rate, c.d_ms) for c in cmds}
    lo, hi = min(n.pitch for n in notes), max(n.pitch for n in notes)
    print(f"sample   : f0 {f0:.2f} Hz ({midi_to_name(m0)}), {sample_ms} ms", file=sys.stderr)
    print(f"score    : {len(notes)} notes, {midi_to_name(lo)[:-3]}..{midi_to_name(hi)[:-3]}, transpose {transpose:+d}", file=sys.stderr)
    print(f"tuning   : max error {max(errs):.1f} cents, {sum(c.rate.speed != 100 for c in cmds)} notes use speed", file=sys.stderr)
    print(f"ringing  : {sum(c.d_ms is None for c in cmds)} notes are longer than the sample", file=sys.stderr)
    print(f"messages : {len(messages)} ({', '.join(f'{len(m.text)} chars/{m.end_ms / 1000:.1f}s' for m in messages)})", file=sys.stderr)
    print(f"decodes  : {len(uniq)} unique commands", file=sys.stderr)

    if args.out_dir:
        args.out_dir.mkdir(parents=True, exist_ok=True)
        for i, m in enumerate(messages, 1):
            (args.out_dir / f"{i:02d}.txt").write_text(m.text + "\n")
    else:
        print("\n\n".join(m.text for m in messages))
    if args.preview:
        write_wav(args.preview, render_preview(messages, args.sample))
        print(f"preview  : {args.preview}", file=sys.stderr)


def int_list(s: str) -> list[int]:
    return [int(x) for x in s.split(",")]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(required=True)

    p = sub.add_parser("prepare", help="trim and normalize a clip into a sample")
    p.add_argument("input", type=Path)
    p.add_argument("output", type=Path, help="output .mp3")
    p.add_argument("--start", type=float, default=0.0, help="seek before trimming silence (s)")
    p.add_argument("--length", type=float, default=1.0, help="max sample length (s)")
    p.add_argument("--fade", type=float, default=0.05, help="fade-out length (s)")
    p.set_defaults(func=cmd_prepare)

    p = sub.add_parser("analyze", help="estimate the pitch of a sample")
    p.add_argument("sample", type=Path)
    p.add_argument("--fmin", type=float, default=50)
    p.add_argument("--fmax", type=float, default=2000)
    p.set_defaults(func=cmd_analyze)

    p = sub.add_parser("midi-info", help="list MIDI tracks and channels")
    p.add_argument("midi", type=Path)
    p.set_defaults(func=cmd_midi_info)

    p = sub.add_parser("gen", help="generate say-sound messages")
    p.add_argument("score", type=Path, help=".mid or a note text file")
    p.add_argument("--sample", type=Path, required=True, help="the sample as uploaded to the bot")
    p.add_argument("--name", required=True, help="the sound name of the sample on the bot")
    p.add_argument("--f0", type=float, help="override the detected sample pitch (Hz)")
    p.add_argument("--transpose", default="auto", help="semitones, or 'auto' to shift by octaves")
    p.add_argument("--bpm", type=float, default=120, help="tempo for note text input")
    p.add_argument("--tempo", type=float, default=1.0, help="playback speed factor")
    p.add_argument("--tracks", type=int_list, help="MIDI tracks to use, e.g. 1,2")
    p.add_argument("--channels", type=int_list, help="MIDI channels (0-based) to use")
    p.add_argument("--drums", action="store_true", help="keep MIDI channel 10")
    p.add_argument("--begin", type=float, default=0.0, help="excerpt start in score time (s)")
    p.add_argument("--end", type=float, help="excerpt end in score time (s)")
    p.add_argument("--melody", action="store_true", help="keep only the top line (skyline)")
    p.add_argument("--melody-tracks", type=int_list, help="apply --melody only to these tracks, keep the others")
    p.add_argument("--max-poly", type=int, help="keep at most N highest notes per onset")
    p.add_argument("--floor", help="raise notes below this pitch (after transposing) by octaves, e.g. E3")
    p.add_argument("--legato", action="store_true", help="extend notes to the next onset (enables `|`)")
    p.add_argument("--grid", type=int, default=10, help="time quantization (ms)")
    p.add_argument("--tolerance", type=float, default=8.0, help="pitch error that triggers speed tuning (cents)")
    p.add_argument("--speed-min", type=int, default=50)
    p.add_argument("--speed-max", type=int, default=400)
    p.add_argument("--ring-margin", type=int, default=30, help="omit `d` when within this of the sample end (ms)")
    p.add_argument("--overhead-ms", type=float, default=0.0, help="per-command delay of the bot to compensate")
    p.add_argument("--max-chars", type=int, default=2000)
    p.add_argument("--out-dir", type=Path, help="write each message to NN.txt instead of stdout")
    p.add_argument("--preview", type=Path, help="render a .wav by replaying the bot's decoding")
    p.set_defaults(func=cmd_gen)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
