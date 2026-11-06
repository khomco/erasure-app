#!/usr/bin/env python3
"""Stand-in for `claude -p` used by the bmad-driver tests (stdlib only).

Point the driver at it with BMAD_DRIVER_CLAUDE=<this file>. Behaviour comes from
a JSON plan file named by FAKE_CLAUDE_PLAN: a list of steps consumed one per
invocation (a counter lives in "<plan>.n"); the last step repeats once the list
is exhausted. Every invocation's argv is appended to "<plan>.calls.jsonl".

Step keys (all optional):
  status   -- write the dispatched story's file with this frontmatter status
  commit   -- git add -A && git commit afterwards (what build-auto's finalization does)
  dirty    -- leave an untracked file behind (an interrupted implementation)
  limit    -- "event": emit a stream-json rate_limit_event rejected + the CLI's
              "You've hit your ..." result; "text": only the plain-text message;
              "legacy": "Claude AI usage limit reached|<epoch>"; "unparseable":
              a limit message with no reset time
  reset_in -- seconds until the simulated reset (default 1)
  exit     -- process exit code (default 0, or 1 when `limit` is set)
"""

import json
import os
import re
import subprocess
import sys
import time


def main() -> int:
    plan_path = os.environ["FAKE_CLAUDE_PLAN"]
    with open(plan_path) as fh:
        plan = json.load(fh)
    counter = plan_path + ".n"
    n = int(open(counter).read()) if os.path.exists(counter) else 0
    with open(counter, "w") as fh:
        fh.write(str(n + 1))
    step = plan[min(n, len(plan) - 1)]
    with open(plan_path + ".calls.jsonl", "a") as fh:
        fh.write(json.dumps({"n": n, "argv": sys.argv[1:], "cwd": os.getcwd()}) + "\n")

    prompt = sys.argv[sys.argv.index("-p") + 1]
    m = re.search(r"spec folder (\S+), story id ([A-Za-z0-9-]+)\.", prompt)
    if not m:
        print("fake claude: prompt not understood", file=sys.stderr)
        return 3
    folder, sid = m.group(1), m.group(2)

    def emit(obj):
        print(json.dumps(obj), flush=True)

    emit({"type": "system", "subtype": "init", "session_id": "fake"})
    # Tool output that merely mentions limit wording must never trip detection.
    emit({"type": "user", "message": {"content": [{"type": "tool_result",
          "content": "docs: You've hit your 5-hour limit · resets 3pm (America/New_York)"}]}})

    limit = step.get("limit")
    if limit:
        reset = int(time.time() + step.get("reset_in", 1))
        clock = time.strftime("%-I:%M%p", time.localtime(reset)).lower()
        if limit == "event":
            emit({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": reset,
                  "rateLimitType": "five_hour"}, "uuid": "u", "session_id": "fake"})
            emit({"type": "result", "subtype": "success", "is_error": True,
                  "result": f"You've hit your 5-hour limit · resets {clock}"})
        elif limit == "text":
            print(f"You've hit your 5-hour limit · resets in {step.get('reset_in', 1)}s", flush=True)
        elif limit == "legacy":
            print(f"Claude AI usage limit reached|{reset}", flush=True)
        elif limit == "unparseable":
            emit({"type": "result", "subtype": "success", "is_error": True, "result": "You've hit your usage limit"})

    status = step.get("status")
    if status:
        stories = os.path.join(folder, "stories")
        os.makedirs(stories, exist_ok=True)
        path = os.path.join(stories, f"{sid}-fake-story.md")
        with open(path, "w") as fh:
            fh.write(f"---\nstatus: {status}\n---\n\n# Story {sid}\n")
    if step.get("dirty"):
        with open(f"partial-{sid}-{n}.txt", "w") as fh:
            fh.write("half-done\n")
    if step.get("commit"):
        subprocess.run(["git", "add", "-A"], check=True)
        subprocess.run(["git", "commit", "-qm", f"feat: story {sid}"], check=True)
    if not limit:
        emit({"type": "result", "subtype": "success", "is_error": False, "result": f"story {sid}: {status}"})
    return int(step.get("exit", 1 if limit else 0))


if __name__ == "__main__":
    sys.exit(main())
