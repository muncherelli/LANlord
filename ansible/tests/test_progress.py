"""tests for callback_plugins/lanlord_progress.py with fake ansible objects; standard library
only. run from the repository root: python3 -m unittest discover -s ansible/tests"""
import contextlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
import unittest.mock

PLUGIN = os.path.join(
    os.path.dirname(__file__), "..", "playbooks", "callback_plugins", "lanlord_progress.py"
)
MARKER = "LANLORD_EVENT "


class CallbackBase:
    def __init__(self, *args, **kwargs):
        pass


class Loader:
    """stands in for ansible's DataLoader: files are looked up in a dict."""

    files = {}

    def load_from_file(self, path):
        if path not in self.files:
            raise IOError(path)
        return self.files[path]


def fake_ansible(cliargs):
    ansible = types.ModuleType("ansible")
    plugins = types.ModuleType("ansible.plugins")
    callback = types.ModuleType("ansible.plugins.callback")
    callback.CallbackBase = CallbackBase
    parsing = types.ModuleType("ansible.parsing")
    dataloader = types.ModuleType("ansible.parsing.dataloader")
    dataloader.DataLoader = Loader
    context = types.ModuleType("ansible.context")
    context.CLIARGS = cliargs
    ansible.context = context
    return {
        "ansible": ansible,
        "ansible.plugins": plugins,
        "ansible.plugins.callback": callback,
        "ansible.parsing": parsing,
        "ansible.parsing.dataloader": dataloader,
        "ansible.context": context,
    }


class Task:
    def __init__(self, name="t", action="command", keeps=True, broken=False):
        self.name = name
        self.action = action
        self.keeps = keeps
        self.broken = broken
        self.asked = None

    def get_name(self):
        return "  %s " % self.name

    def evaluate_tags(self, only, skip, variables):
        if self.broken:
            raise RuntimeError("no tags")
        self.asked = (only, skip)
        return self.keeps


class Block:
    def __init__(self, block=(), rescue=(), always=()):
        self.block = list(block)
        self.rescue = list(rescue)
        self.always = list(always)


class Play:
    def __init__(self, tasks, broken=False, gather_facts=False):
        self.tasks = tasks
        self.gather_facts = gather_facts
        self.broken = broken

    def get_tasks(self):
        if self.broken:
            raise RuntimeError("cannot load")
        return self.tasks


class Playbook:
    def __init__(self, plays, broken=False):
        self.plays = plays
        self.broken = broken

    def get_plays(self):
        if self.broken:
            raise RuntimeError("cannot load")
        return self.plays


class Result:
    def __init__(self, task, **data):
        self._task = task
        self._result = data


class Stats:
    def __init__(self, hosts):
        self.hosts = hosts
        self.processed = dict((h, 1) for h in hosts)

    def summarize(self, host):
        return self.hosts[host]


