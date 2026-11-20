#!/bin/sh
# self-test for tools/set-version.sh: a fixture workspace is bumped, checked and
# bumped again, and bad versions are refused. also checks the real tree agrees
# with its own workspace version. needs no cargo or network.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

mkdir -p "$tmp/tools" "$tmp/crates/a" "$tmp/crates/b"
cp "$here/set-version.sh" "$tmp/tools/"
cat > "$tmp/Cargo.toml" <<'T'
[workspace.package]
version = "0.1.4"

[workspace.dependencies]
clap = { version = "4", features = ["derive"] }
lanlord-a = { path = "crates/a", version = "0.1.4", default-features = false }
T
cat > "$tmp/crates/a/Cargo.toml" <<'T'
[package]
name = "lanlord-a"
version.workspace = true

[dependencies]
lanlord-b = { version = "0.1.4", path = "../b", optional = true }
chrono = { version = "0.4.45" }
T
cat > "$tmp/crates/b/Cargo.toml" <<'T'
[package]
name = "lanlord-b"
version.workspace = true
T
s="$tmp/tools/set-version.sh"
all() { cat "$tmp/Cargo.toml" "$tmp/crates/a/Cargo.toml" "$tmp/crates/b/Cargo.toml"; }

if "$s" --check 0.1.4 >/dev/null 2>&1; then ok "check passes at the current version"; else bad "check at current version"; fi
if "$s" --check 0.1.5 >/dev/null 2>&1; then bad "check passed at the wrong version"; else ok "check fails at another version"; fi
"$s" 0.1.5
if "$s" --check 0.1.5 >/dev/null 2>&1; then ok "set then check"; else bad "set then check"; fi
if grep -q 'version = "4"' "$tmp/Cargo.toml" && grep -q 'version = "0.4.45"' "$tmp/crates/a/Cargo.toml"; then ok "foreign requirements untouched"; else bad "foreign requirements changed"; fi
if grep -q 'lanlord-b = { version = "0.1.5", path' "$tmp/crates/a/Cargo.toml"; then ok "internal requirement set"; else bad "internal requirement not set"; fi
before=$(all)
"$s" 0.1.5
if [ "$before" = "$(all)" ]; then ok "second run changes nothing"; else bad "second run changed files"; fi
# a drifted internal requirement is caught
sed 's/version = "0.1.5", default/version = "0.1.4", default/' "$tmp/Cargo.toml" > "$tmp/x"
cat "$tmp/x" > "$tmp/Cargo.toml"
if "$s" --check 0.1.5 >/dev/null 2>&1; then bad "drift not caught"; else ok "drifted requirement caught"; fi

for v in 1.2 1.2.3.4 v1.2.3 1.2.3-rc1 1.2.3+b 01.2.3 1.02.3 1..3 .1.2 1.2. "" abc; do
  if "$s" "$v" >/dev/null 2>&1; then bad "accepted '$v'"; else ok "refused '$v'"; fi
done

# the real tree: every file agrees with the workspace version
real=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' "$here/../Cargo.toml")
if "$here/set-version.sh" --check "$real" >/dev/null 2>&1; then ok "repo files agree on $real"; else bad "repo files disagree on the version"; fi

exit "$fail"
