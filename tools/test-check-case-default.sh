#!/usr/bin/env bash
# self-test for tools/check-case-default.sh: each sample is written to a temp
# folder and the reported line numbers are compared with the expected ones.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fails=0

# expect NAME "LINES" : the sample is read from stdin
expect() {
  local name=$1 want=$2 got
  cat > "$tmp/$name"
  got=$("$here/check-case-default.sh" "$tmp/$name" | sed -E 's/^[^:]*:([0-9]+):.*/\1/' | tr '\n' ' ' | sed 's/ $//' || true)
  if [ "$got" = "$want" ]; then echo "ok   $name"; else echo "FAIL $name: wanted '$want', got '$got'"; fails=$((fails + 1)); fi
}

expect plain.sh "" <<'SH'
case $a in x) ;; *) ;; esac
SH
expect missing.sh "1" <<'SH'
case $a in x) ;; esac
SH
expect nested.sh "2" <<'SH'
case $a in
  x) case $b in y) ;; esac ;;
  *) ;;
esac
SH
expect fallthrough.sh "" <<'SH'
case $a in
  x) echo x ;;&
  *) echo any ;;
esac
SH
expect substitution.sh "1" <<'SH'
v=$(case $x in a) ;; esac)
SH
expect backtick.sh "1" <<'SH'
v=`case $x in a) ;; esac`
SH
expect comment-and-string.sh "" <<'SH'
# case x in
echo "case y in"
SH
expect heredoc.sh "5" <<'SH'
cat <<TXT
don't blank what follows
TXT
echo done
case $a in x) ;; esac
SH
expect workflow.yml "5" <<'YML'
jobs:
  a:
    steps:
      - run: |
          case $x in
            a) ;;
          esac
      - name: other
        run: echo "it's fine"
YML
# the unclosed quote in the first run block must not leak into the second
expect workflow-reset.yml "8" <<'YML'
jobs:
  a:
    steps:
      - run: |
          echo "unclosed
      - run: |
          true
          case $x in
            a) ;;
          esac
YML

[ "$fails" -eq 0 ]
