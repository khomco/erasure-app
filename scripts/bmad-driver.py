#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml>=6", "tzdata"]
# ///
"""Unattended driver for the `bmad-build-auto` skill.

`bmad-build-auto` builds exactly one story per invocation and never advances on
its own. This driver is the caller: it walks `<spec_folder>/stories.yaml` in list
order, dispatches one `claude -p "/bmad-build-auto spec folder ..., story id ..."`
per story, re-reads the story file's frontmatter `status` afterwards, enforces
the caller-only checkpoint fields (`spec_checkpoint`, `done_checkpoint`), and
survives Claude subscription usage limits by sleeping until the window resets
and resuming the same story.

Usage and design notes: docs/agents/autonomous-driver.md.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import errno
import fcntl
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

try:
    import yaml
except ImportError:  # pragma: no cover - uv installs it via the PEP 723 header
    sys.stderr.write("bmad-driver: PyYAML missing; run via `uv run --script scripts/bmad-driver.py`\n")
    raise

try:
    from zoneinfo import ZoneInfo, ZoneInfoNotFoundError
except ImportError:  # pragma: no cover - Python < 3.9
    ZoneInfo = None  # type: ignore
    ZoneInfoNotFoundError = Exception  # type: ignore

UTC = dt.timezone.utc

KNOWN_STATUSES = ("draft", "ready-for-dev", "in-progress", "in-review", "done", "blocked")
MISSING = "missing"  # no story file yet: build-auto has never been dispatched for it

# Driver exit codes (documented in docs/agents/autonomous-driver.md).
EXIT_OK = 0
EXIT_USAGE = 2
EXIT_CHECKPOINT = 10  # paused waiting for a human approval marker
EXIT_BLOCKED = 20  # a story is blocked / stalled / inconsistent
EXIT_PRECONDITION = 30  # dirty tree, missing claude, auth failure, ...
EXIT_LOCKED = 73  # another driver holds the lock
EXIT_SLEEPING = 75  # --no-wait and the driver would have slept (usage limit / budget / quiet hours)


# ---------------------------------------------------------------------------
# Usage-limit detection
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class LimitHit:
    """A usage-limit hit. `reset_at` is None when no reset time could be parsed."""

    reset_at: Optional[dt.datetime]
    raw: str
    source: str  # "rate_limit_event" | "text" | "legacy-epoch"

    def to_json(self) -> Dict[str, Any]:
        return {
            "reset_at": self.reset_at.isoformat() if self.reset_at else None,
            "raw": self.raw[:300],
            "source": self.source,
        }


# A sentence that says a usage/spend limit stopped the request. Kept broad on
# purpose; it is only ever applied to the CLI's own error/result text, never to
# tool output (see scan_run_output), so it cannot be tripped by code that
# merely mentions these phrases.
_LIMIT_PHRASES = [
    r"you(?:'|’)?ve (?:hit|reached) your\b[^\n]*?\b(?:limit|budget)",
    r"you have (?:hit|reached) your\b[^\n]*?\b(?:limit|budget)",
    r"\b(?:5|five)[- ]hour (?:usage )?limit (?:reached|hit|exceeded)",
    r"\b(?:weekly|daily|monthly|session|opus|sonnet|fable|usage|plan|spend) limit (?:reached|hit|exceeded)",
    r"\bclaude(?: ai)? usage limit reached",
    r"\busage limit reached",
    r"\byou(?:'|’)re out of (?:usage credits|extra usage)",
    r"\brate_limit_error\b",
    r"\busage_limit_reached\b",
]
_LIMIT_RE = re.compile("|".join(f"(?:{p})" for p in _LIMIT_PHRASES), re.IGNORECASE)

# "Claude AI usage limit reached|1730000000" — the pre-2025 CLI format.
_LEGACY_EPOCH_RE = re.compile(r"limit reached\s*\|\s*(\d{9,13})", re.IGNORECASE)

# "... · resets 3pm (America/New_York)", "resets at 3:30 PM", "resets in 2h 5m",
# "resets Nov 7, 3pm (Europe/Berlin)", "resets 2026-08-08 00:00 UTC".
_RESETS_RE = re.compile(
    r"\b(?:resets?|reset(?:s)? at|try again|available again)\s*(?:at|on)?\s*(?P<when>[^\n·|—;]+)",
    re.IGNORECASE,
)

_MONTHS = {m: i for i, m in enumerate(
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"], start=1)}

_TZ_ABBREV = {
    "UTC": "UTC", "GMT": "UTC", "Z": "UTC",
    "EST": "America/New_York", "EDT": "America/New_York", "ET": "America/New_York",
    "CST": "America/Chicago", "CDT": "America/Chicago", "CT": "America/Chicago",
    "MST": "America/Denver", "MDT": "America/Denver", "MT": "America/Denver",
    "PST": "America/Los_Angeles", "PDT": "America/Los_Angeles", "PT": "America/Los_Angeles",
}


def _zone(name: str) -> Optional[dt.tzinfo]:
    name = _TZ_ABBREV.get(name.upper(), name) if len(name) <= 4 else name
    if name.upper() == "UTC":
        return UTC
    if ZoneInfo is None:
        return None
    try:
        return ZoneInfo(name)
    except (ZoneInfoNotFoundError, ValueError, KeyError):
        return None


def _epoch_to_dt(value: float) -> dt.datetime:
    if value > 1e12:  # milliseconds
        value = value / 1000.0
    return dt.datetime.fromtimestamp(value, tz=UTC)


def parse_reset_time(text: str, now: dt.datetime, default_tz: Optional[dt.tzinfo] = None) -> Optional[dt.datetime]:
    """Parse the reset clause of a limit message into an aware UTC datetime.

    `now` must be aware. A bare clock time ("3pm") means its next occurrence
    after `now` in the stated zone (or `default_tz`, else the local zone); a
    time that passed less than 10 minutes ago is taken as "just reset" rather
    than tomorrow. A month/day without a year rolls to next year only when it
    lies more than a day in the past.
    """
    if now.tzinfo is None:
        raise ValueError("now must be timezone-aware")
    s = text.strip().strip(".,;:!").strip()
    if s.endswith(")") and s.count(")") > s.count("("):  # "(daily; resets 2026-08-08 00:00 UTC)"
        s = s[:-1].strip()
    if not s:
        return None

    tz: Optional[dt.tzinfo] = None
    m = re.search(r"\(([A-Za-z]+(?:/[A-Za-z0-9_+\-]+)*)\)", s)
    if m:
        tz = _zone(m.group(1))
        s = (s[: m.start()] + s[m.end():]).strip()
    else:
        m = re.search(r"\s(UTC|GMT|Z|[ECMP][SD]?T)$", s)
        if m:
            tz = _zone(m.group(1))
            s = s[: m.start()].strip()
    if tz is None:
        tz = default_tz or now.astimezone().tzinfo or UTC
    s = s.strip(" ,")

    # Relative: "in 2h 15m", "in 45 minutes", "in 3 hours", "in 1d 2h".
    m = re.match(
        r"^in\s+(?:(?P<d>\d+)\s*d(?:ays?)?\s*)?(?:(?P<h>\d+)\s*h(?:ours?|rs?)?\s*)?"
        r"(?:(?P<m>\d+)\s*m(?:in(?:ute)?s?)?\s*)?(?:(?P<s>\d+)\s*s(?:ec(?:ond)?s?)?)?\s*$",
        s, re.IGNORECASE)
    if m and any(m.group(g) for g in ("d", "h", "m", "s")):
        delta = dt.timedelta(days=int(m.group("d") or 0), hours=int(m.group("h") or 0),
                             minutes=int(m.group("m") or 0), seconds=int(m.group("s") or 0))
        return (now + delta).astimezone(UTC)

    # Bare epoch seconds / milliseconds.
    if re.fullmatch(r"\d{9,13}", s):
        return _epoch_to_dt(float(s))

    # ISO-ish: 2026-08-08 00:00[:ss][Z|+hh:mm]
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?\s*(Z|[+-]\d{2}:?\d{2})?$", s)
    if m:
        y, mo, d = int(m.group(1)), int(m.group(2)), int(m.group(3))
        hh, mm, ss = int(m.group(4) or 0), int(m.group(5) or 0), int(m.group(6) or 0)
        zone = tz
        if m.group(7):
            off = m.group(7)
            if off == "Z":
                zone = UTC
            else:
                sign = 1 if off[0] == "+" else -1
                digits = off[1:].replace(":", "")
                zone = dt.timezone(sign * dt.timedelta(hours=int(digits[:2]), minutes=int(digits[2:])))
        return dt.datetime(y, mo, d, hh, mm, ss, tzinfo=zone).astimezone(UTC)

    tod = _parse_time_of_day(s)

    # Month + day [+ year] [+ time]: "Nov 7, 3pm", "Nov 7 at 3:30 PM", "Jan 2, 2027, 3pm".
    m = re.match(r"^(?P<mon>[A-Za-z]{3,9})\.?\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?\b,?\s*(?P<year>\d{4})?", s)
    if m and m.group("mon")[:3].lower() in _MONTHS:
        mon = _MONTHS[m.group("mon")[:3].lower()]
        day = int(m.group("day"))
        local_now = now.astimezone(tz)
        rest = s[m.end():]
        tod = _parse_time_of_day(rest) or (0, 0)
        year = int(m.group("year")) if m.group("year") else local_now.year
        try:
            cand = dt.datetime(year, mon, day, tod[0], tod[1], tzinfo=tz)
        except ValueError:
            return None
        if not m.group("year") and cand < now - dt.timedelta(days=1):
            cand = cand.replace(year=year + 1)
        return cand.astimezone(UTC)

    # "tomorrow at 3pm" / "today at 3pm"
    day_offset = 0
    lowered = s.lower()
    if lowered.startswith("tomorrow"):
        day_offset = 1
    if tod is not None:
        local_now = now.astimezone(tz)
        cand = dt.datetime(local_now.year, local_now.month, local_now.day, tod[0], tod[1], tzinfo=tz)
        cand += dt.timedelta(days=day_offset)
        if day_offset == 0 and cand < now - dt.timedelta(minutes=10):
            cand += dt.timedelta(days=1)
        return cand.astimezone(UTC)
    return None


def _parse_time_of_day(s: str) -> Optional[Tuple[int, int]]:
    m = re.search(r"\b(\d{1,2})(?::(\d{2}))?\s*([ap])\.?\s*m\b\.?", s, re.IGNORECASE)
    if m:
        h, mi = int(m.group(1)), int(m.group(2) or 0)
        if not (1 <= h <= 12 and 0 <= mi < 60):
            return None
        ap = m.group(3).lower()
        if ap == "a":
            h = 0 if h == 12 else h
        else:
            h = 12 if h == 12 else h + 12
        return h, mi
    m = re.search(r"\b([01]?\d|2[0-3]):([0-5]\d)\b", s)
    if m:
        return int(m.group(1)), int(m.group(2))
    if re.fullmatch(r"\s*(?:at\s+)?noon\s*", s, re.IGNORECASE):
        return 12, 0
    if re.fullmatch(r"\s*(?:at\s+)?midnight\s*", s, re.IGNORECASE):
        return 0, 0
    return None


def parse_limit_message(text: str, now: dt.datetime, default_tz: Optional[dt.tzinfo] = None) -> Optional[LimitHit]:
    """Return a LimitHit if `text` reports a usage limit, else None."""
    if not text:
        return None
    m = _LEGACY_EPOCH_RE.search(text)
    if m:
        return LimitHit(_epoch_to_dt(float(m.group(1))), text.strip(), "legacy-epoch")
    m = _LIMIT_RE.search(text)
    if not m:
        return None
    # Look for the reset clause after the limit phrase first, then anywhere.
    tail = text[m.start():]
    reset_at = None
    for candidate in (tail, text):
        for rm in _RESETS_RE.finditer(candidate):
            reset_at = parse_reset_time(rm.group("when"), now, default_tz)
            if reset_at:
                break
        if reset_at:
            break
    line = text[max(0, m.start() - 40): m.end() + 120].strip()
    return LimitHit(reset_at, line, "text")


@dataclasses.dataclass
class RunScan:
    limit: Optional[LimitHit] = None
    auth_error: Optional[str] = None
    result_text: Optional[str] = None
    result_is_error: Optional[bool] = None


_AUTH_RE = re.compile(
    r"not logged in|please run /login|invalid api key|oauth token (?:has )?(?:expired|revoked)|"
    r"credit balance (?:is )?too low|authentication failed",
    re.IGNORECASE,
)


def _content_text(message: Any) -> str:
    if isinstance(message, str):
        return message
    if isinstance(message, dict):
        content = message.get("content")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    parts.append(str(block.get("text", "")))
            return "\n".join(parts)
    return ""


def scan_run_output(lines: Iterable[str], now: dt.datetime, default_tz: Optional[dt.tzinfo] = None,
                    tail_plain_lines: int = 60) -> RunScan:
    """Inspect one `claude -p` run's combined stdout+stderr.

    Structured `--output-format stream-json` events are preferred:
    - the last `rate_limit_event` decides: `rate_limit_info.status == "rejected"`
      with `resetsAt` (epoch seconds) is a limit hit; a later `allowed` clears it
      (the CLI may wait out a limit in-process and carry on);
    - the `result` message's text and assistant messages that carry an
      `error` field are scanned for limit wording.
    Plain (non-JSON) lines — stderr, or `--output-format text` — are scanned
    too, but only the last `tail_plain_lines` of them. Tool results (`type:
    user`) are never scanned, so a story that edits code mentioning these
    phrases cannot trip detection.
    """
    scan = RunScan()
    last_rle: Optional[Dict[str, Any]] = None
    texts: List[str] = []
    plain: List[str] = []
    for raw in lines:
        line = raw.rstrip("\n")
        if not line.strip():
            continue
        obj = None
        if line.lstrip().startswith("{"):
            try:
                obj = json.loads(line)
            except ValueError:
                obj = None
        if not isinstance(obj, dict):
            plain.append(line)
            continue
        typ = obj.get("type")
        if typ == "rate_limit_event":
            info = obj.get("rate_limit_info")
            if isinstance(info, dict):
                last_rle = info
        elif typ == "result":
            res = obj.get("result")
            if isinstance(res, str):
                scan.result_text = res
                texts.append(res)
            scan.result_is_error = bool(obj.get("is_error"))
            for key in ("error", "errors", "api_error", "subtype"):
                if key in obj and obj[key] and key != "subtype":
                    texts.append(json.dumps(obj[key]))
        elif typ == "assistant":
            if obj.get("error") or obj.get("isApiErrorMessage") or obj.get("api_error"):
                texts.append(_content_text(obj.get("message")))
                texts.append(json.dumps({k: obj.get(k) for k in ("error", "api_error", "apiError") if k in obj}))
                info = (obj.get("api_error_params") or obj.get("apiErrorParams") or {})
                if isinstance(info, dict) and isinstance(info.get("rate_limit_info"), dict):
                    last_rle = info["rate_limit_info"]
        elif typ == "system":
            sub = str(obj.get("subtype", ""))
            if "error" in sub or "retry" in sub or "limit" in sub:
                texts.append(json.dumps(obj))
    texts.extend(plain[-tail_plain_lines:])

    if last_rle is not None and str(last_rle.get("status", "")).lower() == "rejected":
        reset = last_rle.get("resetsAt") or last_rle.get("resets_at") or last_rle.get("overageResetsAt")
        reset_at = None
        if isinstance(reset, (int, float)) and reset > 0:
            reset_at = _epoch_to_dt(float(reset))
        elif isinstance(reset, str):
            reset_at = parse_reset_time(reset, now, default_tz)
        scan.limit = LimitHit(reset_at, json.dumps(last_rle)[:300], "rate_limit_event")
    allowed_after = last_rle is not None and str(last_rle.get("status", "")).lower().startswith("allowed")

    if scan.limit is None and not allowed_after:
        for t in texts:
            hit = parse_limit_message(t, now, default_tz)
            if hit:
                scan.limit = hit
                break
    elif scan.limit is not None and scan.limit.reset_at is None:
        for t in texts:
            hit = parse_limit_message(t, now, default_tz)
            if hit and hit.reset_at:
                scan.limit.reset_at = hit.reset_at
                break

    for t in texts:
        am = _AUTH_RE.search(t)
        if am:
            scan.auth_error = t[max(0, am.start() - 40): am.end() + 80].strip()
            break
    return scan


# ---------------------------------------------------------------------------
# Spec folders, stories and status
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class Story:
    id: str
    title: str
    description: str = ""
    spec_checkpoint: bool = False
    done_checkpoint: bool = False
    invoke_dev_with: str = ""


@dataclasses.dataclass
class StoryStatus:
    status: str  # one of KNOWN_STATUSES, MISSING, or "invalid"
    path: Optional[Path] = None
    error: Optional[str] = None


class SpecError(Exception):
    pass


def load_stories(folder: Path) -> List[Story]:
    path = folder / "stories.yaml"
    if not path.is_file():
        raise SpecError(f"{path}: not found")
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as e:
        raise SpecError(f"{path}: YAML parse error: {e}") from e
    if not isinstance(data, list):
        raise SpecError(f"{path}: top level must be a list of story entries")
    stories: List[Story] = []
    seen = set()
    for i, entry in enumerate(data):
        if not isinstance(entry, dict):
            raise SpecError(f"{path}: entry {i} is not a mapping")
        sid = entry.get("id")
        if not isinstance(sid, str) or not re.fullmatch(r"[A-Za-z0-9-]+", sid):
            raise SpecError(f"{path}: entry {i} id {sid!r} must be a quoted string of letters, digits, dashes")
        if sid in seen:
            raise SpecError(f"{path}: duplicate id {sid!r}")
        seen.add(sid)
        if not entry.get("title"):
            raise SpecError(f"{path}: story {sid!r} has no title")
        if "status" in entry:
            raise SpecError(f"{path}: story {sid!r} has a status field (forbidden by the schema)")
        stories.append(Story(
            id=sid,
            title=str(entry["title"]),
            description=str(entry.get("description") or ""),
            spec_checkpoint=bool(entry.get("spec_checkpoint", False)),
            done_checkpoint=bool(entry.get("done_checkpoint", False)),
            invoke_dev_with=str(entry.get("invoke_dev_with") or "").strip(),
        ))
    for a in seen:
        for b in seen:
            if a != b and b.startswith(a + "-"):
                raise SpecError(f"{path}: ids {a!r} and {b!r} are not prefix-free")
    return stories


def read_frontmatter(path: Path) -> Dict[str, Any]:
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---"):
        return {}
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            try:
                data = yaml.safe_load("\n".join(lines[1:i]))
            except yaml.YAMLError:
                return {"__invalid__": True}
            return data if isinstance(data, dict) else {}
    return {"__invalid__": True}


def read_story_status(folder: Path, story_id: str) -> StoryStatus:
    """Status of one story, read from `<folder>/stories/<id>-*.md` frontmatter."""
    stories_dir = folder / "stories"
    matches = sorted(stories_dir.glob(f"{story_id}-*.md")) if stories_dir.is_dir() else []
    if not matches:
        return StoryStatus(MISSING)
    if len(matches) > 1:
        return StoryStatus("invalid", matches[0], f"ambiguous story files: {', '.join(p.name for p in matches)}")
    fm = read_frontmatter(matches[0])
    status = fm.get("status")
    if fm.get("__invalid__") or not isinstance(status, str) or status not in KNOWN_STATUSES:
        return StoryStatus("invalid", matches[0], f"unrecognized status {status!r} in {matches[0].name}")
    return StoryStatus(status, matches[0])


def marker_path(folder: Path, story_id: str, kind: str) -> Path:
    """Approval markers live beside the story files and are gitignored."""
    assert kind in ("spec", "done")
    return folder / "stories" / f"{story_id}.{kind}.approved"


# ---------------------------------------------------------------------------
# Checkpoint logic (pure)
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class Action:
    kind: str  # skip | dispatch | pause-spec | pause-done | stop
    plan_only: bool = False
    reason: str = ""


HALT_AFTER_PLANNING = "Halt after planning."


def decide(story: Story, st: StoryStatus, spec_approved: bool, done_approved: bool) -> Action:
    """What the driver should do next for one story, given its on-disk state.

    spec_checkpoint: a story without a spec approval marker is only ever
    dispatched with build-auto's "Halt after planning." directive, so the most
    it can reach is `ready-for-dev`; there it pauses until
    `stories/<id>.spec.approved` exists. Finding it already past planning
    without approval is a stop, never a silent continue.
    done_checkpoint: after `done`, the queue pauses until
    `stories/<id>.done.approved` exists.
    """
    if st.status == "invalid":
        return Action("stop", reason=st.error or "invalid story file")
    if st.status == "done":
        if story.done_checkpoint and not done_approved:
            return Action("pause-done", reason=f"done_checkpoint: review story {story.id}, then approve it")
        return Action("skip", reason="done")
    if st.status == "blocked":
        return Action("stop", reason="story is blocked (see its ## Auto Run Result)")
    if story.spec_checkpoint and not spec_approved:
        if st.status in (MISSING, "draft"):
            return Action("dispatch", plan_only=True, reason="spec_checkpoint: plan only")
        if st.status == "ready-for-dev":
            return Action("pause-spec", reason=f"spec_checkpoint: review the planned spec for story {story.id}, then approve it")
        return Action("stop", reason=f"spec_checkpoint violated: status is {st.status} but the spec was never approved")
    return Action("dispatch", reason=f"status {st.status}")


def build_prompt(folder_rel: str, story: Story, plan_only: bool) -> str:
    prompt = f"/bmad-build-auto spec folder {folder_rel}, story id {story.id}."
    if story.invoke_dev_with:
        prompt += " " + " ".join(story.invoke_dev_with.split())
    if plan_only:
        prompt += " " + HALT_AFTER_PLANNING
    return prompt


# ---------------------------------------------------------------------------
# Scheduling helpers: quiet hours and rolling budget
# ---------------------------------------------------------------------------

def parse_quiet_hours(spec: Optional[str]) -> List[Tuple[dt.time, dt.time]]:
    if not spec:
        return []
    out = []
    for part in spec.split(","):
        part = part.strip()
        m = re.fullmatch(r"(\d{1,2}):?(\d{2})?-(\d{1,2}):?(\d{2})?", part)
        if not m:
            raise ValueError(f"bad quiet-hours range {part!r}; expected HH:MM-HH:MM")
        a = dt.time(int(m.group(1)) % 24, int(m.group(2) or 0))
        b = dt.time(int(m.group(3)) % 24, int(m.group(4) or 0))
        out.append((a, b))
    return out


def quiet_until(now_local: dt.datetime, ranges: Sequence[Tuple[dt.time, dt.time]]) -> Optional[dt.datetime]:
    """If `now_local` falls in a quiet range, return when that range ends."""
    for start, end in ranges:
        t = now_local.timetz().replace(tzinfo=None)
        day = now_local.replace(hour=0, minute=0, second=0, microsecond=0)
        if start <= end:
            if start <= t < end:
                return day.replace(hour=end.hour, minute=end.minute)
        else:  # wraps midnight, e.g. 22:00-07:00
            if t >= start:
                return (day + dt.timedelta(days=1)).replace(hour=end.hour, minute=end.minute)
            if t < end:
                return day.replace(hour=end.hour, minute=end.minute)
    return None


def budget_wait_until(history: Sequence[float], now_ts: float, budget: Optional[int], window_s: float) -> Optional[float]:
    """Rolling window: at most `budget` dispatches per `window_s`. Returns when the next is allowed."""
    if not budget:
        return None
    recent = sorted(t for t in history if now_ts - t < window_s)
    if len(recent) < budget:
        return None
    return recent[len(recent) - budget] + window_s


# ---------------------------------------------------------------------------
# State, lock, git
# ---------------------------------------------------------------------------

class State:
    def __init__(self, path: Path):
        self.path = path
        self.data: Dict[str, Any] = {}
        if path.is_file():
            try:
                self.data = json.loads(path.read_text(encoding="utf-8"))
            except ValueError:
                bak = path.with_suffix(".corrupt.json")
                path.replace(bak)
                sys.stderr.write(f"bmad-driver: state.json unreadable, moved to {bak}\n")
        self.data.setdefault("version", 1)
        self.data.setdefault("attempts", {})
        self.data.setdefault("dispatch_history", [])
        self.data.setdefault("limit_backoff_level", 0)

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.data["updated_at"] = dt.datetime.now(UTC).isoformat()
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        os.replace(tmp, self.path)

    def __getitem__(self, k):
        return self.data.get(k)

    def __setitem__(self, k, v):
        self.data[k] = v


class Lock:
    def __init__(self, path: Path):
        self.path = path
        self.fh = None

    def acquire(self) -> bool:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.fh = open(self.path, "a+")
        try:
            fcntl.flock(self.fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as e:
            if e.errno in (errno.EAGAIN, errno.EACCES, errno.EWOULDBLOCK):
                self.fh.close()
                self.fh = None
                return False
            raise
        self.fh.seek(0)
        self.fh.truncate()
        self.fh.write(f"{os.getpid()}\n")
        self.fh.flush()
        return True

    def holder(self) -> str:
        try:
            return self.path.read_text().strip()
        except OSError:
            return "?"

    def release(self) -> None:
        if self.fh:
            fcntl.flock(self.fh.fileno(), fcntl.LOCK_UN)
            self.fh.close()
            self.fh = None


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)


def dirty_paths(repo: Path) -> List[str]:
    cp = git(repo, "status", "--porcelain", "--untracked-files=all")
    if cp.returncode != 0:
        raise RuntimeError(f"git status failed: {cp.stderr.strip()}")
    out = []
    for line in cp.stdout.splitlines():
        if len(line) > 3:
            p = line[3:]
            if " -> " in p:
                p = p.split(" -> ", 1)[1]
            out.append(p.strip('"'))
    return out


def pid_alive(pid: Optional[int]) -> bool:
    """True if `pid` is alive and still looks like the claude run we started (guards pid reuse)."""
    if not pid:
        return False
    try:
        os.kill(int(pid), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    cp = subprocess.run(["ps", "-p", str(int(pid)), "-o", "command="], capture_output=True, text=True)
    return cp.returncode == 0 and "bmad-build-auto" in cp.stdout


# ---------------------------------------------------------------------------
# The driver
# ---------------------------------------------------------------------------

class Halt(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class Driver:
    def __init__(self, args: argparse.Namespace, repo: Path):
        self.a = args
        self.repo = repo
        self.driver_dir = repo / ".scratch" / ".driver"
        self.state = State(self.driver_dir / "state.json")
        self.dispatches_this_run = 0
        self.child: Optional[subprocess.Popen] = None
        self.quiet = parse_quiet_hours(args.quiet_hours)

    # -- output ------------------------------------------------------------
    def log(self, msg: str) -> None:
        stamp = dt.datetime.now().strftime("%H:%M:%S")
        print(f"[bmad-driver {stamp}] {msg}", flush=True)

    def notify(self, title: str, msg: str) -> None:
        if sys.stdout.isatty():
            sys.stdout.write("\a")
            sys.stdout.flush()
        if not self.a.notify or sys.platform != "darwin" or not shutil.which("osascript"):
            return
        script = f"display notification {json.dumps(msg)} with title {json.dumps(title)}"
        subprocess.run(["osascript", "-e", script], capture_output=True)

    # -- folders -----------------------------------------------------------
    def rel(self, p: Path) -> str:
        try:
            return str(p.resolve().relative_to(self.repo.resolve()))
        except ValueError:
            return str(p)

    def resolve_folders(self) -> List[Path]:
        scratch = self.repo / ".scratch"
        if self.a.all:
            queue_file = scratch / "QUEUE"
            candidates = sorted(p.parent for p in scratch.glob("*/stories.yaml") if not p.parent.name.startswith("."))
            if queue_file.is_file():
                listed = []
                for line in queue_file.read_text(encoding="utf-8").splitlines():
                    line = line.split("#", 1)[0].strip()
                    if not line:
                        continue
                    p = Path(line)
                    if not p.is_absolute():
                        p = self.repo / p if "/" in line else scratch / p
                    listed.append(p)
                unlisted = [c for c in candidates if c.resolve() not in {p.resolve() for p in listed}]
                for u in unlisted:
                    self.log(f"note: {self.rel(u)} has stories.yaml but is not in .scratch/QUEUE; skipped")
                return listed
            return candidates
        out = []
        for f in self.a.folders:
            p = Path(f)
            if not p.is_absolute():
                p = (Path.cwd() / p) if (Path.cwd() / p).exists() else self.repo / p
            if not p.exists() and (scratch / f).exists():
                p = scratch / f
            out.append(p)
        return out

    # -- waiting -----------------------------------------------------------
    def sleep_until(self, until: dt.datetime, reason: str) -> None:
        now = dt.datetime.now(UTC)
        if until <= now:
            return
        self.state["sleeping_until"] = until.isoformat()
        self.state["sleep_reason"] = reason
        self.state.save()
        local = until.astimezone()
        if self.a.no_wait:
            raise Halt(EXIT_SLEEPING, f"{reason}; would sleep until {local:%Y-%m-%d %H:%M %Z} (--no-wait)")
        self.log(f"{reason}; sleeping until {local:%Y-%m-%d %H:%M:%S %Z} ({(until - now).total_seconds() / 60:.1f} min)")
        while True:
            remaining = (until - dt.datetime.now(UTC)).total_seconds()
            if remaining <= 0:
                break
            time.sleep(min(remaining, 60.0))
        self.state["sleeping_until"] = None
        self.state["sleep_reason"] = None
        self.state.save()

    def resume_pending_sleep(self) -> None:
        su = self.state["sleeping_until"]
        if su:
            try:
                until = dt.datetime.fromisoformat(su)
            except ValueError:
                return
            self.sleep_until(until, f"resuming earlier sleep ({self.state['sleep_reason'] or 'usage limit'})")

    def pre_dispatch_gates(self) -> None:
        if self.a.max_dispatches is not None and self.dispatches_this_run >= self.a.max_dispatches:
            raise Halt(EXIT_OK, f"--max-dispatches {self.a.max_dispatches} reached for this run")
        if self.quiet:
            now_local = dt.datetime.now().astimezone()
            q = quiet_until(now_local, self.quiet)
            if q:
                self.sleep_until(q.astimezone(UTC), "quiet hours")
        hist = [h["t"] for h in self.state["dispatch_history"] if not h.get("limit")]
        wait = budget_wait_until(hist, time.time(), self.a.window_budget, self.a.window_hours * 3600)
        if wait:
            self.sleep_until(dt.datetime.fromtimestamp(wait, UTC),
                             f"window budget {self.a.window_budget}/{self.a.window_hours}h used")

    def check_tree(self, folder: Path, story: Story, st: StoryStatus) -> None:
        if self.a.allow_dirty:
            return
        dirty = dirty_paths(self.repo)
        if not dirty:
            return
        key = self.key(folder, story)
        folder_rel = self.rel(folder).rstrip("/") + "/"
        if st.status != MISSING:
            # build-auto resumes an existing story without its clean-tree check;
            # partial work of this same story is expected after an interrupted run.
            if self.state["last_dispatched"] == key or all(p.startswith(folder_rel) for p in dirty):
                return
        shown = "\n  ".join(dirty[:15]) + ("\n  ..." if len(dirty) > 15 else "")
        raise Halt(EXIT_PRECONDITION,
                   f"working tree is dirty before dispatching {key} (status {st.status}); "
                   f"build-auto needs a clean tree for a first dispatch. Commit or stash:\n  {shown}")

    # -- core --------------------------------------------------------------
    @staticmethod
    def key(folder: Path, story: Story) -> str:
        return f"{folder.name}::{story.id}"

    def claude_cmd(self, prompt: str) -> List[str]:
        claude = self.a.claude
        cmd = shlex.split(claude) if " " in claude and not Path(claude).exists() else [claude]
        cmd += ["-p", prompt, "--output-format", "stream-json", "--verbose"]
        if self.a.permission_mode:
            cmd += ["--permission-mode", self.a.permission_mode]
        if self.a.model:
            cmd += ["--model", self.a.model]
        for extra in self.a.claude_arg or []:
            cmd += shlex.split(extra)
        return cmd

    def dispatch(self, folder: Path, story: Story, plan_only: bool) -> Tuple[int, RunScan, Path]:
        prompt = build_prompt(self.rel(folder), story, plan_only)
        cmd = self.claude_cmd(prompt)
        logs = self.driver_dir / "logs"
        logs.mkdir(parents=True, exist_ok=True)
        stamp = dt.datetime.now().strftime("%Y%m%dT%H%M%S")
        log_path = logs / f"{stamp}-{folder.name}-{story.id}.log"
        key = self.key(folder, story)
        self.state["current"] = {"folder": self.rel(folder), "id": story.id, "plan_only": plan_only,
                                 "started_at": dt.datetime.now(UTC).isoformat(), "log": self.rel(log_path)}
        self.state["last_dispatched"] = key
        self.log(f"dispatch {key}{' (plan only)' if plan_only else ''} -> {self.rel(log_path)}")
        lines: List[str] = []
        started = time.time()
        timed_out = False
        with open(log_path, "w", encoding="utf-8") as fh:
            fh.write(f"# cmd: {shlex.join(cmd)}\n# cwd: {self.repo}\n# started: {dt.datetime.now().astimezone().isoformat()}\n")
            fh.flush()
            self.child = subprocess.Popen(cmd, cwd=self.repo, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                          stdin=subprocess.DEVNULL, text=True, bufsize=1, start_new_session=True)
            self.state["child_pid"] = self.child.pid
            self.state.save()
            assert self.child.stdout is not None
            deadline = started + self.a.timeout_min * 60
            for line in self.child.stdout:
                fh.write(line)
                fh.flush()
                lines.append(line)
                if len(lines) > 20000:
                    del lines[:5000]
                if time.time() > deadline:
                    timed_out = True
                    self.kill_child()
                    break
            rc = self.child.wait()
            fh.write(f"\n# exit: {rc}{' (timeout)' if timed_out else ''} after {time.time() - started:.0f}s\n")
        self.child = None
        self.state["child_pid"] = None
        self.dispatches_this_run += 1
        scan = scan_run_output(lines, dt.datetime.now(UTC))
        self.state["dispatch_history"] = ([h for h in self.state["dispatch_history"] if time.time() - h["t"] < 8 * 86400]
                                          + [{"t": started, "key": key, "limit": scan.limit is not None}])
        if timed_out:
            rc = 124
        return rc, scan, log_path

    def kill_child(self) -> None:
        if self.child and self.child.poll() is None:
            try:
                os.killpg(self.child.pid, signal.SIGTERM)
                try:
                    self.child.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    os.killpg(self.child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def limit_sleep(self, hit: LimitHit) -> None:
        now = dt.datetime.now(UTC)
        if hit.reset_at:
            self.state["limit_backoff_level"] = 0
            until = hit.reset_at + dt.timedelta(seconds=self.a.limit_buffer_sec)
            if until < now:
                until = now + dt.timedelta(seconds=max(self.a.limit_buffer_sec, 1))
            if (until - now).total_seconds() > self.a.max_sleep_hours * 3600:
                raise Halt(EXIT_SLEEPING, f"usage limit resets {hit.reset_at.astimezone():%Y-%m-%d %H:%M %Z}, "
                                          f"beyond --max-sleep-hours {self.a.max_sleep_hours}; rerun after the reset")
            reason = f"usage limit hit ({hit.source}: {hit.raw[:120]!r})"
        else:
            level = int(self.state["limit_backoff_level"] or 0)
            delay = min(self.a.backoff_initial_sec * (2 ** level), self.a.backoff_max_sec)
            self.state["limit_backoff_level"] = level + 1
            until = now + dt.timedelta(seconds=delay)
            reason = f"usage limit hit, reset time not parseable ({hit.raw[:120]!r}); backoff level {level}"
        self.state["last_limit"] = hit.to_json()
        self.state.save()
        self.sleep_until(until, reason)

    def run_folder(self, folder: Path) -> str:
        """Process one spec folder. Returns 'done' or 'paused-spec'. Raises Halt otherwise."""
        try:
            stories = load_stories(folder)
        except SpecError as e:
            raise Halt(EXIT_BLOCKED, str(e))
        self.log(f"folder {self.rel(folder)}: {len(stories)} stories")
        for story in stories:
            key = self.key(folder, story)
            while True:
                st = read_story_status(folder, story.id)
                spec_ok = marker_path(folder, story.id, "spec").exists()
                done_ok = marker_path(folder, story.id, "done").exists()
                act = decide(story, st, spec_ok, done_ok)
                if act.kind == "skip":
                    break
                if act.kind == "pause-spec":
                    msg = (f"{key} planned and waiting for spec sign-off: review {self.rel(st.path) if st.path else '?'}, "
                           f"then `scripts/bmad-driver.sh approve {self.rel(folder)} {story.id} --spec`")
                    self.log("CHECKPOINT " + msg)
                    self.notify("bmad-driver: spec checkpoint", f"{key} ready for review")
                    self.state["last_result"] = {"key": key, "result": "paused-spec", "at": dt.datetime.now(UTC).isoformat()}
                    self.state.save()
                    return "paused-spec"
                if act.kind == "pause-done":
                    msg = (f"{key} is done and has done_checkpoint: review it, then "
                           f"`scripts/bmad-driver.sh approve {self.rel(folder)} {story.id} --done`")
                    self.notify("bmad-driver: done checkpoint", f"{key} done; queue paused")
                    self.state["last_result"] = {"key": key, "result": "paused-done", "at": dt.datetime.now(UTC).isoformat()}
                    self.state.save()
                    raise Halt(EXIT_CHECKPOINT, "CHECKPOINT " + msg)
                if act.kind == "stop":
                    self.state["last_result"] = {"key": key, "result": "stopped", "reason": act.reason,
                                                 "at": dt.datetime.now(UTC).isoformat()}
                    self.state.save()
                    raise Halt(EXIT_BLOCKED, f"{key}: {act.reason}")

                # dispatch
                phase = "plan" if act.plan_only else "build"
                akey = f"{key}::{phase}"
                attempts = int(self.state["attempts"].get(akey, 0))
                if attempts > self.a.max_retries:
                    raise Halt(EXIT_BLOCKED, f"{key}: {attempts} dispatches ({phase}) without reaching a terminal "
                                             f"status; last status {st.status}. See .scratch/.driver/logs/")
                self.check_tree(folder, story, st)
                self.pre_dispatch_gates()
                before = st.status
                rc, scan, log_path = self.dispatch(folder, story, act.plan_only)
                after = read_story_status(folder, story.id)
                terminal = after.status in ("done", "blocked") or (act.plan_only and after.status == "ready-for-dev")
                result = {"key": key, "phase": phase, "exit": rc, "before": before, "after": after.status,
                          "log": self.rel(log_path), "at": dt.datetime.now(UTC).isoformat()}
                if scan.limit and not terminal:
                    result["result"] = "usage-limit"
                    self.state["last_result"] = result
                    self.state.save()
                    self.log(f"{key}: usage limit hit (status {before} -> {after.status}); will resume the same story")
                    self.limit_sleep(scan.limit)
                    continue  # same story, no attempt consumed
                if scan.auth_error and not terminal:
                    result["result"] = "auth-error"
                    self.state["last_result"] = result
                    self.state.save()
                    raise Halt(EXIT_PRECONDITION, f"{key}: claude reported an auth/billing problem: {scan.auth_error}")
                self.state["limit_backoff_level"] = 0
                if terminal:
                    self.state["attempts"].pop(akey, None)
                    result["result"] = after.status
                else:
                    self.state["attempts"][akey] = attempts + 1
                    result["result"] = "incomplete"
                    self.log(f"{key}: run ended (exit {rc}) at status {after.status}; attempt "
                             f"{attempts + 1}/{self.a.max_retries + 1}")
                self.state["last_result"] = result
                self.state["current"] = None
                self.state.save()
                if scan.limit and terminal:
                    # The story finished but the window is exhausted; wait before the next dispatch.
                    self.limit_sleep(scan.limit)
                if self.a.once:
                    raise Halt(EXIT_OK, f"--once: dispatched {key}, now {after.status}")
        return "done"

    def dry_run(self, folders: List[Path]) -> int:
        dirty = dirty_paths(self.repo)
        print(f"repo: {self.repo}")
        print(f"working tree: {'clean' if not dirty else f'DIRTY ({len(dirty)} paths)'}")
        su = self.state["sleeping_until"]
        if su:
            print(f"state: sleeping until {su} ({self.state['sleep_reason']})")
        for folder in folders:
            print(f"\n{self.rel(folder)}")
            try:
                stories = load_stories(folder)
            except SpecError as e:
                print(f"  ERROR {e}")
                continue
            blocked_here = False
            for story in stories:
                st = read_story_status(folder, story.id)
                act = decide(story, st, marker_path(folder, story.id, "spec").exists(),
                             marker_path(folder, story.id, "done").exists())
                flags = "".join([" [spec_checkpoint]" if story.spec_checkpoint else "",
                                 " [done_checkpoint]" if story.done_checkpoint else ""])
                if blocked_here:
                    what = "queued"
                elif act.kind == "skip":
                    what = "skip (done)"
                elif act.kind == "dispatch":
                    prompt = build_prompt(self.rel(folder), story, act.plan_only)
                    what = "NEXT dispatch: " + shlex.join(self.claude_cmd(prompt))
                    blocked_here = True
                else:
                    what = f"{act.kind.upper()}: {act.reason}"
                    blocked_here = True
                print(f"  {story.id:>6} {st.status:<13} {story.title}{flags}\n         -> {what}")
        return EXIT_OK

    def run(self, folders: List[Path]) -> int:
        if self.a.dry_run:
            return self.dry_run(folders)
        lock = Lock(self.driver_dir / "lock")
        if not lock.acquire():
            print(f"bmad-driver: another driver is running (pid {lock.holder()}); lock {self.rel(lock.path)}", file=sys.stderr)
            return EXIT_LOCKED
        old_int = signal.getsignal(signal.SIGINT)
        old_term = signal.getsignal(signal.SIGTERM)

        def on_signal(signum, _frame):
            self.kill_child()
            raise Halt(130, f"interrupted by signal {signum}; state saved, rerun to resume")

        signal.signal(signal.SIGINT, on_signal)
        signal.signal(signal.SIGTERM, on_signal)
        try:
            if pid_alive(self.state["child_pid"]):
                raise Halt(EXIT_PRECONDITION, f"a claude run from a previous driver (pid {self.state['child_pid']}) "
                                              f"is still alive; wait for it or kill it, then rerun")
            self.state["child_pid"] = None
            exe = shlex.split(self.a.claude)[0] if " " in self.a.claude and not Path(self.a.claude).exists() else self.a.claude
            if not shutil.which(exe) and not Path(exe).is_file():
                raise Halt(EXIT_PRECONDITION, f"claude executable not found: {exe}")
            if Path(exe).name == "claude":
                skill_dirs = [self.repo / ".claude/skills/bmad-build-auto", Path.home() / ".claude/skills/bmad-build-auto"]
                if not any((d / "SKILL.md").is_file() for d in skill_dirs):
                    raise Halt(EXIT_PRECONDITION, "bmad-build-auto skill not installed in this checkout "
                                                  "(.claude/skills/bmad-build-auto) or ~/.claude/skills")
                if not (self.repo / "_bmad/scripts/render_skill.py").is_file():
                    raise Halt(EXIT_PRECONDITION, "_bmad/scripts/render_skill.py missing: bmad-build-auto cannot "
                                                  "render its workflow in this checkout")
            self.resume_pending_sleep()
            paused = []
            for folder in folders:
                if self.run_folder(folder) == "paused-spec":
                    paused.append(folder)
            if paused:
                raise Halt(EXIT_CHECKPOINT, "paused at spec checkpoint(s): " + ", ".join(self.rel(p) for p in paused))
            self.notify("bmad-driver", "queue complete")
            self.log("all stories done")
            return EXIT_OK
        except Halt as h:
            if h.code in (EXIT_OK, EXIT_CHECKPOINT, EXIT_SLEEPING):
                self.log(h.message)
            else:
                print(f"[bmad-driver] HALT: {h.message}", file=sys.stderr, flush=True)
                self.notify("bmad-driver: halted", h.message[:200])
            self.state["halt"] = {"code": h.code, "message": h.message, "at": dt.datetime.now(UTC).isoformat()}
            self.state.save()
            return h.code
        finally:
            signal.signal(signal.SIGINT, old_int)
            signal.signal(signal.SIGTERM, old_term)
            lock.release()


# ---------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------

def cmd_status(args: argparse.Namespace, repo: Path) -> int:
    d = Driver(args, repo)
    st = d.state
    print(f"repo: {repo}")
    lock = Lock(d.driver_dir / "lock")
    if lock.acquire():
        lock.release()
        print("driver: not running")
    else:
        print(f"driver: RUNNING (pid {lock.holder()})")
    for k in ("current", "sleeping_until", "sleep_reason", "last_result", "last_limit", "halt"):
        if st[k]:
            print(f"{k}: {json.dumps(st[k])}")
    if st["attempts"]:
        print(f"attempts: {json.dumps(st['attempts'])}")
    for folder in d.resolve_folders():
        print(f"\n{d.rel(folder)}")
        try:
            stories = load_stories(folder)
        except SpecError as e:
            print(f"  ERROR {e}")
            continue
        for s in stories:
            ss = read_story_status(folder, s.id)
            marks = []
            if s.spec_checkpoint:
                marks.append("spec:" + ("approved" if marker_path(folder, s.id, "spec").exists() else "pending"))
            if s.done_checkpoint:
                marks.append("done:" + ("approved" if marker_path(folder, s.id, "done").exists() else "pending"))
            print(f"  {s.id:>6} {ss.status:<13} {s.title}{'  [' + ', '.join(marks) + ']' if marks else ''}"
                  f"{'  ! ' + ss.error if ss.error else ''}")
    return EXIT_OK


def cmd_approve(args: argparse.Namespace, repo: Path) -> int:
    folder = Path(args.folder)
    if not folder.is_absolute():
        folder = repo / folder if (repo / folder).exists() else repo / ".scratch" / args.folder
    kinds = [k for k in ("spec", "done") if getattr(args, k)]
    if not kinds:
        print("approve: pass --spec and/or --done", file=sys.stderr)
        return EXIT_USAGE
    ids = {s.id for s in load_stories(folder)}
    if args.id not in ids:
        print(f"approve: story id {args.id!r} not in {folder}/stories.yaml", file=sys.stderr)
        return EXIT_USAGE
    for k in kinds:
        p = marker_path(folder, args.id, k)
        p.parent.mkdir(parents=True, exist_ok=True)
        user = os.environ.get("USER", "?")
        p.write_text(f"approved_by: {user}\napproved_at: {dt.datetime.now().astimezone().isoformat()}\n", encoding="utf-8")
        print(f"wrote {p}")
    return EXIT_OK


def find_repo(start: Path) -> Path:
    cp = subprocess.run(["git", "-C", str(start), "rev-parse", "--show-toplevel"], capture_output=True, text=True)
    if cp.returncode != 0:
        raise SystemExit(f"bmad-driver: {start} is not inside a git repository")
    return Path(cp.stdout.strip())


def env_int(name: str, default: Optional[int]) -> Optional[int]:
    v = os.environ.get(name)
    return int(v) if v not in (None, "") else default


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="bmad-driver", description="Unattended driver for /bmad-build-auto.")
    sub = p.add_subparsers(dest="command")

    def common(sp):
        sp.add_argument("folders", nargs="*", help="spec folders (e.g. .scratch/my-feature or my-feature)")
        sp.add_argument("--all", action="store_true", help="every .scratch/*/stories.yaml, ordered by .scratch/QUEUE if present")
        sp.add_argument("--repo", help="repository root (default: git toplevel of cwd)")

    r = sub.add_parser("run", help="process the queue (default command)")
    common(r)
    r.add_argument("--dry-run", action="store_true", help="print the plan; never call claude")
    r.add_argument("--once", action="store_true", help="dispatch at most one story run, then exit")
    r.add_argument("--claude", default=os.environ.get("BMAD_DRIVER_CLAUDE", "claude"), help="claude executable [$BMAD_DRIVER_CLAUDE]")
    r.add_argument("--model", default=os.environ.get("BMAD_DRIVER_MODEL") or None, help="--model for claude [$BMAD_DRIVER_MODEL]")
    r.add_argument("--permission-mode", default=os.environ.get("BMAD_DRIVER_PERMISSION_MODE", "bypassPermissions"),
                   help="claude --permission-mode (default bypassPermissions) [$BMAD_DRIVER_PERMISSION_MODE]; '' to omit")
    r.add_argument("--claude-arg", action="append", help="extra argument(s) for claude, shell-split; repeatable")
    r.add_argument("--max-retries", type=int, default=env_int("BMAD_DRIVER_MAX_RETRIES", 2),
                   help="re-dispatches per story phase after a run that ends non-terminal (default 2)")
    r.add_argument("--timeout-min", type=float, default=240, help="kill a single claude run after this many minutes")
    r.add_argument("--limit-buffer-sec", type=int, default=120, help="sleep this long past a parsed reset time")
    r.add_argument("--backoff-initial-sec", type=int, default=300, help="first backoff when the reset time is unknown")
    r.add_argument("--backoff-max-sec", type=int, default=1800, help="backoff cap when the reset time is unknown")
    r.add_argument("--max-sleep-hours", type=float, default=8 * 24, help="halt instead of sleeping past this")
    r.add_argument("--max-dispatches", type=int, default=env_int("BMAD_DRIVER_MAX_DISPATCHES", None),
                   help="stop after this many claude runs in this driver run")
    r.add_argument("--window-budget", type=int, default=env_int("BMAD_DRIVER_WINDOW_BUDGET", None),
                   help="at most N claude runs per rolling --window-hours (keeps usage for interactive work)")
    r.add_argument("--window-hours", type=float, default=5.0)
    r.add_argument("--quiet-hours", default=os.environ.get("BMAD_DRIVER_QUIET_HOURS"),
                   help="local HH:MM-HH:MM[,..] ranges in which no run starts, e.g. 09:00-18:00")
    r.add_argument("--no-wait", action="store_true", help="exit 75 instead of sleeping (for cron/launchd)")
    r.add_argument("--allow-dirty", action="store_true", help="skip the driver's clean-tree check")
    r.add_argument("--notify", dest="notify", action="store_true", default=os.environ.get("BMAD_DRIVER_NOTIFY", "1") != "0")
    r.add_argument("--no-notify", dest="notify", action="store_false")

    s = sub.add_parser("status", help="print queue state")
    common(s)
    s.set_defaults(notify=False, quiet_hours=None, claude="claude", all_default=True)

    a = sub.add_parser("approve", help="write a checkpoint approval marker")
    a.add_argument("folder")
    a.add_argument("id")
    a.add_argument("--spec", action="store_true", help="approve the planned spec (spec_checkpoint)")
    a.add_argument("--done", action="store_true", help="approve the finished story (done_checkpoint)")
    a.add_argument("--repo")
    return p


def main(argv: Optional[Sequence[str]] = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or (argv[0] not in ("run", "status", "approve", "-h", "--help")):
        argv = ["run"] + argv
    args = build_parser().parse_args(argv)
    repo = Path(args.repo).resolve() if getattr(args, "repo", None) else find_repo(Path.cwd())
    if args.command == "approve":
        return cmd_approve(args, repo)
    if args.command == "status":
        if not args.folders:
            args.all = True
        return cmd_status(args, repo)
    if not args.all and not args.folders:
        print("bmad-driver: name spec folder(s) or pass --all", file=sys.stderr)
        return EXIT_USAGE
    try:
        args.quiet_hours and parse_quiet_hours(args.quiet_hours)
    except ValueError as e:
        print(f"bmad-driver: {e}", file=sys.stderr)
        return EXIT_USAGE
    d = Driver(args, repo)
    folders = d.resolve_folders()
    missing = [f for f in folders if not (f / "stories.yaml").is_file()]
    if missing and args.all:
        # A QUEUE entry whose stories.yaml is not written yet: skip it, do not abort the queue.
        for m in missing:
            print(f"bmad-driver: note: {m} listed in .scratch/QUEUE has no stories.yaml yet; skipped", file=sys.stderr)
        folders = [f for f in folders if f not in missing]
    elif missing:
        print("bmad-driver: no stories.yaml in: " + ", ".join(str(m) for m in missing), file=sys.stderr)
        return EXIT_USAGE
    if not folders:
        print("bmad-driver: no spec folders found", file=sys.stderr)
        return EXIT_OK
    return d.run(folders)


if __name__ == "__main__":
    sys.exit(main())
