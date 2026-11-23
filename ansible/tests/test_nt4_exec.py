# run with: python3 -m unittest discover -s ansible/tests
import base64
import json
import os
import re
import shlex
import shutil
import subprocess
import tempfile
import time
import unittest

IMAGE = os.path.join(os.path.dirname(__file__), "..", "playbooks", "nt4-domain", "image")
EXEC = os.path.join(IMAGE, "lanlord-exec")
SECRET = "S3cret-pw!x"

# smbclient, smbcacls, rpcclient, net and nmblookup are stubs that record their arguments. the "machine" is a folder:
# put and get move files in and out of it, and starting the service plays the .bat's part by
# writing .out, .err and .rc from the scenario files (out, err, rc, hang, start_error, put_error);
# remote/ is the root of ADMIN$ (with a Temp) and cdrive/ the root of the drive share (C$ or D$), where the private
# work folder cdrive/lanlord lives (setUp makes it; mkdir makes it otherwise). a cd to a missing folder
# prints the error and carries on, as smbclient does. smbcacls keeps nothing: a read prints the file
# acl_parent (the drive root), acl_file (an uploaded file) or acl (the folder) of the scenario, with good
# defaults when absent, and acl_set_error, acl_set_noise or acl_read_error make the set or the read fail.
# rpcclient answers netsharegetinfo with the path in adminpath (default C:\WINNT) and lookupnames with
# the sid in usersid (no_share makes the share lookup fail)
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
[[ $1 == //*/[A-Z]\$ ]] && remote="$STUBS/cdrive"
case $name in
  nmblookup)
    [[ -e $STUBS/no_wins ]] && { echo "name_query failed"; exit 1; }
    echo "querying $1 on 192.0.2.255"
    echo "192.0.2.20 $1<00>" ;;
  smbclient)
    [[ -e $STUBS/put_error ]] && [[ $* == *put* ]] && { if [[ -s $STUBS/put_error ]]; then cat "$STUBS/put_error"; else printf 'NT_STATUS_ACCESS_DENIED \001 "x" \\ \a\n'; fi; exit 1; }
    cmds=
    cur=$remote
    for ((k = 0; k < ${#args[@]}; k++)); do [[ ${args[k]} == -c ]] && cmds=${args[k+1]}; done
    IFS=';' read -ra list <<< "$cmds"
    for c in "${list[@]}"; do
      read -ra w <<< "$c"
      case ${w[0]:-} in
        mkdir)
          p=${w[1]//\\//}
          if [[ -e $remote/$p ]]; then { echo "mkdir failed on directory $p: NT_STATUS_OBJECT_NAME_COLLISION"; exit 1; }
          elif [[ -e $STUBS/mkdir_error ]]; then { echo "mkdir failed on directory $p: NT_STATUS_ACCESS_DENIED"; exit 1; }
          elif [[ $p == */* && ! -d $remote/${p%/*} ]]; then { echo "mkdir failed on directory $p: NT_STATUS_OBJECT_PATH_NOT_FOUND"; exit 1; }
          else mkdir "$remote/$p"; fi ;;
        cd)
          p=${w[1]//\\//}
          if [[ -d $cur/$p ]]; then cur=$cur/$p; else echo "cd \\$p\\: NT_STATUS_OBJECT_NAME_NOT_FOUND"; fi ;;
        put) cp "${w[1]}" "$cur/${w[2]}"; cp "${w[1]}" "$STUBS/bats/${w[2]}" ;;
        get)
          # in the slow scenario the poll waits until the start stub has written its pids, so the kill
          # on timeout can never come first
          if [[ -e $STUBS/slow ]]; then n=0; while [[ ! -e $STUBS/ready ]] && ((n++ < 200)); do sleep 0.05; done; fi
          [[ -f $cur/${w[1]} ]] && cp "$cur/${w[1]}" "${w[2]}" ;;
        del) [[ -e $STUBS/del_hang && ${w[1]} != *[*?]* ]] && sleep 30; (cd "$cur" && rm -f ${w[1]}) ;;
      esac
    done
    exit 0 ;;
  smbcacls)
    # //addr/SHARE path -A file ... [--sddl --set SDDL | --numeric]
    path=${2//\\//}
    if [[ $path != / ]]; then
      [[ -e $remote/$path ]] || { echo "Failed to open $2: NT_STATUS_OBJECT_NAME_NOT_FOUND"; exit 1; }
    fi
    if [[ $* == *--set* ]]; then
      msg="ERROR: security descriptor set failed: NT_STATUS_ACCESS_DENIED"
      [[ -e $STUBS/acl_set_error ]] && { echo "$msg"; exit 1; }
      [[ -e $STUBS/acl_set_noise ]] && echo "$msg"
      exit 0
    fi
    [[ -e $STUBS/acl_read_error ]] && { echo "NT_STATUS_ACCESS_DENIED"; exit 1; }
    case $path in
      /) f=acl_parent ;;
      lanlord/ll-*) f=acl_file ;;
      *) f=acl ;;
    esac
    if [[ -e $STUBS/$f ]]; then cat "$STUBS/$f"; exit 0; fi
    printf 'REVISION:1\nCONTROL:0x8004\nOWNER:S-1-5-32-544\nGROUP:S-1-5-21-1-2-3-513\n'
    if [[ $f == acl_parent ]]; then
      printf 'ACL:S-1-5-32-544:0/0x0/0x001f01ff\nACL:S-1-1-0:0/0x3/0x001301bf\n'
      printf 'ACL:S-1-3-0:0/0x3/0x001f01ff\nACL:S-1-5-18:0/0x0/0x001f01ff\n'
    else
      printf 'ACL:S-1-5-32-544:0/0x3/0x001f01ff\nACL:S-1-5-18:0/0x3/0x001f01ff\n'
    fi
    exit 0 ;;
  rpcclient)
    # rpcclient HOST -A file ... -c CMD
    cmd=${args[${#args[@]}-1]}
    case $cmd in
      netsharegetinfo*)
        [[ -e $STUBS/no_share ]] && { echo "result was NT_STATUS_ACCESS_DENIED"; exit 1; }
        adminpath=C:\\WINNT
        [[ -e $STUBS/adminpath ]] && adminpath=$(cat "$STUBS/adminpath")
        printf 'netname: ADMIN$\r\n\tremark:\tRemote Admin\r\n\tpath:\t%s\r\n\tpassword:\t\r\n' "$adminpath" ;;
      lookupnames*)
        sid=S-1-5-21-1-2-3-1105
        [[ -e $STUBS/usersid ]] && sid=$(cat "$STUBS/usersid")
        echo "${cmd#lookupnames } $sid (User: 1)" ;;
    esac
    exit 0 ;;
  net)
    verb=$3
    case $verb in
      list) cat "$STUBS/services" ;;
      create)
        [[ -e $STUBS/create_io_timeout ]] && { echo "Create service request failed. [NT_STATUS_IO_TIMEOUT]"; exit 2; }
        echo "$4" >> "$STUBS/services" ;;
      stop) echo "stopped $4" >> "$STUBS/stopped" ;;
      delete) sed -i "/^$4\$/d" "$STUBS/services" ;;
      start)
        [[ -e $STUBS/start_error ]] && { echo "Query status request failed. [WERR_ACCESS_DENIED]"; exit 1; }
        [[ -e $STUBS/start_io_timeout ]] && io_timeout=1
        [[ -e $STUBS/slow ]] && { echo $$ > "$STUBS/startpid"; sleep 30 & echo $! > "$STUBS/sleeppid"; touch "$STUBS/ready"; wait; }
        # the files are named after the service, not the first bat found (earlier runs leave theirs)
        [[ -e $STUBS/hang ]] || {
          [[ -e $STUBS/out ]] && cp "$STUBS/out" "$STUBS/cdrive/lanlord/ll-${4#LANlordExec}.out"
          [[ -e $STUBS/err ]] && cp "$STUBS/err" "$STUBS/cdrive/lanlord/ll-${4#LANlordExec}.err"
          rcname=ll-${4#LANlordExec}.rc
          if [[ -e $STUBS/late ]]; then
            ( sleep 1.5; cp "$STUBS/rc" "$STUBS/cdrive/lanlord/$rcname" ) >/dev/null 2>&1 &
          else
            cp "$STUBS/rc" "$STUBS/cdrive/lanlord/$rcname"
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
        for command in ("smbclient", "smbcacls", "rpcclient", "net", "nmblookup"):
            path = os.path.join(self.stubs, "bin", command)
            with open(path, "w") as f:
                f.write(STUB)
            os.chmod(path, 0o755)
        self.svc = os.path.join(self.tmp, "lanlord-svc.exe")
        with open(self.svc, "wb") as f:
            f.write(b"MZ dummy helper")
        self.env = dict(
            os.environ,
            PATH=os.path.join(self.stubs, "bin") + ":" + os.environ["PATH"],
            STUBS=self.stubs,
            LANLORD_EXEC_DIR=self.locks,
            LANLORD_EXEC_TMP=self.scratch,
            LANLORD_EXEC_POLL="0.1",
            LANLORD_EXEC_SVC=self.svc,
            LANLORD_EXEC_LOG=os.path.join(self.tmp, "exec.log"),
        )
        self.log = self.env["LANLORD_EXEC_LOG"]
        os.makedirs(os.path.join(self.stubs, "remote", "Temp"))
        os.makedirs(os.path.join(self.stubs, "cdrive", "lanlord"))
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
        # files left in Temp (an old place) and in the work folder
        temp = os.path.join(self.stubs, "remote", "Temp")
        return os.listdir(temp) + os.listdir(os.path.join(self.stubs, "cdrive", "lanlord"))

    def root_files(self):
        return [n for n in os.listdir(os.path.join(self.stubs, "remote")) if n != "Temp"]

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
        self.assertNotIn("cmd.exe", calls)

    def test_the_helper_is_put_and_the_service_runs_it_with_the_bat(self):
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        calls = self.calls()
        run_id = re.search(r"put \S+/ll-([0-9a-f]{8})\.bat ll-", calls).group(1)
        self.assertRegex(calls, r"put %s ll-%s\.exe" % (re.escape(self.svc), run_id))
        self.assertIn(
            "net rpc service create LANlordExec%s LANlord exec "
            '"C:\\lanlord\\ll-%s.exe" C:\\lanlord\\ll-%s.bat -S ' % (run_id, run_id, run_id),
            calls,
        )
        self.assertIn("del ll-%s.exe" % run_id, calls)
        self.assertEqual(self.remote(), [])

    def test_a_missing_helper_is_a_clear_error_before_anything_is_sent(self):
        os.remove(self.svc)
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("service helper", self.events(out)[-1]["error"])
        self.assertIn("missing", self.events(out)[-1]["error"])
        self.assertNotIn("service create", self.calls())
        self.assert_clean()

    def test_a_timeout_stops_the_service_before_it_is_deleted(self):
        self.flag("hang")
        out = self.run_exec(args=("--machine", "nt4srv01", "--timeout", "1"))
        self.assertEqual(self.events(out)[-1], {"timed_out": True})
        calls = self.calls()
        stop = calls.index("net rpc service stop LANlordExec")
        self.assertLess(stop, calls.index("net rpc service delete LANlordExec"))
        self.assert_clean()

    def test_a_normal_run_does_not_stop_the_service(self):
        self.run_exec()
        self.assertNotIn("service stop", self.calls())

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
        d = "C:\\lanlord\\"
        run_id = re.search(r"ll-([0-9a-f]{8})\.cmd\.bat", outer).group(1)
        self.assertEqual(inner, "@echo off\r\ndir %SystemRoot%\\system32 & for %%i in (a b) do echo %%i\r\n")
        self.assertEqual(
            outer,
            "@echo off\r\n"
            "call " + d + "ll-" + run_id + ".cmd.bat > " + d + "ll-" + run_id + ".out"
            " 2> " + d + "ll-" + run_id + ".err\r\n"
            "echo %ERRORLEVEL% > " + d + "ll-" + run_id + ".rc\r\n",
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
        self.assertIn("service manager on NT4SRV01 is busy", error)
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
        for name in ("ll-deadbeef.bat", "ll-deadbeef.exe"):
            with open(os.path.join(self.stubs, "remote", "Temp", name), "w") as f:
                f.write("x")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertIn("net rpc service delete LANlordExecdeadbeef", self.calls())
        self.assertNotIn("delete OtherService", self.calls())
        self.assertEqual(self.services(), ["OtherService"])
        self.assertEqual(self.remote(), [])

    def test_the_sweep_stops_leftover_services_before_deleting_them(self):
        self.scenario(services="LANlordExec0123abcd\nSpooler\n")
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(self.events(out), [{"swept": 1}])
        calls = self.calls()
        self.assertLess(
            calls.index("net rpc service stop LANlordExec0123abcd"),
            calls.index("net rpc service delete LANlordExec0123abcd"),
        )
        self.assertNotIn("stop Spooler", calls)

    def lines(self):
        names = ("smbclient", "smbcacls", "rpcclient", "net")
        return [c for c in self.calls().splitlines() if c.split(" ")[0] in names]

    def index(self, lines, *parts):
        return next(i for i, c in enumerate(lines) if all(p in c for p in parts))

    def acl_calls(self):
        return [c for c in self.calls().splitlines() if c.startswith("smbcacls")]

    def assert_nothing_sent(self, out):
        self.assertEqual(out.returncode, 1)
        calls = self.calls()
        self.assertNotIn(" put ", calls)
        self.assertNotIn("service create", calls)
        self.assertEqual(self.remote(), [])
        self.assertEqual(os.listdir(os.path.join(self.stubs, "bats")), [])

    def error(self, out):
        return self.events(out)[-1]["error"]

    def head(self, owner="S-1-5-32-544"):
        return "REVISION:1\nCONTROL:0x8004\nOWNER:%s\nGROUP:S-1-5-21-1-2-3-513\n" % owner

    GOOD = "ACL:S-1-5-32-544:0/0x3/0x001f01ff\nACL:S-1-5-18:0/0x3/0x001f01ff\n"

    def test_the_folder_is_made_on_the_system_drive_when_it_is_missing(self):
        shutil.rmtree(os.path.join(self.stubs, "cdrive", "lanlord"))
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        lines = self.lines()
        self.assertLess(self.index(lines, "//192.0.2.20/C$", "mkdir lanlord"), self.index(lines, " put "))
        self.assertTrue(os.path.isdir(os.path.join(self.stubs, "cdrive", "lanlord")))
        self.assertNotIn("mkdir Temp", self.calls())

    def test_a_new_folder_is_locked_by_the_very_next_call_after_the_mkdir(self):
        shutil.rmtree(os.path.join(self.stubs, "cdrive", "lanlord"))
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        lines = self.lines()
        mk = self.index(lines, "mkdir lanlord")
        self.assertIn("smbcacls", lines[mk + 1])
        self.assertIn("--set", lines[mk + 1])
        # the folder check comes after the lock, and before any put
        check = next(i for i, c in enumerate(lines) if c.endswith("-c cd lanlord"))
        self.assertLess(mk + 1, check)
        self.assertLess(check, self.index(lines, " put "))

    def test_an_existing_folder_is_checked_before_the_lock_and_before_any_put(self):
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        lines = self.lines()
        mk = self.index(lines, "mkdir lanlord")
        owner = self.index(lines[mk:], "smbcacls", "C$ lanlord ", "--numeric") + mk
        check = next(i for i, c in enumerate(lines) if i > mk and c.endswith("-c cd lanlord"))
        setacl = self.index(lines, "--set")
        self.assertLess(owner, check)
        self.assertLess(check, setacl)
        self.assertLess(setacl, self.index(lines, " put "))

    def test_an_existing_folder_is_not_an_error(self):
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("mkdir lanlord", self.calls())

    def test_a_folder_that_cannot_be_made_is_a_clear_error(self):
        shutil.rmtree(os.path.join(self.stubs, "cdrive", "lanlord"))
        self.flag("mkdir_error")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("could not create", self.error(out))
        self.assertNotIn("could not connect", self.error(out))
        self.assertIn("NT_STATUS_ACCESS_DENIED", self.error(out))
        self.assertNotIn(" put ", self.calls())

    def test_a_put_that_cannot_enter_the_folder_fails_instead_of_using_the_root(self):
        # the folder is there for mkdir and smbcacls but every cd fails, so the error shows up in the put
        path = os.path.join(self.stubs, "bin", "smbclient")
        with open(path) as f:
            body = f.read()
        with open(path, "w") as f:
            f.write(body.replace('if [[ -d $cur/$p ]]; then cur=$cur/$p; else', "if false; then :; else"))
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn("not a folder", self.error(out))
        self.assertNotIn("service create", self.calls())

    def test_the_run_files_land_in_the_folder_not_the_root(self):
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.root_files(), [])
        self.assertTrue(any(n.endswith(".cmd.bat") for n in os.listdir(os.path.join(self.stubs, "bats"))))

    def test_the_drive_comes_from_netsharegetinfo(self):
        self.scenario(adminpath="D:\\WINNT40\r\n")
        shutil.rmtree(os.path.join(self.stubs, "cdrive"))
        os.makedirs(os.path.join(self.stubs, "cdrive", "lanlord"))
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        calls = self.calls()
        self.assertIn("rpcclient 192.0.2.20", calls)
        self.assertIn("netsharegetinfo ADMIN$", calls)
        self.assertIn("//192.0.2.20/D$", calls)
        self.assertNotIn("//192.0.2.20/C$", calls)
        self.assertIn('"D:\\lanlord\\ll-', calls)
        self.assertIn("call D:\\lanlord\\ll-", self.bat())

    def test_no_drive_means_no_run(self):
        for scenario in ({"no_share": ""}, {"adminpath": "\\\\odd\\path\r\n"}):
            self.scenario(**scenario)
            out = self.run_exec()
            self.assert_nothing_sent(out)
            self.assertIn("system drive", self.error(out))
            self.assertNotIn("C$", self.calls().replace("netsharegetinfo ADMIN$", ""))
            os.remove(os.path.join(self.stubs, "calls"))
            open(os.path.join(self.stubs, "calls"), "w").close()
            for name in ("no_share", "adminpath"):
                if os.path.exists(os.path.join(self.stubs, name)):
                    os.remove(os.path.join(self.stubs, name))

    def test_the_password_never_reaches_rpcclient_argv(self):
        self.run_exec()
        rpc = [c for c in self.calls().splitlines() if c.startswith("rpcclient")]
        self.assertTrue(rpc)
        for c in rpc:
            self.assertNotIn(SECRET, c)
            self.assertIn("client use spnego=no", c)
            self.assertIn("client ipc signing=if_required", c)

    def test_a_failed_tool_with_nothing_to_show_says_no_output(self):
        for log in ("", "\n  \nlpcfg_do_global_parameter: WARNING: the old option\n"):
            with self.subTest(log=log):
                self.scenario(no_share="")
                path = os.path.join(self.stubs, "bin", "rpcclient")
                with open(path, "w") as f:
                    f.write("#!/bin/bash\nprintf '%s' " + shlex.quote(log) + " >&2\nexit 1\n")
                os.chmod(path, 0o755)
                out = self.run_exec()
                self.assertEqual(out.returncode, 1)
                self.assertTrue(self.error(out).endswith("no output"), self.error(out))

    def test_a_sign_in_failure_is_not_reported_as_a_folder_problem(self):
        path = os.path.join(self.stubs, "bin", "smbclient")
        with open(path) as f:
            body = f.read()
        with open(path, "w") as f:
            f.write(body.replace("    cmds=\n", "    echo 'session setup failed: NT_STATUS_LOGON_FAILURE'; exit 1\n    cmds=\n", 1))
        out = self.run_exec()
        self.assertIn("could not connect", self.error(out))
        self.assertNotIn("could not create", self.error(out))

    def test_the_order_is_sweep_parent_set_read_put_verify_create(self):
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        lines = self.lines()
        sweep = self.index(lines, "del ll-????????")
        parent = self.index(lines, "smbcacls", "C$ \\ ")
        setacl = self.index(lines, "smbcacls", "--set")
        read = self.index(lines[setacl:], "smbcacls", "C$ lanlord ", "--numeric") + setacl
        put = self.index(lines, " put ")
        verify = self.index(lines[put:], "smbcacls", "lanlord\\ll-") + put
        create = self.index(lines, "service create")
        self.assertLess(sweep, parent)
        self.assertLess(parent, setacl)
        self.assertLess(setacl, read)
        self.assertLess(read, put)
        self.assertLess(put, verify)
        self.assertLess(verify, create)
        # the folder and the three files are read again, and the root too
        again = [c for c in lines[put:create] if c.startswith("smbcacls")]
        self.assertEqual(len(again), 5)
        # the root, the existing folder, the set, the read-back, then the five reads before the create
        self.assertEqual(len(self.acl_calls()), 1 + 1 + 1 + 1 + 5)

    def test_the_acl_sets_owner_and_a_plain_admin_system_dacl_without_p(self):
        self.run_exec()
        setcall = next(c for c in self.acl_calls() if "--set" in c)
        self.assertIn("--sddl", setcall)
        self.assertIn("O:BAD:(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)", setcall)
        self.assertNotIn("D:P", setcall)
        self.assertIn("//192.0.2.20/C$ lanlord", setcall)
        self.assertEqual(len([c for c in self.acl_calls() if "--set" in c]), 1)
        for c in self.acl_calls():
            self.assertIn("client min protocol=NT1", c)
            self.assertIn("client use spnego=no", c)
            self.assertIn("-A ", c)

    def test_the_password_never_reaches_smbcacls_argv(self):
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertTrue(self.acl_calls())
        for c in self.acl_calls():
            self.assertNotIn(SECRET, c)
        with open(os.path.join(self.stubs, "auth-seen")) as f:
            self.assertIn(SECRET, f.read())

    def test_a_folder_acl_that_lets_everyone_in_is_refused(self):
        self.scenario(acl=self.head() + "ACL:S-1-5-32-544:0/0x3/0x001f01ff\nACL:S-1-1-0:0/0x3/0x001f01ff\n")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("not private", self.error(out))
        self.assert_no_secret(out)

    def test_a_folder_acl_with_a_deny_or_an_empty_dacl_is_refused(self):
        for acl in (self.head() + "ACL:S-1-5-18:1/0x3/0x001f01ff\n", self.head()):
            self.scenario(acl=acl)
            out = self.run_exec()
            self.assert_nothing_sent(out)
            self.assertIn("not private", self.error(out))

    def test_a_folder_owner_that_is_not_an_administrator_is_refused(self):
        self.scenario(acl=self.head("S-1-5-21-1-2-3-1105") + self.GOOD)
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("already exists and is owned by S-1-5-21-1-2-3-1105", self.error(out))
        self.assertIn("delete C:\\lanlord", self.error(out))
        self.assertFalse([c for c in self.acl_calls() if "--set" in c])

    def test_a_fresh_folder_owned_by_the_login_is_taken_over_by_the_set(self):
        shutil.rmtree(os.path.join(self.stubs, "cdrive", "lanlord"))
        out = self.run_exec()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(len([c for c in self.acl_calls() if "--set" in c]), 1)

    def test_system_as_owner_is_accepted(self):
        self.scenario(acl=self.head("S-1-5-18") + self.GOOD)
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})

    def test_a_target_that_is_a_file_means_no_run(self):
        shutil.rmtree(os.path.join(self.stubs, "cdrive", "lanlord"))
        with open(os.path.join(self.stubs, "cdrive", "lanlord"), "w") as f:
            f.write("x")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertNotIn(" put ", self.calls())
        self.assertNotIn("service create", self.calls())
        self.assertIn("not a folder", self.error(out))
        self.assertFalse([c for c in self.acl_calls() if "--set" in c])

    def test_an_acl_that_cannot_be_set_means_no_run(self):
        self.flag("acl_set_error")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("could not lock", self.error(out))

    def test_a_set_that_exits_0_but_prints_an_error_means_no_run(self):
        self.flag("acl_set_noise")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("could not lock", self.error(out))
        self.assertEqual(len([c for c in self.acl_calls() if "--set" in c]), 1)
        self.assertFalse([c for c in self.acl_calls() if "--numeric" in c][2:])

    def test_an_acl_that_cannot_be_read_means_no_run(self):
        self.flag("acl_read_error")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("could not read the acl", self.error(out))

    def test_a_root_that_grants_everyone_full_control_means_no_run(self):
        self.scenario(acl_parent=self.head() + "ACL:S-1-1-0:0/0x3/0x001f01ff\nACL:S-1-5-32-544:0/0x3/0x001f01ff\n")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("too open", self.error(out))
        self.assertIn("S-1-1-0", self.error(out))
        self.assertIn("tighten", self.error(out))
        self.assertFalse([c for c in self.acl_calls() if "--set" in c])

    def test_a_root_that_grants_delete_child_write_dac_or_write_owner_means_no_run(self):
        for mask in ("0x00000040", "0x00040000", "0x00080000", "0x10000000", "0x40000000", "0x001301ff"):
            self.scenario(acl_parent=self.head() + "ACL:S-1-1-0:0/0x0/%s\nACL:S-1-5-18:0/0x0/0x001f01ff\n" % mask)
            out = self.run_exec()
            self.assert_nothing_sent(out)
            self.assertIn("too open", self.error(out), mask)

    def test_a_root_where_only_an_inherit_only_ace_is_wide_open_passes(self):
        self.scenario(acl_parent=self.head() + "ACL:S-1-1-0:0/0xb/0x001f01ff\nACL:S-1-5-32-544:0/0x0/0x001f01ff\n")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})

    def test_a_root_with_an_unknown_owner_an_odd_ace_or_no_aces_means_no_run(self):
        for acl in (
            self.head("S-1-5-21-1-2-3-1105") + "ACL:S-1-5-18:0/0x0/0x001f01ff\n",
            self.head() + "ACL:S-1-5-18:0/0x0/0x001f01ff\nACL:garbage\n",
            self.head() + "ACL:S-1-1-0:5/0x0/0x00000001\nACL:S-1-5-18:0/0x0/0x001f01ff\n",
            self.head(),
        ):
            self.scenario(acl_parent=acl)
            out = self.run_exec()
            self.assert_nothing_sent(out)
            self.assertIn("too open", self.error(out))

    def test_a_denied_ace_on_the_root_is_not_a_grant(self):
        self.scenario(acl_parent=self.head() + "ACL:S-1-1-0:1/0x0/0x001f01ff\nACL:S-1-5-18:0/0x0/0x001f01ff\n")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})

    def test_the_root_like_the_one_on_a_real_server_passes(self):
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})

    def test_a_refused_root_is_swept_first_but_nothing_is_locked_or_used(self):
        self.scenario(services="LANlordExecdeadbeef\n", acl_parent=self.head() + "ACL:S-1-1-0:0/0x0/0x001f01ff\n")
        out = self.run_exec()
        self.assert_nothing_sent(out)
        self.assertIn("service delete", self.calls())

    def test_a_file_that_is_not_private_at_the_end_means_no_create(self):
        self.scenario(acl_file=self.head() + "ACL:S-1-1-0:0/0x0/0x001f01ff\nACL:S-1-5-18:0/0x0/0x001f01ff\n")
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertIn(" put ", self.calls())
        self.assertNotIn("service create", self.calls())
        self.assertIn("not private", self.error(out))
        self.assertIn("ll-", self.error(out))
        self.assert_clean()

    def test_a_file_owned_by_the_login_is_accepted_and_a_stranger_is_not(self):
        self.scenario(usersid="S-1-5-21-1-2-3-1105\n", acl_file=self.head("S-1-5-21-1-2-3-1105") + self.GOOD)
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assertIn("lookupnames DEMO\\Administrator", self.calls())
        self.scenario(acl_file=self.head("S-1-5-21-1-2-3-1999") + self.GOOD)
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertNotIn("service create", self.calls().split("lookupnames")[-1])

    def test_the_folder_changing_after_the_put_means_no_create(self):
        # the folder reads are good at first and bad from the fourth smbcacls read on
        path = os.path.join(self.stubs, "bin", "smbcacls")
        with open(path) as f:
            body = f.read()
        flip = (
            'n=$(cat "$STUBS/reads" 2>/dev/null || echo 0); echo $((n+1)) > "$STUBS/reads"\n'
            '    if ((n >= 4)) && [[ $f == acl ]]; then '
            "printf 'REVISION:1\\nOWNER:S-1-5-32-544\\nACL:S-1-1-0:0/0x3/0x001f01ff\\n'; exit 0; fi\n"
        )
        with open(path, "w") as f:
            f.write(body.replace("    if [[ -e $STUBS/$f ]]; then", flip + "    if [[ -e $STUBS/$f ]]; then", 1))
        out = self.run_exec()
        self.assertEqual(out.returncode, 1)
        self.assertNotIn("service create", self.calls())

    def test_every_run_path_is_in_the_private_folder(self):
        self.scenario(out="x\r\n")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        calls = self.calls()
        self.assertEqual(len(re.findall(r"cd lanlord; put ", calls)), 1)
        self.assertRegex(calls, r"cd lanlord; get ll-[0-9a-f]{8}\.rc")
        self.assertRegex(calls, r"cd lanlord; del ll-[0-9a-f]{8}\.exe")
        self.assertNotRegex(calls, r"cd Temp; (put|get)")
        self.assertNotIn("%SystemRoot%\\Temp\\ll-", calls)
        self.assertNotIn("%SystemRoot%", self.bat())
        self.assert_clean()

    def test_stale_files_in_the_old_and_new_places_are_swept(self):
        with open(os.path.join(self.stubs, "cdrive", "lanlord", "ll-deadbeef.out"), "w") as f:
            f.write("x")
        with open(os.path.join(self.stubs, "remote", "Temp", "ll-deadbeef.out"), "w") as f:
            f.write("x")
        out = self.run_exec()
        self.assertEqual(self.events(out)[-1], {"exit": 0})
        self.assert_clean()

    def test_sweep_without_a_drive_still_cleans_the_old_places(self):
        self.flag("no_share")
        with open(os.path.join(self.stubs, "remote", "Temp", "ll-deadbeef.out"), "w") as f:
            f.write("x")
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(os.listdir(os.path.join(self.stubs, "remote", "Temp")), [])

    def test_sweep_does_not_lock_or_make_anything(self):
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertNotIn("smbcacls", self.calls())
        self.assertNotIn("mkdir", self.calls())

    def test_sweep_without_temp_leaves_other_root_files_alone(self):
        shutil.rmtree(os.path.join(self.stubs, "remote", "Temp"))
        root = os.path.join(self.stubs, "remote")
        for name in ("ll-system.exe", "ll-0123abcd.exe", "ll-0123abcd.cmd.bat"):
            with open(os.path.join(root, name), "w") as f:
                f.write("x")
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.root_files(), ["ll-system.exe"])

    def test_sweep_also_removes_leftovers_in_the_root(self):
        out = self.run_exec(mode="sweep", args=("--machine", "nt4srv01"))
        self.assertEqual(out.returncode, 0, out.stderr)
        sweep = next(c for c in self.calls().splitlines() if c.startswith("smbclient") and "del " in c)
        self.assertIn("del ll-????????.cmd.bat", sweep)
        self.assertLess(sweep.index("del ll-????????.exe"), sweep.index("cd Temp"))
        self.assertNotIn("del ll-*;", sweep.split("cd Temp")[0])

    def test_sweep_removes_what_a_dead_run_left(self):
        self.scenario(services="LANlordExec0123abcd\nLANlordExecffffffff\nSpooler\n")
        for name in ("ll-0123abcd.out", "ll-0123abcd.exe"):
            with open(os.path.join(self.stubs, "remote", "Temp", name), "w") as f:
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
            self.assertEqual(out.returncode, 0, out.stderr + out.stdout)
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
        # communicate below must not flush the closed pipe (python 3.12 raises)
        proc.stdin = None
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
        calls = self.calls()
        self.assertLess(
            calls.index("net rpc service stop LANlordExec"),
            calls.index("net rpc service delete LANlordExec"),
        )


class SvcDockerfileTest(unittest.TestCase):
    # nt4 never enables sse or mmx, so the helper must be built without them and the build must check it
    def setUp(self):
        with open(os.path.join(IMAGE, "Dockerfile")) as f:
            self.text = f.read()

    def test_helper_built_for_a_486_without_sse_or_mmx(self):
        for flag in ("-march=i486", "-mno-mmx", "-mno-sse", "-mno-sse2"):
            self.assertIn(flag, self.text)

    def test_build_fails_on_simd_registers(self):
        self.assertIn("dis=$(llvm-objdump -d \"$exe\")", self.text)
        self.assertIn("grep -Eq '%(x|y|z)?mm[0-9]|\\bcmov|\\bf(u)?comip?\\b'", self.text)
        self.assertIn("exit 1; fi", self.text)


if __name__ == "__main__":
    unittest.main()
