# run with: python3 -m unittest discover -s ansible/tests
import os
import re
import shutil
import subprocess
import tempfile
import unittest

import yaml

PLAYBOOKS = os.path.join(os.path.dirname(__file__), "..", "playbooks")
IMAGE = os.path.join(PLAYBOOKS, "nt4-domain", "image")
USER = os.path.join(IMAGE, "lanlord-user")

# every external command is a stub that records its arguments; id, getent and pdbedit answer from
# files, so the script's own logic (state files, hooks, the rules for existing accounts) runs for real
STUB = """#!/bin/bash
echo "$(basename "$0") $*" >> "$STUBS/calls"
case "$(basename "$0")" in
  id) grep -qxF -- "$1" "$STUBS/unix" ;;
  getent)
    case "$1" in
      group) grep -q "^$2:" "$STUBS/groups" && grep "^$2:" "$STUBS/groups" ;;
    esac ;;
  pdbedit)
    if [[ $1 == -L && ${2:-} == -u ]]; then grep -qxF -- "$3" "$STUBS/samba"; exit; fi
    if [[ $1 == -L && ${2:-} == -v ]]; then
      # the real -v block (4.22 spells it Logon Script) for the user in the third argument pair
      user=$4
      echo "Unix username:        $user"
      echo "Logon Script:         $(cat "$STUBS/script-$user" 2>/dev/null)"
      echo "Profile Path:         "
      exit
    fi
    if [[ $1 == -L ]]; then sed 's/$/:1000:/' "$STUBS/samba"; fi ;;
  useradd) for last; do :; done; echo "$last" >> "$STUBS/unix" ;;
  groupadd) for last; do :; done; echo "$last:x:9100:" >> "$STUBS/groups" ;;
  smbpasswd) cat > /dev/null; for last; do :; done; echo "$last" >> "$STUBS/samba" ;;
  hostname) echo testbox ;;
  testparm) echo TESTDC ;;
  install) ;;
esac
"""
COMMANDS = ["id", "getent", "pdbedit", "useradd", "groupadd", "smbpasswd", "usermod", "gpasswd",
            "groupdel", "userdel", "install", "hostname", "testparm", "chgrp"]


class LanlordUserTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.stubs = os.path.join(self.tmp, "stubs")
        self.private = os.path.join(self.tmp, "private")
        self.netlogon = os.path.join(self.tmp, "netlogon")
        os.makedirs(os.path.join(self.stubs, "bin"))
        os.makedirs(os.path.join(self.netlogon, "users"))
        for name in ("unix", "samba", "groups", "calls"):
            open(os.path.join(self.stubs, name), "w").close()
        for command in COMMANDS:
            path = os.path.join(self.stubs, "bin", command)
            with open(path, "w") as f:
                f.write(STUB)
            os.chmod(path, 0o755)
        conf = os.path.join(self.tmp, "smb.conf")
        with open(conf, "w") as f:
            f.write("[global]\n   domain logons = yes\n")
        self.env = dict(
            os.environ,
            PATH=os.path.join(self.stubs, "bin") + ":" + os.environ["PATH"],
            STUBS=self.stubs,
            LANLORD_PRIVATE=self.private,
            LANLORD_NETLOGON=self.netlogon,
            LANLORD_SMB_CONF=conf,
        )

    def run_user(self, *args, stdin=""):
        return subprocess.run(["bash", USER, *args], input=stdin, text=True, capture_output=True, env=self.env)

    def calls(self):
        with open(os.path.join(self.stubs, "calls")) as f:
            return f.read()

    def state(self, name):
        path = os.path.join(self.private, name)
        if not os.path.exists(path):
            return None
        with open(path) as f:
            return f.read().split()

    def test_scripts_parse(self):
        for name in ("lanlord-user", "entrypoint.sh"):
            subprocess.run(["bash", "-n", os.path.join(IMAGE, name)], check=True)

    def test_a_new_account_gets_its_role_and_a_dos_wrapper(self):
        out = self.run_user("add", "dana", "admin", "software", stdin="pw\n")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("created dana", out.stdout)
        self.assertEqual(self.state("admins"), ["dana"])
        self.assertEqual(self.state("softadmins"), ["dana"])
        with open(os.path.join(self.netlogon, "users", "dana.bat"), "rb") as f:
            wrapper = f.read()
        self.assertEqual(
            wrapper,
            b"@echo off\r\nset USERNAME=dana\r\ncall \\\\TESTDC\\netlogon\\lanlord.bat\r\n",
        )
        self.assertIn("pdbedit -u dana -S users\\dana.bat", self.calls())

    def test_settings_never_change_the_membership_of_an_existing_account(self):
        self.run_user("add", "dana", "admin", stdin="pw\n")
        # dana is demoted from user manager for domains
        self.assertEqual(self.run_user("hook", "delmember", "admins", "dana").returncode, 0)
        self.assertEqual(self.state("admins"), [])
        out = self.run_user("add", "dana", "admin", stdin="new\n")
        self.assertIn("updated dana", out.stdout)
        self.assertEqual(self.state("admins"), [])
        self.assertNotIn("usermod -aG admins dana\nusermod -aG admins dana", self.calls())
        # and a promotion made there is not undone by settings that say user
        self.run_user("hook", "addmember", "admins", "dana")
        self.run_user("add", "dana", "user", stdin="new\n")
        self.assertEqual(self.state("admins"), ["dana"])

    def test_hooks_record_changes_for_reading_back(self):
        self.run_user("hook", "addgroup", "Printers")
        self.run_user("hook", "addmember", "Printers", "bob")
        self.run_user("hook", "setprimary", "Printers", "bob")
        self.assertEqual(self.state("groups"), ["Printers:9100"])
        self.assertEqual(self.state("members/Printers"), ["bob"])
        self.assertEqual(self.state("primary"), ["bob:Printers"])
        with open(os.path.join(self.private, "changes.log")) as f:
            actions = [line.split()[1] for line in f]
        self.assertEqual(actions, ["addgroup", "addmember", "setprimary"])

    def test_restore_rebuilds_groups_and_memberships(self):
        self.run_user("hook", "addgroup", "printers")
        self.run_user("hook", "addmember", "printers", "bob")
        self.run_user("hook", "addmember", "admins", "bob")
        # a replaced container: the unix side is empty again, the accounts are back
        for name in ("groups", "calls"):
            open(os.path.join(self.stubs, name), "w").close()
        with open(os.path.join(self.stubs, "unix"), "w") as f:
            f.write("bob\n")
        with open(os.path.join(self.stubs, "groups"), "w") as f:
            f.write("admins:x:9998:\n")
        self.assertEqual(self.run_user("restore").returncode, 0)
        calls = self.calls()
        self.assertIn("groupadd -g 9100 printers", calls)
        self.assertIn("usermod -aG printers bob", calls)
        self.assertIn("usermod -aG admins bob", calls)

    def test_delete_and_rename_follow_the_account(self):
        self.run_user("hook", "addmember", "admins", "bob")
        self.run_user("hook", "rename", "bob", "robert")
        self.assertEqual(self.state("admins"), ["robert"])
        self.run_user("hook", "deluser", "robert")
        self.assertEqual(self.state("admins"), [])

    def test_the_lanlord_groups_cannot_be_deleted_from_windows(self):
        self.assertNotEqual(self.run_user("hook", "delgroup", "admins").returncode, 0)

    def test_hook_names_are_checked(self):
        for bad in ("-x", "a b", "a;b", "$(x)", ".."):
            self.assertEqual(self.run_user("hook", "addgroup", bad).returncode, 2, bad)

    def set_script(self, user, script):
        with open(os.path.join(self.stubs, "script-" + user), "w") as f:
            f.write(script)

    def sync_calls(self, user, script):
        self.set_script(user, script)
        with open(os.path.join(self.stubs, "samba"), "w") as f:
            f.write(user + "\n")
        open(os.path.join(self.stubs, "calls"), "w").close()
        self.assertEqual(self.run_user("sync").returncode, 0)
        return [c for c in self.calls().splitlines() if c.startswith("pdbedit -u")]

    def test_a_logon_script_set_from_windows_is_kept(self):
        self.assertEqual(self.sync_calls("dana", "custom\\mine.bat"), [])
        self.assertEqual(self.sync_calls("dana", "users\\dana.bat"), [])

    def test_the_plain_empty_and_stale_scripts_are_pointed_at_the_wrapper(self):
        for old in ("", "lanlord.bat", "users\\other.bat"):
            self.assertEqual(self.sync_calls("dana", old), ["pdbedit -u dana -S users\\dana.bat"], old)

    def test_software_admins_belong_to_windows_after_the_first_sync(self):
        with open(os.path.join(self.stubs, "groups"), "w") as f:
            f.write("softadmins:x:9994:\n")
        os.makedirs(self.private, exist_ok=True)
        with open(os.path.join(self.private, "admins"), "w") as f:
            f.write("dana\n")
        with open(os.path.join(self.stubs, "unix"), "w") as f:
            f.write("dana\n")
        self.run_user("sync")
        self.assertEqual(self.state("softadmins"), ["dana"])
        # removed in user manager: it stays removed through later syncs and restores
        self.run_user("hook", "delmember", "softadmins", "dana")
        open(os.path.join(self.stubs, "calls"), "w").close()
        self.run_user("sync")
        self.run_user("restore")
        self.assertEqual(self.state("softadmins"), [])
        self.assertNotIn("usermod -aG softadmins dana", self.calls())

    def test_rename_only_writes_the_wrapper(self):
        self.run_user("hook", "rename", "bob", "robert")
        self.assertNotIn("pdbedit", self.calls())
        self.assertTrue(os.path.exists(os.path.join(self.netlogon, "users", "robert.bat")))

    def test_groups_from_windows_get_their_own_id_range(self):
        self.run_user("hook", "addgroup", "printers")
        self.assertIn("groupadd -K GID_MIN=20000 -K GID_MAX=29999 printers", self.calls())
        self.assertNotIn("--badname", self.calls())

    def test_names_are_exact_not_patterns(self):
        self.run_user("hook", "addgroup", "a.c")
        self.run_user("hook", "addgroup", "abc")
        self.run_user("hook", "delgroup", "a.c")
        with open(os.path.join(self.private, "groups")) as f:
            self.assertEqual([line.split(":")[0] for line in f], ["abc"])

    def test_hostile_names_run_nothing(self):
        # what smbd does: put the name into the quoted command from smb.conf and run it with sh -c
        with open(os.path.join(PLAYBOOKS, "nt4-domain", "smb.conf.j2")) as f:
            conf = f.read()
        template = next(
            line.split(" = ", 1)[1] for line in conf.splitlines() if line.strip().startswith("add group script")
        ).replace("/usr/local/bin/lanlord-user", "bash " + USER)
        mark = os.path.join(self.tmp, "pwned")
        for name in ("a&b", "`touch " + mark + "`", "$(touch " + mark + ")", "a b", "a'b", "a;touch " + mark):
            command = template.replace("%g", name)
            subprocess.run(["sh", "-c", command], env=self.env, capture_output=True)
            self.assertFalse(os.path.exists(mark), name)
            self.assertNotIn("groupadd", self.calls(), name)

    def test_every_hook_macro_is_quoted(self):
        with open(os.path.join(PLAYBOOKS, "nt4-domain", "smb.conf.j2")) as f:
            conf = f.read()
        for line in conf.splitlines():
            if "lanlord-user hook" in line:
                for macro in re.findall(r"%\w+", line):
                    self.assertIn("'" + macro + "'", line)


