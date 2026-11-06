#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml>=6", "tzdata"]
# ///
"""Tests for scripts/bmad-driver.py. Run: uv run --script scripts/tests/test_bmad_driver.py

Never calls the real claude CLI: the end-to-end cases use fake_claude.py.
"""

import datetime as dt
import fcntl
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path
from zoneinfo import ZoneInfo

HERE = Path(__file__).resolve().parent
DRIVER = HERE.parent / "bmad-driver.py"
FAKE = HERE / "fake_claude.py"

spec = importlib.util.spec_from_file_location("bmad_driver", DRIVER)
bd = importlib.util.module_from_spec(spec)
sys.modules["bmad_driver"] = bd
spec.loader.exec_module(bd)

UTC = dt.timezone.utc
NY = ZoneInfo("America/New_York")


def at(tz, *args):
    return dt.datetime(*args, tzinfo=tz)


class ResetTimeParsing(unittest.TestCase):
    def check(self, msg, now, expected):
        hit = bd.parse_limit_message(msg, now)
        self.assertIsNotNone(hit, msg)
        self.assertEqual(hit.reset_at, expected.astimezone(UTC) if expected else None, msg)

    def test_current_cli_format_later_today(self):
        # Verified against the claude 2.1.318 binary: `You've hit your ${limitName} · resets ${time} (${tz})`.
        self.check("You've hit your 5-hour limit · resets 3pm (America/New_York)",
                   at(NY, 2026, 11, 5, 12, 0), at(NY, 2026, 11, 5, 15, 0))

    def test_bare_time_already_passed_means_tomorrow(self):
        self.check("You've hit your 5-hour limit · resets 3pm (America/New_York)",
                   at(NY, 2026, 11, 5, 16, 30), at(NY, 2026, 11, 6, 15, 0))

    def test_bare_time_just_passed_is_not_tomorrow(self):
        self.check("You've hit your 5-hour limit · resets 3pm (America/New_York)",
                   at(NY, 2026, 11, 5, 15, 4), at(NY, 2026, 11, 5, 15, 0))

    def test_minutes_and_progress_saved_suffix(self):
        tokyo = ZoneInfo("Asia/Tokyo")
        self.check("You've hit your usage limit · resets 3:30am (Asia/Tokyo) · progress saved",
                   at(tokyo, 2026, 11, 5, 23, 0), at(tokyo, 2026, 11, 6, 3, 30))

    def test_timezone_differs_from_now(self):
        # now expressed in UTC; the message's zone governs the wall clock.
        self.check("You've hit your Opus limit · resets 9am (Europe/Berlin)",
                   at(UTC, 2026, 11, 5, 6, 0), at(ZoneInfo("Europe/Berlin"), 2026, 11, 5, 9, 0))

    def test_noon_and_midnight_12h_edges(self):
        self.check("You've hit your 5-hour limit · resets 12pm (America/New_York)",
                   at(NY, 2026, 11, 5, 10, 0), at(NY, 2026, 11, 5, 12, 0))
        self.check("You've hit your 5-hour limit · resets 12am (America/New_York)",
                   at(NY, 2026, 11, 5, 22, 0), at(NY, 2026, 11, 6, 0, 0))

    def test_weekly_with_date(self):
        self.check("You've hit your weekly limit · resets Nov 7, 3pm (Europe/Berlin)",
                   at(UTC, 2026, 11, 5, 12, 0), at(ZoneInfo("Europe/Berlin"), 2026, 11, 7, 15, 0))

    def test_weekly_with_at_separator(self):
        # Newer ICU renders en-US date+time as "Nov 7 at 3 PM".
        self.check("You've hit your weekly limit · resets Nov 7 at 3:15 PM (America/Chicago)",
                   at(UTC, 2026, 11, 5, 12, 0), at(ZoneInfo("America/Chicago"), 2026, 11, 7, 15, 15))

    def test_date_with_year_and_rollover(self):
        la = ZoneInfo("America/Los_Angeles")
        self.check("You've hit your limit · resets Jan 2, 2027, 3pm (America/Los_Angeles)",
                   at(la, 2026, 12, 30, 9, 0), at(la, 2027, 1, 2, 15, 0))
        # No year, date earlier in the calendar than now: next year.
        self.check("You've hit your weekly limit · resets Jan 2, 3pm (America/Los_Angeles)",
                   at(la, 2026, 12, 30, 9, 0), at(la, 2027, 1, 2, 15, 0))

    def test_legacy_pipe_epoch(self):
        self.check("Claude AI usage limit reached|1762380000", at(UTC, 2026, 11, 5, 0, 0),
                   dt.datetime.fromtimestamp(1762380000, UTC))
        self.check("Claude AI usage limit reached|1762380000000", at(UTC, 2026, 11, 5, 0, 0),
                   dt.datetime.fromtimestamp(1762380000, UTC))

    def test_older_bullet_format(self):
        hit = bd.parse_limit_message("5-hour limit reached ∙ resets 3pm", at(NY, 2026, 11, 5, 12, 0), default_tz=NY)
        self.assertEqual(hit.reset_at, at(NY, 2026, 11, 5, 15, 0).astimezone(UTC))

    def test_relative(self):
        now = at(UTC, 2026, 11, 5, 12, 0)
        self.check("Usage limit reached. Your limit resets in 2h 15m.", now, now + dt.timedelta(hours=2, minutes=15))
        self.check("You've hit your 5-hour limit · resets in 45 minutes", now, now + dt.timedelta(minutes=45))

    def test_iso_utc_spend_limit(self):
        self.check("spend limit reached (daily; resets 2026-08-08 00:00 UTC) — request an increase",
                   at(UTC, 2026, 8, 7, 12, 0), at(UTC, 2026, 8, 8, 0, 0))

    def test_abbreviated_zone(self):
        self.check("You've hit your 5-hour limit · resets 3pm PT", at(UTC, 2026, 11, 5, 12, 0),
                   at(ZoneInfo("America/Los_Angeles"), 2026, 11, 5, 15, 0))

    def test_default_zone_is_local_when_unstated(self):
        now = at(NY, 2026, 11, 5, 12, 0)
        hit = bd.parse_limit_message("You've hit your 5-hour limit · resets 3pm", now, default_tz=NY)
        self.assertEqual(hit.reset_at, at(NY, 2026, 11, 5, 15, 0).astimezone(UTC))

    def test_unparseable_reset_still_a_limit(self):
        self.check("You've hit your usage limit", at(UTC, 2026, 11, 5, 12, 0), None)
        self.check("API Error: 429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\"}}",
                   at(UTC, 2026, 11, 5, 12, 0), None)

    def test_not_a_limit(self):
        now = at(UTC, 2026, 11, 5, 12, 0)
        for msg in ["All 100 tests passed", "implemented the token-bucket rate limiter",
                    "the bay limit is 24 drives", "story 1: done"]:
            self.assertIsNone(bd.parse_limit_message(msg, now), msg)


