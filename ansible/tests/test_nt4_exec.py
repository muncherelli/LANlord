# run with: python3 -m unittest discover -s ansible/tests
import base64
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
import unittest

IMAGE = os.path.join(os.path.dirname(__file__), "..", "playbooks", "nt4-domain", "image")
EXEC = os.path.join(IMAGE, "lanlord-exec")
SECRET = "S3cret-pw!x"

# smbclient, net and nmblookup are stubs that record their arguments. the "machine" is a folder:
# put and get move files in and out of it, and starting the service plays the .bat's part by
# writing .out, .err and .rc from the scenario files (out, err, rc, hang, start_error, put_error)
STUB = r"""#!/bin/bash
name=$(basename "$0")
echo "$name $*" >> "$STUBS/calls"
args=("$@")
for ((k = 0; k < ${#args[@]}; k++)); do
  if [[ ${args[k]} == -A ]]; then
    echo "authmode=$(stat -c %a "${args[k+1]}")" >> "$STUBS/calls"
    cat "${args[k+1]}" >> "$STUBS/auth-seen"
  fi
done
remote="$STUBS/remote"
case $name in
  nmblookup)
    [[ -e $STUBS/no_wins ]] && { echo "name_query failed"; exit 1; }
    echo "querying $1 on 192.0.2.255"
    echo "192.0.2.20 $1<00>" ;;
  smbclient)
    [[ -e $STUBS/put_error ]] && [[ $* == *put* ]] && { if [[ -s $STUBS/put_error ]]; then cat "$STUBS/put_error"; else printf 'NT_STATUS_ACCESS_DENIED \001 "x" \\ \a\n'; fi; exit 1; }
    cmds=
    for ((k = 0; k < ${#args[@]}; k++)); do [[ ${args[k]} == -c ]] && cmds=${args[k+1]}; done
    IFS=';' read -ra list <<< "$cmds"
    for c in "${list[@]}"; do
      read -ra w <<< "$c"
      case ${w[0]:-} in
        put) cp "${w[1]}" "$remote/${w[2]}"; cp "${w[1]}" "$STUBS/bats/${w[2]}" ;;
        get)
          # in the slow scenario the poll waits until the start stub has written its pids, so the kill
          # on timeout can never come first
          if [[ -e $STUBS/slow ]]; then n=0; while [[ ! -e $STUBS/ready ]] && ((n++ < 200)); do sleep 0.05; done; fi
          [[ -f $remote/${w[1]} ]] && cp "$remote/${w[1]}" "${w[2]}" ;;
        del) [[ -e $STUBS/del_hang && ${w[1]} != *\** ]] && sleep 30; rm -f $remote/${w[1]} ;;
      esac
    done
    exit 0 ;;
  net)
    verb=$3
    case $verb in
      list) cat "$STUBS/services" ;;
      create)
        [[ -e $STUBS/create_io_timeout ]] && { echo "Create service request failed. [NT_STATUS_IO_TIMEOUT]"; exit 2; }
        echo "$4" >> "$STUBS/services" ;;
      delete) sed -i "/^$4\$/d" "$STUBS/services" ;;
      start)
        [[ -e $STUBS/start_error ]] && { echo "WERR_ACCESS_DENIED"; exit 1; }
        [[ -e $STUBS/start_io_timeout ]] && io_timeout=1
        [[ -e $STUBS/slow ]] && { echo $$ > "$STUBS/startpid"; sleep 30 & echo $! > "$STUBS/sleeppid"; touch "$STUBS/ready"; wait; }
        [[ -e $STUBS/hang ]] || {
          [[ -e $STUBS/out ]] && cp "$STUBS/out" "$remote/$(ls "$STUBS/bats" | head -1 | sed 's/.bat$/.out/')"
          [[ -e $STUBS/err ]] && cp "$STUBS/err" "$remote/$(ls "$STUBS/bats" | head -1 | sed 's/.bat$/.err/')"
          rcname=$(ls "$STUBS/bats" | head -1 | sed 's/.bat$/.rc/')
          if [[ -e $STUBS/late ]]; then
            ( sleep 1.5; cp "$STUBS/rc" "$remote/$rcname" ) >/dev/null 2>&1 &
          else
            cp "$STUBS/rc" "$remote/$rcname"
          fi
        }
        [[ -n ${io_timeout:-} ]] && { echo "Query status request failed. [NT_STATUS_IO_TIMEOUT]"; exit 2; }
        echo "WERR_SERVICE_REQUEST_TIMEOUT"; exit 1 ;;
    esac ;;
esac
exit 0
"""


class LanlordExecTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.stubs = os.path.join(self.tmp, "stubs")
        self.scratch = os.path.join(self.tmp, "scratch")
        self.locks = os.path.join(self.tmp, "locks")
        for d in ("bin", "remote", "bats"):
            os.makedirs(os.path.join(self.stubs, d))
        os.makedirs(self.scratch)
        for name in ("calls", "services", "auth-seen"):
            open(os.path.join(self.stubs, name), "w").close()
        for command in ("smbclient", "net", "nmblookup"):
            path = os.path.join(self.stubs, "bin", command)
            with open(path, "w") as f:
                f.write(STUB)
            os.chmod(path, 0o755)
        self.env = dict(
            os.environ,
            PATH=os.path.join(self.stubs, "bin") + ":" + os.environ["PATH"],
            STUBS=self.stubs,
            LANLORD_EXEC_DIR=self.locks,
            LANLORD_EXEC_TMP=self.scratch,
            LANLORD_EXEC_POLL="0.1",
            LANLORD_EXEC_LOG=os.path.join(self.tmp, "exec.log"),
        )
        self.log = self.env["LANLORD_EXEC_LOG"]
        self.scenario(rc="0\r\n")

    def scenario(self, **files):
        for name, content in files.items():
            with open(os.path.join(self.stubs, name), "w") as f:
                f.write(content)

    def flag(self, name):
        open(os.path.join(self.stubs, name), "w").close()

    def run_exec(self, command="ver", args=("--machine", "nt4srv01", "--timeout", "5"), mode="run", **extra):
        request = {"user": "DEMO\\Administrator", "password": SECRET, "command": command}
        request.update(extra)
        return subprocess.run(
            ["bash", EXEC, mode, *args],
            input=json.dumps(request),
            text=True,
            capture_output=True,
            env=self.env,
        )

    def events(self, out):
        events = []
        for line in out.stdout.splitlines():
            self.assertTrue(line.startswith("LANLORD_EVENT "), line)
            events.append(json.loads(line[len("LANLORD_EVENT "):]))
        return events

    def calls(self):
        with open(os.path.join(self.stubs, "calls")) as f:
            return f.read()

    def bat(self, suffix=".bat"):
        names = [n for n in os.listdir(os.path.join(self.stubs, "bats")) if n.endswith(suffix)]
        if suffix == ".bat":
            names = [n for n in names if not n.endswith(".cmd.bat")]
        self.assertEqual(len(names), 1)
        with open(os.path.join(self.stubs, "bats", names[0]), "rb") as f:
            return f.read().decode()

    def remote(self):
        return os.listdir(os.path.join(self.stubs, "remote"))

    def services(self):
        with open(os.path.join(self.stubs, "services")) as f:
            return f.read().split()

    def assert_clean(self):
        self.assertEqual(self.services(), [])
        self.assertEqual(self.remote(), [])
        self.assertEqual(os.listdir(self.scratch), [])

    def assert_no_secret(self, out):
        self.assertNotIn(SECRET, self.calls())
        self.assertNotIn(SECRET, out.stdout)
        self.assertNotIn(SECRET, out.stderr)

    def log_lines(self):
        with open(self.log) as f:
            return [line.rstrip("\n").split("\t") for line in f]

    def test_script_parses(self):
        subprocess.run(["bash", "-n", EXEC], check=True)

    def test_a_command_runs_and_its_output_comes_back(self):
        self.scenario(out="Windows NT Version 4.0\r\n", err="oops\r\n", rc="3 \r\n")
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        events = self.events(out)
        got = {"out": b"", "err": b""}
        for e in events:
            if "stream" in e:
                got[e["stream"]] += base64.b64decode(e["data"])
        self.assertEqual(got["out"], b"Windows NT Version 4.0\r\n")
        self.assertEqual(got["err"], b"oops\r\n")
        self.assertEqual(events[-1], {"exit": 3})
        self.assert_clean()
        self.assert_no_secret(out)
        calls = self.calls()
        self.assertIn("nmblookup -- NT4SRV01", calls)
        self.assertIn("//192.0.2.20/ADMIN$", calls)
        self.assertIn("client min protocol=NT1", calls)
        self.assertRegex(calls, r"net rpc service create LANlordExec[0-9a-f]{8} LANlord exec")
        self.assertIn("cmd.exe /Q /c %SystemRoot%\\Temp\\ll-", calls)

    def test_output_is_streamed_as_it_grows(self):
        self.scenario(out="line one\r\n", rc="0\r\n")
        self.flag("late")
        out = self.run_exec()
        streams = [e for e in self.events(out) if e.get("stream") == "out"]
        self.assertEqual(b"".join(base64.b64decode(e["data"]) for e in streams), b"line one\r\n")
        self.assertEqual(len(streams), 1)

    def test_the_bat_holds_the_command_as_typed_and_redirects_a_chain_as_a_whole(self):
        out = self.run_exec(command="dir %SystemRoot%\\system32 & for %%i in (a b) do echo %%i")
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        outer = self.bat()
        inner = self.bat(".cmd.bat")
        run_id = re.search(r"ll-([0-9a-f]{8})\.cmd\.bat", outer).group(1)
        self.assertEqual(inner, "@echo off\r\ndir %SystemRoot%\\system32 & for %%i in (a b) do echo %%i\r\n")
        self.assertEqual(
            outer,
            "@echo off\r\n"
            "call %SystemRoot%\\Temp\\ll-" + run_id + ".cmd.bat > %SystemRoot%\\Temp\\ll-" + run_id + ".out"
            " 2> %SystemRoot%\\Temp\\ll-" + run_id + ".err\r\n"
            "echo %ERRORLEVEL% > %SystemRoot%\\Temp\\ll-" + run_id + ".rc\r\n",
        )
        self.assert_clean()

    def test_an_explicit_address_skips_the_name_lookup(self):
        out = self.run_exec(args=("--machine", "nt4srv01", "--address", "192.0.2.77", "--timeout", "5"))
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertNotIn("nmblookup", self.calls())
        self.assertIn("//192.0.2.77/ADMIN$", self.calls())

    def test_the_password_is_only_in_a_0600_file_that_is_gone_afterwards(self):
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        calls = self.calls()
        modes = re.findall(r"authmode=(\d+)", calls)
        self.assertTrue(modes)
        self.assertEqual(set(modes), {"600"})
        with open(os.path.join(self.stubs, "auth-seen")) as f:
            seen = f.read()
        self.assertIn("domain=DEMO\n", seen)
        self.assertIn("username=Administrator\n", seen)
        self.assertIn("password=" + SECRET + "\n", seen)
        self.assert_no_secret(out)
        self.assertEqual(os.listdir(self.scratch), [])

    def test_hostile_commands_are_refused_before_anything_is_sent(self):
        for command in ("ver\r\ndel x", "ver\ndel x", "ver\x00", "", "a" * 2001):
            out = self.run_exec(command=command)
            events = self.events(out)
            self.assertEqual(len(events), 1, command)
            self.assertIn("error", events[0], command)
            self.assertEqual(self.calls(), "", command)
            self.assertEqual(os.listdir(self.scratch), [])
        # a command at the cap is fine
        out = self.run_exec(command="a" * 2000)
        self.assertEqual(self.events(out)[-1], {"exit": 0})

    def test_a_raw_nul_in_the_request_is_refused(self):
        out = subprocess.run(
            ["bash", EXEC, "run", "--machine", "nt4srv01", "--timeout", "5"],
            input=b'{"user":"a","password":"b","command":"ver\x00"}',
            capture_output=True,
            env=self.env,
        )
        self.assertIn(b'"error"', out.stdout)
        self.assertEqual(self.calls(), "")

    def test_a_failed_start_cleans_up(self):
        self.flag("start_error")
        out = self.run_exec()
        events = self.events(out)
        self.assertEqual(out.returncode, 1)
        self.assertIn("could not start the service", events[-1]["error"])
        self.assertEqual(self.events(out)[-1].keys(), {"error"})
        self.assert_clean()
        self.assertIn("net rpc service delete LANlordExec", self.calls())
        self.assert_no_secret(out)

    def test_every_tool_call_turns_spnego_off_and_keeps_ntlmv2(self):
        self.run_exec()
        calls = [c for c in self.calls().splitlines() if c.startswith(("smbclient", "net "))]
        self.assertTrue(any(c.startswith("smbclient") for c in calls))
        self.assertTrue(any(c.startswith("net rpc service start") for c in calls))
        for c in calls:
            self.assertIn("client use spnego=no", c)
            self.assertNotIn("lanman", c)
            self.assertNotIn("plaintext", c)

    def test_every_tool_call_does_not_require_ipc_signing(self):
        self.run_exec()
        calls = [c for c in self.calls().splitlines() if c.startswith(("smbclient", "net "))]
        self.assertTrue(calls)
        for c in calls:
            self.assertIn("client ipc signing=if_required", c)
            self.assertNotIn("client signing", c)

    def test_a_start_that_ends_in_io_timeout_still_waits_for_the_rc(self):
        self.flag("start_io_timeout")
        self.flag("late")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assert_clean()

    def test_a_busy_service_manager_is_an_actionable_error(self):
        self.flag("create_io_timeout")
        out = self.run_exec()
        error = self.events(out)[-1]["error"]
        self.assertEqual(out.returncode, 1)
        self.assertIn("service manager on nt4srv01 is busy", error)
        self.assertIn("try again in a minute", error)
        self.assertNotIn("could not create", error)
        self.assertEqual(self.calls().count("net rpc service create"), 1)

    def test_a_copy_error_shows_the_real_line_not_the_warnings(self):
        warnings = "".join(
            'lpcfg_do_global_parameter: WARNING: The "x%d" option is deprecated\n' % i for i in range(12)
        )
        self.scenario(
            put_error=warnings
            + "\nServer does not support EXTENDED_SECURITY but 'client use spnego = yes' is set\n"
            + "session setup failed: NT_STATUS_LOGON_FAILURE\n"
        )
        out = self.run_exec()
        error = self.events(out)[-1]["error"]
        self.assertIn("could not copy", error)
        self.assertIn("NT_STATUS_LOGON_FAILURE", error)
        self.assertNotIn("lpcfg_do_global_parameter", error)

    def test_a_copy_failure_is_audited_even_when_the_cleanup_is_cut_short(self):
        # the cleanup's delete hangs and the run is terminated meanwhile: the line must already be there
        self.flag("put_error")
        self.flag("del_hang")
        proc = subprocess.Popen(
            ["bash", EXEC, "run", "--machine", "nt4srv01", "--timeout", "5"],
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
            env=self.env,
        )
        proc.stdin.write(json.dumps({"user": "DEMO\\Administrator", "password": SECRET, "command": "ver"}))
        proc.stdin.close()
        for _ in range(100):
            if os.path.exists(self.log):
                break
            time.sleep(0.1)
        proc.terminate()
        proc.wait(timeout=10)
        self.assertEqual(self.log_lines()[-1][5], "error")

    def test_a_failed_copy_cleans_up(self):
        self.flag("put_error")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("could not copy", self.events(out)[-1]["error"])
        self.assertNotIn("service create", self.calls())
        self.assert_clean()

    def test_the_start_process_group_is_killed_on_timeout(self):
        self.flag("hang")
        self.flag("slow")
        out = self.run_exec(args=("--machine", "nt4srv01", "--timeout", "1"))
        self.assertEqual(self.events(out)[-1], {"timed_out": True})
        for name in ("startpid", "sleeppid"):
            with open(os.path.join(self.stubs, name)) as f:
                pid = int(f.read())
            for _ in range(40):
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                os.kill(pid, 9)
                self.fail(name + " survived")
        self.assert_clean()

    def test_a_non_numeric_exit_code_is_an_error(self):
        self.scenario(rc="garbage\r\n")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("holds no number", self.events(out)[-1]["error"])
        self.assert_clean()

    def test_a_timeout_cleans_up(self):
        self.flag("hang")
        out = self.run_exec(args=("--machine", "nt4srv01", "--timeout", "1"))
        self.assertEqual(out.returncode, 0)
        self.assertEqual(self.events(out)[-1], {"timed_out": True})
        self.assert_clean()
        self.assert_no_secret(out)

    def test_option_looking_names_and_addresses_are_refused(self):
        for args in (("--machine", "-d10", "--timeout", "5"), ("--machine", "-", "--timeout", "5"),
                     ("--machine", "ws1", "--address", "-x", "--timeout", "5"),
                     ("--machine", "ws1", "--address", "999.1.1.1", "--timeout", "5"),
                     ("--machine", "ws1", "--address", "192.0.2.1/x", "--timeout", "5"),
                     ("--machine", "ws1", "--timeout", "901")):
            out = self.run_exec(args=args)
            self.assertIn("error", self.events(out)[-1], args)
        self.assertEqual(self.calls(), "")

    def test_the_request_must_be_a_flat_object_of_strings(self):
        bodies = ('{"user":"a","password":"b","command":"ver","n":1}',
                  '{"user":"a","password":"b","command":{"x":"ver"}}',
                  '{"user":"a","password":"b","command":"ver"} junk',
                  '{"user":"a","password":"b","command":"ver"}{"command":"x"}')
        for body in bodies:
            out = subprocess.run(["bash", EXEC, "run", "--machine", "ws1", "--timeout", "5"],
                                 input=body, text=True, capture_output=True, env=self.env)
            self.assertIn("error", self.events(out)[-1], body)
        self.assertEqual(self.calls(), "")

    def test_a_stale_scratch_folder_goes_and_a_fresh_one_stays(self):
        old = os.path.join(self.scratch, "lanlord-exec.old123")
        fresh = os.path.join(self.scratch, "lanlord-exec.new123")
        for d in (old, fresh):
            os.makedirs(d)
            with open(os.path.join(d, "auth"), "w") as f:
                f.write("password=" + SECRET)
        past = time.time() - 3600
        os.utime(old, (past, past))
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertFalse(os.path.exists(old))
        self.assertTrue(os.path.exists(fresh))

    def test_an_unknown_name_is_an_error(self):
        self.flag("no_wins")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("does not answer", self.events(out)[-1]["error"])
        self.assertEqual(os.listdir(self.scratch), [])

    def test_bad_arguments_and_requests(self):
        for args in (("--machine", "a b", "--timeout", "5"), ("--machine", "x;y", "--timeout", "5"),
                     ("--machine", "ws1", "--address", "host", "--timeout", "5"),
                     ("--machine", "ws1", "--timeout", "0"), ("--timeout", "5")):
            out = self.run_exec(args=args)
            self.assertIn("error", self.events(out)[-1], args)
        out = self.run_exec(password="")
        self.assertIn("error", self.events(out)[-1])
        self.assertEqual(self.calls(), "")

    def test_stale_services_are_swept_at_the_start_of_a_run(self):
        self.scenario(services="LANlordExecdeadbeef\nOtherService\n")
        with open(os.path.join(self.stubs, "remote", "ll-deadbeef.bat"), "w") as f:
            f.write("x")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertIn("net rpc service delete LANlordExecdeadbeef", self.calls())
        self.assertNotIn("delete OtherService", self.calls())
        self.assertEqual(self.services(), ["OtherService"])
        self.assertEqual(self.remote(), [])

    def test_sweep_removes_what_a_dead_run_left(self):
        self.scenario(services="LANlordExec0123abcd\nLANlordExecffffffff\nSpooler\n")
        with open(os.path.join(self.stubs, "remote", "ll-0123abcd.out"), "w") as f:
            f.write("x")
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.events(out), [{"swept": 2}])
        self.assertEqual(self.services(), ["Spooler"])
        self.assertEqual(self.remote(), [])
        self.assertNotIn("service create", self.calls())
        self.assert_no_secret(out)
        self.assertEqual(os.listdir(self.scratch), [])

    def test_one_run_per_address(self):
        os.makedirs(self.locks)
        holder = subprocess.Popen(["flock", os.path.join(self.locks, "addr-192.0.2.20.lock"), "sleep", "5"])
        self.addCleanup(holder.wait)
        self.addCleanup(holder.kill)
        for _ in range(50):
            probe = subprocess.run(["flock", "-n", os.path.join(self.locks, "addr-192.0.2.20.lock"), "true"])
            if probe.returncode != 0:
                break
            time.sleep(0.05)
        # a different name that resolves to the same address is held up, and so is sweep
        for mode in ("run", "sweep"):
            out = self.run_exec(mode=mode, args=("--machine", "nt4srv04", "--timeout", "5")
                                if mode == "run" else ("--machine", "nt4srv04"))
            self.assertIn("already running", self.events(out)[-1]["error"], mode)
        self.assertNotIn("service", self.calls())

    def test_one_run_per_machine(self):
        os.makedirs(self.locks)
        holder = subprocess.Popen(
            ["flock", os.path.join(self.locks, "NT4SRV01.lock"), "sleep", "5"]
        )
        self.addCleanup(holder.wait)
        self.addCleanup(holder.kill)
        # wait for the holder to take the lock
        for _ in range(50):
            probe = subprocess.run(["flock", "-n", os.path.join(self.locks, "NT4SRV01.lock"), "true"])
            if probe.returncode != 0:
                break
            subprocess.run(["sleep", "0.05"])
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("already running", self.events(out)[-1]["error"])
        self.assertEqual(self.calls(), "")
        # another machine is not held up
        other = self.run_exec(args=("--machine", "nt4srv02", "--timeout", "5"))
        self.assertEqual(self.events(other)[-1], {"exit": 0})


    def test_a_run_appends_one_audit_line(self):
        self.scenario(rc="3 \r\n")
        out = self.run_exec(command="dir C:\\", caller="ann.lee@demo-1")
        self.assertEqual(self.events(out)[-1], {"exit": 3})
        lines = self.log_lines()
        self.assertEqual(len(lines), 1)
        when, caller, machine, address, command, result, duration = lines[0]
        self.assertRegex(when, r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual(
            (caller, machine, address, command, result),
            ("ann.lee@demo-1", "NT4SRV01", "192.0.2.20", "dir C:\\", "3"),
        )
        self.assertRegex(duration, r"^\d+\.\d{3}$")
        self.assertEqual(os.stat(self.log).st_mode & 0o777, 0o600)

    def test_the_caller_defaults_to_cli_and_must_be_plain(self):
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.log_lines()[0][1], "cli")
        for bad in ("ann lee", "a/b", "x" * 65):
            out = self.run_exec(caller=bad)
            self.assertEqual(out.returncode, 1, bad)
            self.assertIn("caller is not valid", self.events(out)[-1]["error"])
        self.assertEqual(len(self.log_lines()), 1)

    def test_the_password_never_reaches_the_log(self):
        out = self.run_exec(command="echo hi", caller="ann")
        self.assertEqual(out.returncode, 0, out.stderr)
        for name in os.listdir(self.tmp):
            path = os.path.join(self.tmp, name)
            if name.startswith("exec.log") and os.path.isfile(path):
                with open(path) as f:
                    self.assertNotIn(SECRET, f.read())

    def test_a_command_can_be_logged_as_its_hash(self):
        import hashlib

        out = self.run_exec(
            command="net user",
            args=("--machine", "nt4srv01", "--timeout", "5", "--hash-command"),
        )
        self.assertEqual(out.returncode, 0, out.stderr)
        digest = hashlib.sha256(b"net user").hexdigest()
        self.assertEqual(self.log_lines()[0][4], "sha256:" + digest)

    def test_failures_and_timeouts_are_logged(self):
        self.flag("put_error")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertEqual(self.log_lines()[-1][5], "error")
        os.remove(os.path.join(self.stubs, "put_error"))
        self.flag("hang")
        out = self.run_exec(args=("--machine", "nt4srv01", "--timeout", "1"))
        self.assertEqual(self.events(out)[-1], {"timed_out": True})
        self.assertEqual(self.log_lines()[-1][5], "timed_out")

    def test_the_log_rotates_and_keeps_one_old_file(self):
        self.env["LANLORD_EXEC_LOG_MAX"] = "300"
        for n in range(8):
            out = self.run_exec(command="echo run%d" % n)
            self.assertEqual(out.returncode, 0, out.stderr)
        old = self.log + ".1"
        self.assertTrue(os.path.exists(old))
        self.assertFalse(os.path.exists(self.log + ".2"))
        self.assertEqual(os.stat(old).st_mode & 0o777, 0o600)
        self.assertEqual(os.stat(self.log).st_mode & 0o777, 0o600)
        self.assertEqual(self.log_lines()[-1][4], "echo run7")
        self.assertLess(os.path.getsize(self.log), 300 + 200)

    def stub_flock_without_w(self):
        # busybox flock: no -w, any other use goes to the real one
        real = shutil.which("flock")
        path = os.path.join(self.stubs, "bin", "flock")
        with open(path, "w") as f:
            f.write(
                '#!/bin/bash\nfor a in "$@"; do [[ $a == -w ]] && { echo "flock: unrecognized option: w" >&2; exit 1; }; done\nexec %s "$@"\n'
                % real
            )
        os.chmod(path, 0o755)

    def test_the_audit_line_is_written_with_a_flock_that_has_no_w(self):
        self.stub_flock_without_w()
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertNotIn("could not write the audit log", out.stderr)
        self.assertEqual(len(self.log_lines()), 1)

    def test_a_held_audit_lock_gives_up_with_a_warning(self):
        import fcntl

        self.stub_flock_without_w()
        self.env["LANLORD_EXEC_LOCK_TRIES"] = "3"
        with open(self.log + ".lock", "w") as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            out = self.run_exec()
        self.assertEqual(out.returncode, 0)
        self.assertIn("warning: could not write the audit log", out.stderr)
        self.assertFalse(os.path.exists(self.log))

    def test_an_unwritable_log_does_not_fail_the_run(self):
        # the log's parent is a regular file, so it cannot be created even by root
        blocker = os.path.join(self.tmp, "blocker")
        open(blocker, "w").close()
        self.env["LANLORD_EXEC_LOG"] = os.path.join(blocker, "exec.log")
        self.scenario(out="hello\r\n", rc="0\r\n")
        out = self.run_exec()
        self.assertEqual(out.returncode, 0)
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertIn("warning: could not write the audit log", out.stderr)
        self.assertNotIn(SECRET, out.stderr)
        self.assert_clean()

    def test_large_output_comes_in_chunks_of_at_most_1_mib(self):
        mib = 1024 * 1024
        data = os.urandom(2 * mib + 12345)
        with open(os.path.join(self.stubs, "out"), "wb") as f:
            f.write(data)
        self.scenario(rc="0\r\n")
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr[:200])
        events = self.events(out)
        chunks = [base64.b64decode(e["data"]) for e in events if e.get("stream") == "out"]
        self.assertGreaterEqual(len(chunks), 3)
        self.assertTrue(all(0 < len(c) <= mib for c in chunks))
        self.assertEqual(b"".join(chunks), data)
        self.assertEqual(events[-1], {"exit": 0})


    def test_a_refused_command_is_audited_as_refused(self):
        out = self.run_exec(command="ver\r\ndel x", caller="ann")
        self.assertEqual(out.returncode, 1)
        line = self.log_lines()[-1]
        self.assertEqual((line[1], line[2], line[5]), ("ann", "NT4SRV01", "refused"))
        self.assertEqual(self.calls(), "")

    def test_a_killed_run_is_audited_as_killed_and_cleans_up(self):
        import signal

        self.flag("hang")
        self.flag("slow")
        request = {"user": "DEMO\\Administrator", "password": SECRET, "command": "ver"}
        proc = subprocess.Popen(
            ["bash", EXEC, "run", "--machine", "nt4srv01", "--timeout", "60"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=self.env,
        )
        proc.stdin.write(json.dumps(request))
        proc.stdin.close()
        # the start stub touches "ready" once it is running, so the kill comes after the service exists
        ready = os.path.join(self.stubs, "ready")
        deadline = time.time() + 30
        while not os.path.exists(ready):
            self.assertLess(time.time(), deadline, "the start never began")
            self.assertIsNone(proc.poll())
            time.sleep(0.02)
        proc.send_signal(signal.SIGTERM)
        proc.communicate(timeout=30)
        self.assertEqual(self.log_lines()[-1][5], "killed")
        self.assert_clean()


if __name__ == "__main__":
    unittest.main()
