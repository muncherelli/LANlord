#!/usr/bin/env bash
# lint the repo's shell code: shellcheck on every tracked *.sh file, actionlint
# (which shellchecks the run: blocks) on the workflows, and a check that every
# case statement has a default branch.
#
# needs shellcheck and actionlint on PATH. run it from anywhere in the repo.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

for tool in shellcheck actionlint; do
  command -v "$tool" > /dev/null || { echo "lint-shell: $tool not found on PATH" >&2; exit 2; }
done

status=0
mapfile -t scripts < <(git ls-files '*.sh')

echo "shellcheck: ${#scripts[@]} scripts"
shellcheck -x -S warning "${scripts[@]}" || status=1

echo "actionlint: .github/workflows"
actionlint || status=1

echo "case defaults"
tools/check-case-default.sh || status=1

exit "$status"
