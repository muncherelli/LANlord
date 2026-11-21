# run with: python3 -m unittest discover -s ansible/tests
import base64
import json
import os
import re
import shutil
import subprocess
import tempfile
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
    [[ -e $STUBS/put_error ]] && [[ $* == *put* ]] && { echo "NT_STATUS_ACCESS_DENIED"; exit 1; }
    cmds=
    for ((k = 0; k < ${#args[@]}; k++)); do [[ ${args[k]} == -c ]] && cmds=${args[k+1]}; done
    IFS=';' read -ra list <<< "$cmds"
    for c in "${list[@]}"; do
      read -ra w <<< "$c"
      case ${w[0]:-} in
        put) cp "${w[1]}" "$remote/${w[2]}"; cp "${w[1]}" "$STUBS/bats/${w[2]}" ;;
        get) [[ -f $remote/${w[1]} ]] && cp "$remote/${w[1]}" "${w[2]}" ;;
        del) rm -f $remote/${w[1]} ;;
      esac
    done
    exit 0 ;;
  net)
    verb=$3
    case $verb in
      list) cat "$STUBS/services" ;;
      create) echo "$4" >> "$STUBS/services" ;;
      delete) sed -i "/^$4\$/d" "$STUBS/services" ;;
      start)
        [[ -e $STUBS/start_error ]] && { echo "WERR_ACCESS_DENIED"; exit 1; }
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
        )
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

    def bat(self):
        names = os.listdir(os.path.join(self.stubs, "bats"))
        self.assertEqual(len(names), 1)
        with open(os.path.join(self.stubs, "bats", names[0]), "rb") as f:
            return f.read()

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
        self.assertIn("nmblookup NT4SRV01", calls)
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

    def test_the_bat_holds_the_command_as_typed(self):
        out = self.run_exec(command="dir %SystemRoot%\\system32 & for %%i in (a b) do echo %%i")
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        bat = self.bat().decode()
        run_id = re.search(r"ll-([0-9a-f]{8})\.out", bat).group(1)
        self.assertEqual(
            bat,
            "@echo off\r\n"
            "dir %SystemRoot%\\system32 & for %%i in (a b) do echo %%i > %SystemRoot%\\Temp\\ll-" + run_id + ".out"
            " 2> %SystemRoot%\\Temp\\ll-" + run_id + ".err\r\n"
            "echo %ERRORLEVEL% > %SystemRoot%\\Temp\\ll-" + run_id + ".rc\r\n",
        )

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

    def test_a_failed_copy_cleans_up(self):
        self.flag("put_error")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("could not copy", self.events(out)[-1]["error"])
        self.assertNotIn("service create", self.calls())
        self.assert_clean()

    def test_a_timeout_cleans_up(self):
        self.flag("hang")
        out = self.run_exec(args=("--machine", "nt4srv01", "--timeout", "1"))
        self.assertEqual(out.returncode, 0)
        self.assertEqual(self.events(out)[-1], {"timed_out": True})
        self.assert_clean()
        self.assert_no_secret(out)

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


if __name__ == "__main__":
    unittest.main()
