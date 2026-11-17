#!/usr/bin/env bash
# fails when a pull request leaves open sonarcloud issues of any severity.
# the default "sonar way" gate only fails on ratings, so a pull request can add code
# smells and still pass; this closes that gap. it runs after the scan, which waits
# for the analysis to finish (sonar.qualitygate.wait), so the results are current.
#
# usage: tools/sonar-gate.sh PULL_REQUEST_NUMBER
set -euo pipefail

pr="${1:?pull request number}"
project="${SONAR_PROJECT:-muncherelli_LANlord}"
url="https://sonarcloud.io/api/issues/search?componentKeys=${project}&pullRequest=${pr}&issueStatuses=OPEN,CONFIRMED&ps=500"

curl -fsS "$url" | python3 -c '
import json, sys
data = json.load(sys.stdin)
issues = data["issues"]
for issue in issues:
    where = issue["component"].split(":", 1)[-1]
    line = issue.get("line", "-")
    rule, message = issue["rule"], issue["message"]
    print(f"::error file={where},line={line}::{rule} {message}")
print(f"{len(issues)} open sonarcloud issue(s) in this pull request")
sys.exit(1 if issues else 0)
'