class StreamScan(unittest.TestCase):
    now = at(UTC, 2026, 11, 5, 12, 0)

    def lines(self, *objs):
        return [json.dumps(o) if isinstance(o, dict) else o for o in objs]

    def test_rate_limit_event_rejected(self):
        scan = bd.scan_run_output(self.lines(
            {"type": "system", "subtype": "init"},
            {"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 1762380000}},
            {"type": "result", "is_error": True, "result": "You've hit your 5-hour limit · resets 3pm"}), self.now)
        self.assertEqual(scan.limit.source, "rate_limit_event")
        self.assertEqual(scan.limit.reset_at, dt.datetime.fromtimestamp(1762380000, UTC))

    def test_rejected_then_allowed_is_not_a_limit(self):
        scan = bd.scan_run_output(self.lines(
            {"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 1762380000}},
            {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}},
            {"type": "result", "is_error": False, "result": "done"}), self.now)
        self.assertIsNone(scan.limit)

    def test_tool_output_never_trips_detection(self):
        scan = bd.scan_run_output(self.lines(
            {"type": "user", "message": {"content": [{"type": "tool_result",
                                                      "content": "You've hit your 5-hour limit · resets 3pm"}]}},
            {"type": "assistant", "message": {"content": [{"type": "text",
                                                           "text": "I added parsing for 'usage limit reached'"}]}},
            {"type": "result", "is_error": False, "result": "story done"}), self.now)
        self.assertIsNone(scan.limit)

    def test_assistant_api_error_message(self):
        scan = bd.scan_run_output(self.lines(
            {"type": "assistant", "error": "rate_limit",
             "message": {"content": [{"type": "text", "text": "You've hit your weekly limit · resets Nov 7, 3pm (UTC)"}]}}),
            self.now)
        self.assertEqual(scan.limit.reset_at, at(UTC, 2026, 11, 7, 15, 0))

    def test_plain_stderr_tail(self):
        scan = bd.scan_run_output(["some log", "Claude AI usage limit reached|1762380000"], self.now)
        self.assertEqual(scan.limit.source, "legacy-epoch")

    def test_event_without_reset_falls_back_to_text(self):
        scan = bd.scan_run_output(self.lines(
            {"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}},
            {"type": "result", "is_error": True, "result": "You've hit your 5-hour limit · resets in 30m"}), self.now)
        self.assertEqual(scan.limit.reset_at, self.now + dt.timedelta(minutes=30))

    def test_auth_error(self):
        scan = bd.scan_run_output(self.lines({"type": "result", "is_error": True,
                                              "result": "Not logged in · Please run /login"}), self.now)
        self.assertIsNone(scan.limit)
        self.assertIn("login", scan.auth_error)


class StatusAndCheckpoints(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.folder = Path(self.tmp.name) / "feat"
        (self.folder / "stories").mkdir(parents=True)

    def tearDown(self):
        self.tmp.cleanup()

    def write_story(self, name, body):
        (self.folder / "stories" / name).write_text(body)

    def test_missing(self):
        self.assertEqual(bd.read_story_status(self.folder, "1").status, bd.MISSING)

    def test_status_read_and_prefix_is_exact(self):
        self.write_story("1-add-thing.md", "---\nstatus: in-review\nbaseline_revision: abc\n---\n# x\n")
        self.write_story("10-other.md", "---\nstatus: done\n---\n")
        self.assertEqual(bd.read_story_status(self.folder, "1").status, "in-review")
        self.assertEqual(bd.read_story_status(self.folder, "10").status, "done")

    def test_ambiguous_and_unrecognized(self):
        self.write_story("2-a.md", "---\nstatus: done\n---\n")
        self.write_story("2-b.md", "---\nstatus: done\n---\n")
        self.assertEqual(bd.read_story_status(self.folder, "2").status, "invalid")
        self.write_story("3-a.md", "---\nstatus: finished\n---\n")
        self.assertEqual(bd.read_story_status(self.folder, "3").status, "invalid")
        self.write_story("4-a.md", "no frontmatter\n")
        self.assertEqual(bd.read_story_status(self.folder, "4").status, "invalid")

    def test_load_stories_validation(self):
        (self.folder / "stories.yaml").write_text('- id: 1\n  title: x\n  description: y\n')
        with self.assertRaises(bd.SpecError):
            bd.load_stories(self.folder)  # unquoted id parses as int
        (self.folder / "stories.yaml").write_text('- id: "3"\n  title: a\n- id: "3-2"\n  title: b\n')
        with self.assertRaises(bd.SpecError):
            bd.load_stories(self.folder)  # not prefix-free
        (self.folder / "stories.yaml").write_text(textwrap.dedent('''\
            - id: "1"
              title: A
              description: d
              spec_checkpoint: true
              invoke_dev_with: >-
                Use the existing
                client.
            '''))
        s = bd.load_stories(self.folder)[0]
        self.assertTrue(s.spec_checkpoint)
        self.assertEqual(bd.build_prompt(".scratch/feat", s, plan_only=True),
                         "/bmad-build-auto spec folder .scratch/feat, story id 1. Use the existing client. Halt after planning.")

    def test_decide_matrix(self):
        S = bd.StoryStatus
        plain = bd.Story("1", "t")
        spec = bd.Story("2", "t", spec_checkpoint=True)
        done = bd.Story("3", "t", done_checkpoint=True)
        cases = [
            (plain, S(bd.MISSING), False, False, "dispatch", False),
            (plain, S("in-progress"), False, False, "dispatch", False),
            (plain, S("done"), False, False, "skip", False),
            (plain, S("blocked"), False, False, "stop", False),
            (plain, S("invalid", error="x"), False, False, "stop", False),
            (spec, S(bd.MISSING), False, False, "dispatch", True),
            (spec, S("draft"), False, False, "dispatch", True),
            (spec, S("ready-for-dev"), False, False, "pause-spec", False),
            (spec, S("in-progress"), False, False, "stop", False),  # implementation without sign-off
            (spec, S("ready-for-dev"), True, False, "dispatch", False),
            (spec, S("in-review"), True, False, "dispatch", False),
            (spec, S("done"), False, False, "skip", False),
            (done, S("done"), False, False, "pause-done", False),
            (done, S("done"), False, True, "skip", False),
            (done, S("in-progress"), False, False, "dispatch", False),
        ]
        for story, st, spec_ok, done_ok, kind, plan_only in cases:
            act = bd.decide(story, st, spec_ok, done_ok)
            self.assertEqual((act.kind, act.plan_only), (kind, plan_only), (story, st))


class Scheduling(unittest.TestCase):
    def test_quiet_hours(self):
        r = bd.parse_quiet_hours("09:00-18:00,22:00-07:00")
        self.assertEqual(bd.quiet_until(at(NY, 2026, 11, 5, 10, 0), r), at(NY, 2026, 11, 5, 18, 0))
        self.assertIsNone(bd.quiet_until(at(NY, 2026, 11, 5, 19, 0), r))
        self.assertEqual(bd.quiet_until(at(NY, 2026, 11, 5, 23, 0), r), at(NY, 2026, 11, 6, 7, 0))
        self.assertEqual(bd.quiet_until(at(NY, 2026, 11, 5, 3, 0), r), at(NY, 2026, 11, 5, 7, 0))

    def test_budget(self):
        self.assertIsNone(bd.budget_wait_until([100, 200], 300, 3, 1000))
        self.assertEqual(bd.budget_wait_until([100, 200, 250], 300, 3, 1000), 1100)
        self.assertIsNone(bd.budget_wait_until([100, 200, 250], 300, None, 1000))
        self.assertIsNone(bd.budget_wait_until([100, 200, 250], 1150, 3, 1000))


class EndToEnd(unittest.TestCase):
    """Drive a throwaway git repo + spec folder with the fake claude."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name) / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "feat/test")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "t")
        (self.repo / ".gitignore").write_text(".scratch/.driver/\n.scratch/*/stories/*.approved\n")
        self.folder = self.repo / ".scratch" / "feat"
        self.folder.mkdir(parents=True)
        (self.folder / "SPEC.md").write_text("# spec\n")
        (self.folder / "stories.yaml").write_text(textwrap.dedent('''\
            - id: "1"
              title: First
              description: plain story
            - id: "2"
              title: Second
              description: needs spec sign-off
              spec_checkpoint: true
              invoke_dev_with: Keep it small.
            - id: "3"
              title: Third
              description: pause after
              done_checkpoint: true
            '''))
        self.git("add", "-A")
        self.git("commit", "-qm", "init")
        self.plan = Path(self.tmp.name) / "plan.json"

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.repo), *args], check=True, capture_output=True)

    def set_plan(self, steps):
        self.plan.write_text(json.dumps(steps))
        for suffix in (".n", ".calls.jsonl"):
            p = Path(str(self.plan) + suffix)
            if p.exists():
                p.unlink()

    def calls(self):
        p = Path(str(self.plan) + ".calls.jsonl")
        return [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []

    def drive(self, *args, timeout=60):
        env = dict(os.environ, BMAD_DRIVER_CLAUDE=str(FAKE), FAKE_CLAUDE_PLAN=str(self.plan), BMAD_DRIVER_NOTIFY="0")
        cp = subprocess.run([sys.executable, str(DRIVER), *args, "--repo", str(self.repo)],
                            cwd=self.repo, env=env, capture_output=True, text=True, timeout=timeout)
        self.last = cp
        return cp.returncode

    def state(self):
        return json.loads((self.repo / ".scratch/.driver/state.json").read_text())

    def test_full_loop_with_limit_and_checkpoints(self):
        self.set_plan([
            {"limit": "event", "reset_in": 2},           # story 1: limit hit mid-run
            {"status": "done", "commit": True},          # story 1 resumed: done
            {"status": "ready-for-dev", "commit": True},  # story 2: plan only
        ])
        t0 = time.time()
        rc = self.drive(".scratch/feat", "--limit-buffer-sec", "0")
        self.assertEqual(rc, bd.EXIT_CHECKPOINT, self.last.stdout + self.last.stderr)
        self.assertGreaterEqual(time.time() - t0, 1.0, "driver must sleep until the reset")
        calls = self.calls()
        self.assertEqual(len(calls), 3)
        prompts = [c["argv"][c["argv"].index("-p") + 1] for c in calls]
        self.assertIn("story id 1.", prompts[0])
        self.assertIn("story id 1.", prompts[1])  # same story resumed after the limit
        self.assertTrue(prompts[2].endswith("story id 2. Keep it small. Halt after planning."))
        for c in calls:
            self.assertIn("--permission-mode", c["argv"])
            self.assertIn("stream-json", c["argv"])
        st = self.state()
        self.assertEqual(st["attempts"], {}, "a limit hit must not consume a retry")
        self.assertEqual(st["last_limit"]["source"], "rate_limit_event")
        self.assertIn("usage limit hit", self.last.stdout)
        logs = list((self.repo / ".scratch/.driver/logs").glob("*-feat-1.log"))
        self.assertEqual(len(logs), 2)

        # Still unapproved: a rerun pauses again without dispatching.
        self.assertEqual(self.drive(".scratch/feat"), bd.EXIT_CHECKPOINT)
        self.assertEqual(len(self.calls()), 3)

        self.assertEqual(self.drive("approve", ".scratch/feat", "2", "--spec"), 0)
        self.set_plan([{"status": "done", "commit": True}])
        rc = self.drive("--all")
        self.assertEqual(rc, bd.EXIT_CHECKPOINT, self.last.stdout + self.last.stderr)
        calls = self.calls()
        self.assertEqual(len(calls), 2)  # story 2 build, story 3
        self.assertNotIn("Halt after planning", calls[0]["argv"][calls[0]["argv"].index("-p") + 1])
        self.assertIn("done_checkpoint", self.last.stdout)

        self.assertEqual(self.drive("approve", "feat", "3", "--done"), 0)
        self.set_plan([{"status": "done"}])
        self.assertEqual(self.drive("--all"), bd.EXIT_OK, self.last.stdout + self.last.stderr)
        self.assertEqual(len(self.calls()), 0)
        self.assertEqual(self.drive("status"), 0)
        self.assertIn("spec:approved", self.last.stdout)

    def test_dry_run_calls_nothing(self):
        self.set_plan([{"status": "done"}])
        self.assertEqual(self.drive(".scratch/feat", "--dry-run"), 0)
        self.assertEqual(self.calls(), [])
        self.assertIn("NEXT dispatch", self.last.stdout)
        self.assertFalse((self.repo / ".scratch/.driver/lock").exists())

    def test_retries_exhausted_stops(self):
        self.set_plan([{"status": "in-progress", "exit": 1}])
        rc = self.drive(".scratch/feat", "--max-retries", "1", "--allow-dirty")
        self.assertEqual(rc, bd.EXIT_BLOCKED, self.last.stdout + self.last.stderr)
        self.assertEqual(len(self.calls()), 2)

    def test_unparseable_limit_backs_off_then_resumes(self):
        self.set_plan([{"limit": "unparseable"}, {"limit": "legacy", "reset_in": 1},
                       {"status": "done", "commit": True}, {"status": "ready-for-dev", "commit": True}])
        rc = self.drive(".scratch/feat", "--backoff-initial-sec", "1", "--limit-buffer-sec", "0")
        self.assertEqual(rc, bd.EXIT_CHECKPOINT, self.last.stdout + self.last.stderr)
        self.assertEqual(len(self.calls()), 4)
        self.assertIn("not parseable", self.last.stdout)
        self.assertEqual(self.state()["attempts"], {})

    def test_limit_while_dirty_resumes_same_story(self):
        # An interrupted implementation leaves partial work; the resume must not be refused.
        self.set_plan([{"status": "in-progress", "dirty": True, "limit": "text", "reset_in": 1},
                       {"status": "done", "commit": True}])
        rc = self.drive(".scratch/feat", "--once", "--limit-buffer-sec", "0")
        self.assertEqual(rc, 0, self.last.stdout + self.last.stderr)
        self.assertEqual(len(self.calls()), 2)

    def test_no_wait_exits_75_and_resumes_sleep(self):
        self.set_plan([{"limit": "event", "reset_in": 3600}])
        self.assertEqual(self.drive(".scratch/feat", "--no-wait"), bd.EXIT_SLEEPING)
        self.assertIsNotNone(self.state()["sleeping_until"])
        # A rerun honours the persisted sleep instead of dispatching again.
        self.assertEqual(self.drive(".scratch/feat", "--no-wait"), bd.EXIT_SLEEPING)
        self.assertEqual(len(self.calls()), 1)

    def test_dirty_tree_refuses_first_dispatch(self):
        (self.repo / "stray.txt").write_text("x")
        self.set_plan([{"status": "done"}])
        self.assertEqual(self.drive(".scratch/feat"), bd.EXIT_PRECONDITION)
        self.assertIn("dirty", self.last.stderr)
        self.assertEqual(self.calls(), [])

    def test_blocked_stops_folder(self):
        self.set_plan([{"status": "blocked", "commit": True}])
        self.assertEqual(self.drive(".scratch/feat"), bd.EXIT_BLOCKED)
        self.assertEqual(len(self.calls()), 1)

    def test_lock_prevents_second_driver(self):
        d = self.repo / ".scratch/.driver"
        d.mkdir(parents=True, exist_ok=True)
        with open(d / "lock", "a+") as fh:
            fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.set_plan([{"status": "done"}])
            self.assertEqual(self.drive(".scratch/feat"), bd.EXIT_LOCKED)
        self.assertEqual(self.calls(), [])

    def test_queue_file_orders_all(self):
        other = self.repo / ".scratch" / "aaa"
        other.mkdir()
        (other / "SPEC.md").write_text("# s\n")
        (other / "stories.yaml").write_text('- id: "1"\n  title: A\n  description: d\n')
        (self.repo / ".scratch" / "QUEUE").write_text("# order\nfeat\naaa\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "more")
        self.assertEqual(self.drive("--all", "--dry-run"), 0)
        out = self.last.stdout
        self.assertLess(out.index(".scratch/feat"), out.index(".scratch/aaa"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