class PlaybookTest(unittest.TestCase):
    def setUp(self):
        with open(os.path.join(PLAYBOOKS, "nt4-domain.yml")) as f:
            self.text = f.read()
        with open(os.path.join(PLAYBOOKS, "nt4-domain", "smb.conf.j2")) as f:
            self.conf = f.read()

    def test_playbook_is_yaml(self):
        yaml.safe_load(self.text)

    def test_every_account_hook_is_set(self):
        for key in ("add user script", "delete user script", "rename user script", "add group script",
                    "delete group script", "add user to group script", "delete user from group script",
                    "set primary group script", "add machine script"):
            self.assertIn(f"   {key} = /usr/local/bin/lanlord-user hook ", self.conf)

    def test_logon_script_is_the_per_user_wrapper(self):
        self.assertIn("logon script = users\\%U.bat", self.conf)

    def test_state_is_snapshotted_before_the_config_changes(self):
        snap = self.text.index("snapshot samba's state before")
        self.assertLess(snap, self.text.index("name: write the samba configuration"))
        self.assertIn("tar -czf", self.text)
        for key in ("workgroup", "netbios name", "domain logons"):
            self.assertIn(key, self.text.split("see whether the domain identity changes")[1].split("- name:")[0])

    def test_the_logon_script_guards_windows_9x(self):
        self.assertIn('net use H: /home', self.text)
        self.assertIn('%OS%', self.text)


class ImageTest(unittest.TestCase):
    def setUp(self):
        with open(os.path.join(IMAGE, "Dockerfile")) as f:
            self.dockerfile = f.read()

    def test_winbind_and_rpcclient_are_built_and_kept(self):
        self.assertNotIn("--without-winbind", self.dockerfile)
        removed = self.dockerfile.split("rm -f ")[1].split("&&")[0].split()
        self.assertNotIn("rpcclient", removed)
        self.assertNotIn("wbinfo", removed)
        for flag in ("--disable-cups", "--without-ldap", "--without-ad-dc"):
            self.assertIn(flag, self.dockerfile)

    def test_account_name_mask_patch_is_applied_and_checked(self):
        name = "samba-4.22-mask-account-script-names.patch"
        with open(os.path.join(IMAGE, "patches", name)) as f:
            patch = f.read()
        self.assertEqual(patch.count("+\t\t\t\t\ttrue, false, true);") + patch.count("+\t\t\t\ttrue, false, true);"), 2)
        self.assertIn("talloc_string_sub2", patch)
        self.assertIn("patch -p1 < /patches/" + name, self.dockerfile)
        self.assertIn("grep -c 'talloc_string_sub2'", self.dockerfile)


if __name__ == "__main__":
    unittest.main()
