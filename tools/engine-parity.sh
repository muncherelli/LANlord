#!/usr/bin/env bash
# compare what the playbook and the native engine leave behind for one module.
# run as root on a scratch gateway, in two steps, changing the module's `engine:` setting between:
#   engine: {nt4-domain: ansible}   tools/engine-parity.sh collect ansible
#   engine: {nt4-domain: native}    tools/engine-parity.sh collect native
#   tools/engine-parity.sh compare
# `collect` applies the module with the engine set at that moment and saves its state; `compare`
# diffs the two documents. passwords appear only as matches or not, never as values.
# set MODULE to check another module (default nt4-domain).
set -euo pipefail

module="${MODULE:-nt4-domain}"
dir="${PARITY_DIR:-/var/tmp/lanlord-engine-parity}"

if [ "$(id -u)" -ne 0 ]; then
    echo "run as root on the gateway" >&2
    exit 2
fi

case "${1:-}" in
collect)
    label="${2:?usage: engine-parity.sh collect ansible|native}"
    case "$label" in ansible | native) ;; *)
        echo "label is ansible or native" >&2
        exit 2
        ;;
    esac
    mkdir -p "$dir"
    lanlord apply this-server.localhost --module "$module"
    lanlord engine state --module "$module" >"$dir/$label.json"
    echo "saved $dir/$label.json"
    ;;
compare)
    # the playbook records no fingerprints, so that key, in every server, is expected to differ
    for engine in ansible native; do
        jq 'del(.servers[].password_recorded_current)' "$dir/$engine.json" >"$dir/$engine.cmp"
    done
    if diff -u "$dir/ansible.cmp" "$dir/native.cmp"; then
        echo "parity: $module is the same after both engines"
    else
        echo "parity: $module differs (ansible first, native second)" >&2
        exit 1
    fi
    ;;
*)
    echo "usage: engine-parity.sh collect ansible|native | compare" >&2
    exit 2
    ;;
esac
