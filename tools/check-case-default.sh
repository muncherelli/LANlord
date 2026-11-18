#!/usr/bin/env bash
# check that every case statement has a default `*)` branch.
#
# usage: tools/check-case-default.sh [file...]
#   with no arguments it checks every tracked *.sh file and the run: blocks of
#   .github/workflows/*.yml. prints file:line for each case without a default
#   branch and exits 1 if there are any.
#
# it is a line scanner, not a parser: comments and quoted strings are blanked
# first, nested cases are tracked with a stack, and heredoc bodies are scanned too
# (quotes do not span lines there). good enough for the shapes this repo uses.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ "$#" -eq 0 ]; then
  mapfile -t files < <(git ls-files '*.sh' '.github/workflows/*.yml')
  set -- "${files[@]}"
fi

status=0
for f in "$@"; do
  out=$(awk -v file="$f" '
    BEGIN { yml = (file ~ /\.ya?ml$/); depth = 0; inrun = 0; q = ""; hd = "" }

    # for workflow files only look inside run: blocks, keeping line numbers
    yml {
      match($0, /^ */); ind = RLENGTH
      if (inrun && $0 !~ /^ *$/ && ind <= runind) { inrun = 0; q = ""; depth = 0; hd = "" }
      if (!inrun && match($0, /^ *(- +)?run: */)) {
        runind = index($0, "run:") - 1
        rest = substr($0, RLENGTH + 1)
        if (rest ~ /^[|>][-+0-9]*$/ || rest == "") { inrun = 1; next }
        $0 = rest
      } else if (!inrun) next
    }

    {
      # blank comments and quoted text, remember quotes across lines
      line = $0; out = ""; prev = " "
      # inside a heredoc body quotes never span lines (prose has apostrophes)
      if (hd != "") {
        stripped = line; sub(/^[ \t]+/, "", stripped)
        if (stripped == hd) { hd = ""; q = ""; next }
        q = ""
      }
      for (i = 1; i <= length(line); i++) {
        c = substr(line, i, 1)
        if (q != "") {
          if (c == "\\" && q == "\"") i++
          else if (c == q) q = ""
          out = out " "
        } else if (c == "\\") { i++; out = out "  " }
        else if (c == "\047" || c == "\"") { q = c; out = out " " }
        else if (c == "#" && prev ~ /[ \t;&|(]/) break
        else out = out c
        prev = c
      }
      # a heredoc opened on this line ends at its delimiter word
      if (hd == "" && match(line, "(^|[^<])<<-?[ \t]*[\"\047]?[A-Za-z_][A-Za-z_0-9]*")) {
        hd = substr(line, RSTART, RLENGTH); sub(/^.*<</, "", hd); gsub(/[^A-Za-z_0-9]/, "", hd)
      }
      # command substitutions open a group; ;; ;;& ;& and ; all separate
      gsub(/\$\(|`/, " ( ", out)
      gsub(/;;&|;&|;;|;/, " ; ", out)
      n = split(out, tok, /[ \t]+/)
      last = ""
      for (i = 1; i <= n; i++) {
        t = tok[i]
        if (t == "") continue
        if (t ~ /^esac\)+$/) t = "esac"
        cmdpos = (last == "" || last ~ /^(;|&&|\|\||\||then|do|else|elif|\{|\(|!)$/ || last ~ /\)$/)
        if (t == "case" && cmdpos) { depth++; at[depth] = NR; seen[depth] = 0 }
        else if (t == "esac" && cmdpos && depth > 0) {
          if (!seen[depth]) printf "%s:%d: case has no *) default branch\n", file, at[depth]
          depth--
        } else if (depth > 0 && (last == "" || last == "in" || last == ";" || last ~ /\|$/) \
                   && t ~ /^\(?(.*\|)?\*\)/) seen[depth] = 1
        last = t
      }
    }
  ' "$f")
  if [ -n "$out" ]; then
    printf '%s\n' "$out"
    status=1
  fi
done
exit "$status"
