# stdout callback for lanlord: one json line per event, so the apply can show progress.
# every line starts with LANLORD_EVENT; anything else on stdout is ordinary ansible output.
# plain python 3 against ansible-core; no collection needed.
from __future__ import absolute_import, division, print_function

__metaclass__ = type

import json
import sys

from ansible.plugins.callback import CallbackBase

DOCUMENTATION = """
    name: lanlord_progress
    type: stdout
    short_description: json progress events for lanlord
    description:
      - prints the number of tasks, each task start and end, and the final stats as json lines.
"""

MARKER = "LANLORD_EVENT "


def in_rescue(task):
    """is the task in the rescue section of a block, however deep? play.get_tasks() flattens
    blocks, so this is read off the task's parents."""
    child = task
    parent = getattr(task, "_parent", None)
    while parent is not None:
        if any(t is child for t in (getattr(parent, "rescue", None) or [])):
            return True
        child = parent
        parent = getattr(parent, "_parent", None)
    return False


def runs(task, only_tags, skip_tags):
    """does this one task count: not meta, not in a rescue section (that runs only after a
    failure), and not left out by --tags or --skip-tags?"""
    if getattr(task, "action", "") in ("meta", "ansible.builtin.meta"):
        return False
    if in_rescue(task):
        return False
    if not (only_tags or skip_tags):
        return True
    try:
        return bool(task.evaluate_tags(only_tags or [], skip_tags or [], {}))
    except Exception:
        return True


def count_block(block, only_tags, skip_tags):
    """tasks in a block and its always section; rescue tasks only run after a failure, so
    they are left to the front end's total = max(total, done)."""
    n = 0
    for part in ("block", "always"):
        n += count_tasks(getattr(block, part, None), only_tags, skip_tags)
    return n


def count_tasks(blocks, only_tags=None, skip_tags=None):
    """tasks in `blocks`, flattened: blocks count by what is in them, meta tasks not at all,
    and tasks that --tags or --skip-tags leave out never run, so they are not counted."""
    n = 0
    for item in blocks or []:
        if isinstance(item, list):
            n += count_tasks(item, only_tags, skip_tags)
        elif item.__class__.__name__ == "Block":
            n += count_block(item, only_tags, skip_tags)
        elif runs(item, only_tags, skip_tags):
            n += 1
    return n


def entry_runs(entry, only_tags, skip_tags):
    """does this plain entry of an included file count: not meta, not left out by tags?"""
    for key in ("meta", "ansible.builtin.meta"):
        if key in entry:
            return False
    if not (only_tags or skip_tags):
        return True
    tags = entry.get("tags") or []
    if not isinstance(tags, list):
        tags = [tags]
    tags = [str(t) for t in tags]
    if any(t in skip_tags for t in tags):
        return False
    if only_tags and "all" not in only_tags:
        return any(t in only_tags for t in tags)
    return True


def walk_entries(entries, only_tags, skip_tags):
    total = 0
    for entry in entries or []:
        if not isinstance(entry, dict):
            continue
        if "block" in entry:
            total += walk_entries(entry.get("block"), only_tags, skip_tags)
            total += walk_entries(entry.get("always"), only_tags, skip_tags)
        elif entry_runs(entry, only_tags, skip_tags):
            total += 1
    return total


def count_file(path, only_tags=None, skip_tags=None):
    """tasks in an included file, read plainly and filtered like the others; 1 when it cannot
    be read."""
    try:
        from ansible.parsing.dataloader import DataLoader

        entries = DataLoader().load_from_file(path)
        return walk_entries(entries, only_tags or [], skip_tags or []) or 1
    except Exception:
        return 1