class ProgressTests(unittest.TestCase):
    def setUp(self):
        self.cliargs = {}
        patcher = unittest.mock.patch.dict(sys.modules, fake_ansible(self.cliargs))
        patcher.start()
        self.addCleanup(patcher.stop)
        spec = importlib.util.spec_from_file_location("lanlord_progress_under_test", PLUGIN)
        self.plugin = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.plugin)
        self.cb = self.plugin.CallbackModule()
        Loader.files = {}

    def events(self, call, *args, **kwargs):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            call(*args, **kwargs)
        lines = out.getvalue().splitlines()
        self.assertTrue(all(line.startswith(MARKER) for line in lines), lines)
        return [json.loads(line[len(MARKER):]) for line in lines]

    # counting

    def test_blocks_are_flattened_and_meta_tasks_do_not_count(self):
        tasks = [
            [
                Block(
                    block=[Task(), Task()],
                    rescue=[Task()],
                    always=[Task(), Task(action="meta")],
                ),
                Task(action="ansible.builtin.meta"),
                Task(),
            ]
        ]
        # rescue tasks only run after a failure, so they are not counted up front
        self.assertEqual(self.plugin.count_tasks(tasks), 4)
        self.assertEqual(self.plugin.count_tasks(None), 0)

    def test_flattened_rescue_tasks_are_found_through_their_parents(self):
        inner, outer, plain = Task(), Task(), Task()
        inner_block = Block(rescue=[inner])
        outer_block = Block(block=[], rescue=[inner_block])
        inner_block._parent = outer_block
        inner._parent = inner_block
        outer._parent = outer_block
        plain._parent = Block(block=[plain])
        self.assertTrue(self.plugin.in_rescue(inner))
        self.assertFalse(self.plugin.in_rescue(plain))
        self.assertEqual(self.plugin.count_tasks([[inner, plain]]), 1)

    def test_tasks_left_out_by_tags_are_not_counted(self):
        keep, drop = Task(keeps=True), Task(keeps=False)
        self.assertEqual(self.plugin.count_tasks([keep, drop], ["web"], []), 1)
        self.assertEqual(keep.asked, (["web"], []))
        self.assertEqual(self.plugin.count_tasks([drop], [], ["slow"]), 0)
        self.assertEqual(drop.asked, ([], ["slow"]))
        # no tag options at all: nobody is asked
        asked = Task(keeps=False)
        self.assertEqual(self.plugin.count_tasks([asked]), 1)
        self.assertIsNone(asked.asked)

    def test_a_task_whose_tags_cannot_be_evaluated_counts(self):
        self.assertTrue(self.plugin.runs(Task(broken=True), ["x"], []))

    def test_an_included_file_counts_its_tasks_and_blocks(self):
        Loader.files["inc.yml"] = [
            {"name": "a"},
            {"block": [{"name": "b"}, {"name": "c"}], "rescue": [{"name": "d"}]},
            {"always": [{"name": "e"}], "block": []},
            {"meta": "flush_handlers"},
            "not a task",
        ]
        self.assertEqual(self.plugin.count_file("inc.yml"), 4)

    def test_an_included_file_follows_the_tag_options(self):
        Loader.files["inc.yml"] = [
            {"name": "a", "tags": ["web"]},
            {"name": "b", "tags": "slow"},
            {"name": "c"},
            {"name": "d", "tags": ["web", "slow"]},
        ]
        count = self.plugin.count_file
        self.assertEqual(count("inc.yml", ["web"], []), 2)
        self.assertEqual(count("inc.yml", [], ["slow"]), 2)
        self.assertEqual(count("inc.yml", ["all"], ["slow"]), 2)
        self.assertEqual(count("inc.yml", [], []), 4)

    def test_fact_gathering_counts_unless_the_play_or_the_tags_leave_it_out(self):
        tasks = [[Task()]]
        total = self.plugin.CallbackModule()._play_total
        self.assertEqual(total(Play(tasks, gather_facts=True)), 2)
        self.assertEqual(total(Play(tasks, gather_facts=None)), 2)
        self.assertEqual(total(Play(tasks, gather_facts=False)), 1)
        self.assertEqual(total(Play(tasks, gather_facts="no")), 1)
        self.cliargs.update(tags=["web"])
        self.assertEqual(total(Play(tasks, gather_facts=True)), 2)
        self.cliargs.update(tags=[], skip_tags=["always"])
        self.assertEqual(total(Play(tasks, gather_facts=True)), 1)

    def test_an_unreadable_or_empty_include_counts_as_one(self):
        self.assertEqual(self.plugin.count_file("missing.yml"), 1)
        Loader.files["empty.yml"] = []
        self.assertEqual(self.plugin.count_file("empty.yml"), 1)
        Loader.files["none.yml"] = None
        self.assertEqual(self.plugin.count_file("none.yml"), 1)

    # events

    def test_the_total_is_announced_once_for_every_play_up_front(self):
        playbook = Playbook([Play([[Task(), Task()]]), Play([[Task()]])])
        self.assertEqual(
            self.events(self.cb.v2_playbook_on_start, playbook),
            [{"event": "total", "n": 3}],
        )
        # plays that were counted up front are not counted again
        self.assertEqual(self.events(self.cb.v2_playbook_on_play_start, playbook.plays[0]), [])
        self.assertEqual(self.events(self.cb.v2_playbook_on_play_start, playbook.plays[1]), [])
        # a play nobody knew about adds its own tasks
        extra = Play([[Task(), Task()]])
        self.assertEqual(
            self.events(self.cb.v2_playbook_on_play_start, extra),
            [{"event": "total", "n": 2}],
        )

    def test_tags_on_the_command_line_shrink_the_total(self):
        self.cliargs.update(tags=["web"], skip_tags=["slow"])
        keep, drop = Task(keeps=True), Task(keeps=False)
        out = self.events(self.cb.v2_playbook_on_start, Playbook([Play([[keep, drop]])]))
        self.assertEqual(out, [{"event": "total", "n": 1}])
        self.assertEqual(keep.asked, (["web"], ["slow"]))
        # --tags all is every task
        self.cliargs.update(tags=["all"], skip_tags=[])
        all_tasks = Task(keeps=False)
        out = self.events(self.cb.v2_playbook_on_start, Playbook([Play([[all_tasks]])]))
        self.assertEqual(out, [{"event": "total", "n": 1}])

    def test_a_playbook_that_cannot_be_read_announces_nothing(self):
        self.assertEqual(self.events(self.cb.v2_playbook_on_start, Playbook([], broken=True)), [])
        self.assertEqual(self.events(self.cb.v2_playbook_on_play_start, Play([], broken=True)), [])

    def test_cli_arguments_that_are_missing_mean_no_tags(self):
        del sys.modules["ansible.context"].CLIARGS
        self.assertEqual(self.plugin.CallbackModule._tags(), ([], []))

    def test_task_start_names_the_task_and_resets_the_items(self):
        self.cb._items = 4
        out = self.events(self.cb.v2_playbook_on_task_start, Task("write"), False)
        self.assertEqual(out, [{"event": "task_start", "name": "write"}])
        self.assertEqual(self.cb._items, 0)
        out = self.events(self.cb.v2_playbook_on_handler_task_start, Task("restart"))
        self.assertEqual(out, [{"event": "task_start", "name": "restart"}])

    def test_a_notified_handler_raises_the_total_once(self):
        handler = Task("restart")
        self.assertEqual(
            self.events(self.cb.v2_playbook_on_notify, handler, "localhost"),
            [{"event": "total", "n": 1}],
        )
        self.assertEqual(self.events(self.cb.v2_playbook_on_notify, handler, "localhost"), [])

    def test_an_include_raises_the_total_by_its_tasks(self):
        Loader.files["inc.yml"] = [{"name": "a"}, {"name": "b"}]
        included = types.SimpleNamespace(_filename="inc.yml")
        self.assertEqual(
            self.events(self.cb.v2_playbook_on_include, included),
            [{"event": "total", "n": 2}],
        )
        self.assertEqual(
            self.events(self.cb.v2_playbook_on_include, types.SimpleNamespace()),
            [{"event": "total", "n": 1}],
        )

    def test_results_become_task_done_events(self):
        task = Task("write")
        ok = self.events(self.cb.v2_runner_on_ok, Result(task, changed=True))
        self.assertEqual(
            ok,
            [{"event": "task_done", "name": "write", "status": "ok", "changed": True, "msg": ""}],
        )
        skipped = self.events(self.cb.v2_runner_on_skipped, Result(task))
        self.assertEqual(skipped[0]["status"], "skipped")
        self.assertFalse(skipped[0]["changed"])

    def test_failures_carry_their_message_cut_to_a_sane_length(self):
        task = Task("write")
        failed = self.events(self.cb.v2_runner_on_failed, Result(task, msg="x" * 900))
        self.assertEqual(failed[0]["status"], "failed")
        self.assertEqual(len(failed[0]["msg"]), 500)
        err = self.events(self.cb.v2_runner_on_failed, Result(task, stderr="no space"))
        self.assertEqual(err[0]["msg"], "no space")
        down = self.events(self.cb.v2_runner_on_unreachable, Result(task, msg="no route"))
        self.assertEqual((down[0]["status"], down[0]["msg"]), ("unreachable", "no route"))

    def test_a_task_with_no_log_never_leaks_its_message(self):
        result = Result(Task("secret"), msg="password is hunter2", _ansible_no_log=True)
        out = self.events(self.cb.v2_runner_on_failed, result)
        self.assertEqual(out[0]["msg"], "")
        debug = Result(Task("show", action="debug"), msg="hunter2", _ansible_no_log=True)
        self.assertEqual([e["event"] for e in self.events(self.cb.v2_runner_on_ok, debug)], ["task_done"])

    def test_the_debug_module_logs_what_it_said(self):
        out = self.events(self.cb.v2_runner_on_ok, Result(Task("show", action="debug"), msg="hi"))
        self.assertEqual([e["event"] for e in out], ["log", "task_done"])
        self.assertEqual(out[0]["text"], "hi")
        quiet = self.events(self.cb.v2_runner_on_ok, Result(Task("show", action="debug")))
        self.assertEqual([e["event"] for e in quiet], ["task_done"])

    def test_a_result_with_no_data_is_still_an_event(self):
        result = Result(Task("t"))
        result._result = None
        self.assertEqual(self.events(self.cb.v2_runner_on_ok, result)[0]["status"], "ok")

    def test_items_count_up_whatever_their_outcome_and_never_down(self):
        self.cb._items = 0
        seen = []
        for call in (
            self.cb.v2_runner_item_on_ok,
            self.cb.v2_runner_item_on_failed,
            self.cb.v2_runner_item_on_skipped,
        ):
            seen.extend(e["n"] for e in self.events(call, Result(Task())))
        self.assertEqual(seen, [1, 2, 3])
        self.assertEqual(seen, sorted(seen))

    def test_totals_only_ever_add_to_the_count(self):
        # every total event is a positive amount, so the sum the front end keeps cannot fall
        Loader.files["inc.yml"] = [{"name": "a"}]
        amounts = []
        amounts += [e["n"] for e in self.events(self.cb.v2_playbook_on_start, Playbook([Play([[Task()]])]))]
        amounts += [e["n"] for e in self.events(self.cb.v2_playbook_on_notify, Task("h"), "x")]
        amounts += [
            e["n"]
            for e in self.events(
                self.cb.v2_playbook_on_include, types.SimpleNamespace(_filename="inc.yml")
            )
        ]
        self.assertTrue(amounts and all(n > 0 for n in amounts))

    def test_the_stats_add_up_over_hosts_and_rename_two_keys(self):
        stats = Stats(
            {
                "b": {"ok": 2, "changed": 1, "failures": 1, "skipped": 0, "unreachable": 0},
                "a": {"ok": 3, "changed": 0, "failures": 0, "skipped": 2, "unreachable": 1},
            }
        )
        out = self.events(self.cb.v2_playbook_on_stats, stats)
        self.assertEqual(
            out,
            [
                {
                    "event": "finished",
                    "stats": {"ok": 5, "changed": 1, "failed": 1, "skipped": 2, "unreachable": 1},
                }
            ],
        )


SMOKE_PLAY = """\
- hosts: localhost
  connection: local
  gather_facts: true
  tasks:
    - name: first
      debug: msg=hi
    - name: slow one
      debug: msg=slow
      tags: slow
    - block:
        - name: in block
          debug: msg=b
      rescue:
        - name: in rescue
          debug: msg=r
    - name: include it
      include_tasks: inc.yml
    - name: last
      debug: msg=l
"""

SMOKE_INCLUDE = """\
- name: inc one
  debug: msg=one
- name: inc two
  debug: msg=two
- name: inc slow
  debug: msg=slow
  tags: slow
"""


@unittest.skipUnless(shutil.which("ansible-playbook"), "ansible-playbook is not installed")
class AnsibleSmokeTests(unittest.TestCase):
    """the plugin inside a real ansible-playbook run on this machine."""

    def run_play(self, *extra):
        with tempfile.TemporaryDirectory() as tmp:
            for name, text in (("play.yml", SMOKE_PLAY), ("inc.yml", SMOKE_INCLUDE)):
                with open(os.path.join(tmp, name), "w") as handle:
                    handle.write(text)
            env = dict(
                os.environ,
                LC_ALL="C.UTF-8",
                ANSIBLE_STDOUT_CALLBACK="lanlord_progress",
                ANSIBLE_CALLBACK_PLUGINS=os.path.dirname(os.path.abspath(PLUGIN)),
                ANSIBLE_LOCALHOST_WARNING="false",
                PYTHONDONTWRITEBYTECODE="1",
            )
            done = subprocess.run(
                ["ansible-playbook", "-i", "localhost,", *extra, "play.yml"],
                cwd=tmp, env=env, capture_output=True, text=True, timeout=300,
            )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        self.assertTrue(lines and all(line.startswith(MARKER) for line in lines), lines)
        return [json.loads(line[len(MARKER):]) for line in lines]

    def check(self, events, ran):
        names = [e["name"] for e in events if e["event"] == "task_done"]
        self.assertEqual(sorted(names), sorted(ran))
        self.assertEqual(events[-1]["event"], "finished")
        self.assertEqual(events[-1]["stats"]["failed"], 0)
        # the front end's sums, as the reducer in lanlord-model keeps them: the total grows, the
        # shown percentage is the best seen, and 100 is only shown once the run is over
        total = done = shown = 0
        for event in events:
            if event["event"] == "total":
                total += event["n"]
            elif event["event"] == "task_done":
                done += 1
            total = max(total, done)
            if event["event"] == "finished":
                now = 100
            else:
                now = min(done * 100 // total, 99) if total else 0
                self.assertLess(shown, 100)
            shown = max(shown, now)
        self.assertEqual(total, len(ran), "the totals match the tasks that ran")
        self.assertEqual(shown, 100)

    def test_a_clean_run_counts_what_runs_and_ends_at_100(self):
        events = self.run_play("--skip-tags", "slow")
        self.check(
            events,
            ["Gathering Facts", "first", "in block", "include it", "inc one", "inc two", "last"],
        )

    def test_without_a_filter_the_tagged_tasks_count_too(self):
        events = self.run_play()
        self.check(
            events,
            ["Gathering Facts", "first", "slow one", "in block", "include it",
             "inc one", "inc two", "inc slow", "last"],
        )

    def test_with_tags_only_the_tagged_tasks_and_fact_gathering_count(self):
        events = self.run_play("--tags", "slow")
        self.check(events, ["Gathering Facts", "slow one"])


if __name__ == "__main__":
    unittest.main()