class CallbackModule(CallbackBase):
    CALLBACK_VERSION = 2.0
    CALLBACK_TYPE = "stdout"
    CALLBACK_NAME = "lanlord_progress"

    def __init__(self, *args, **kwargs):
        super(CallbackModule, self).__init__(*args, **kwargs)
        self._plays = 0
        self._counted = 0
        self._handlers = set()
        self._items = 0
        self._stats_seen = False

    def _emit(self, event, **fields):
        fields["event"] = event
        sys.stdout.write(MARKER + json.dumps(fields, sort_keys=True) + "\n")
        sys.stdout.flush()

    @staticmethod
    def _tags():
        try:
            from ansible import context

            return (
                list(context.CLIARGS.get("tags") or []),
                list(context.CLIARGS.get("skip_tags") or []),
            )
        except Exception:
            return [], []

    def _play_total(self, play):
        only, skip = self._tags()
        if only == ["all"]:
            only = []
        try:
            total = count_tasks(play.get_tasks(), only, skip)
        except Exception:
            return 0
        # the implicit fact gathering is a task too, but get_tasks() does not list it; it is
        # tagged always, so only --skip-tags always leaves it out
        if self._gathers(play) and "always" not in skip:
            total += 1
        return total

    @staticmethod
    def _gathers(play):
        # gather_facts is on unless the play says no; "smart" and unset gather
        value = getattr(play, "gather_facts", None)
        return value is None or value not in (False, "no", "false", "False")

    def v2_playbook_on_start(self, playbook):
        # every play of the playbook is known now, so the total is too
        total = 0
        try:
            plays = playbook.get_plays()
        except Exception:
            plays = []
        for play in plays:
            total += self._play_total(play)
        self._counted = len(plays)
        if total:
            self._emit("total", n=total)

    def v2_playbook_on_play_start(self, play):
        self._plays += 1
        if self._plays > self._counted:
            total = self._play_total(play)
            if total:
                self._emit("total", n=total)

    def _start(self, task):
        self._items = 0
        self._emit("task_start", name=task.get_name().strip())

    def v2_playbook_on_task_start(self, task, is_conditional):
        self._start(task)

    def v2_playbook_on_handler_task_start(self, task):
        self._start(task)

    def v2_playbook_on_notify(self, handler, host):
        # a handler that was not counted up front shows up when it is notified
        name = handler.get_name()
        if name not in self._handlers:
            self._handlers.add(name)
            self._emit("total", n=1)

    def v2_playbook_on_include(self, included_file):
        only, skip = self._tags()
        self._emit("total", n=count_file(getattr(included_file, "_filename", ""), only, skip))

    def _done(self, result, status, ignore_errors=False):
        task = result._task
        data = result._result or {}
        msg = ""
        if status in ("failed", "unreachable") and not data.get("_ansible_no_log"):
            msg = str(data.get("msg") or data.get("stderr") or "")[:500]
        if task.action in ("debug", "ansible.builtin.debug") and not data.get("_ansible_no_log"):
            if data.get("msg"):
                self._emit("log", text=str(data["msg"])[:500])
        self._emit(
            "task_done",
            name=task.get_name().strip(),
            status=status,
            changed=bool(data.get("changed")),
            msg=msg,
        )

    def v2_runner_on_ok(self, result, **kwargs):
        self._done(result, "ok")

    def v2_runner_on_failed(self, result, ignore_errors=False, **kwargs):
        self._done(result, "failed")

    def v2_runner_on_skipped(self, result, **kwargs):
        self._done(result, "skipped")

    def v2_runner_on_unreachable(self, result, **kwargs):
        self._done(result, "unreachable")

    def _item(self, result):
        self._items += 1
        self._emit("item", n=self._items)

    def v2_runner_item_on_ok(self, result):
        self._item(result)

    def v2_runner_item_on_failed(self, result):
        self._item(result)

    def v2_runner_item_on_skipped(self, result):
        self._item(result)

    def v2_playbook_on_stats(self, stats):
        totals = {"ok": 0, "changed": 0, "failed": 0, "skipped": 0, "unreachable": 0}
        for host in sorted(stats.processed.keys()):
            summary = stats.summarize(host)
            # ansible names two of them differently
            for key, name in (("ok", "ok"), ("changed", "changed"), ("failed", "failures"),
                              ("skipped", "skipped"), ("unreachable", "unreachable")):
                totals[key] += summary.get(name, 0)
        self._emit("finished", stats=totals)
