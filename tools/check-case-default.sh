#!/usr/bin/env bash
# check that every case statement has a default `*)` branch.
#
# usage: tools/check-case-default.sh [file...]
#   with no arguments it checks every tracked *.sh file and the run: blocks of
#   .github/workflows/*.yml. prints file:line for each case without a default
#   branch, or without a closing esac, and exits 1 if there are any. the
#   self-test samples in tools/test-check-case-default.sh are left out.
#
# it is a line scanner, not a parser: comments and quoted strings are blanked
# first, nested cases are tracked with a stack, and heredoc bodies are scanned too
# (quotes do not span lines there). good enough for the shapes this repo uses.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ "$#" -eq 0 ]; then
  mapfile -t files < <(git ls-files '*.sh' '.github/workflows/*.yml' ':!tools/test-check-case-default.sh')
  set -- "${files[@]}"
fi

status=0
for f in "$@"; do
  out=$(awk -v file="$f" '
    # a case that never closed, reported at the line that opened it
    function report_open(   d) {
      for (d = depth; d > 0; d--) printf "%s:%d: case has no matching esac\n", file, at[d]
      depth = 0
    }

    # open (( groups before position pos of s, so << there is a shift, not a heredoc
    function arith_open(s, pos,   j, a, ch, nx) {
      a = 0
      for (j = 1; j < pos; j++) {
        ch = substr(s, j, 1); nx = substr(s, j + 1, 1)
        if (ch == "(" && nx == "(") { a++; j++ }
        else if (ch == ")" && nx == ")" && a > 0) { a--; j++ }
      }
      return a
    }

    BEGIN { yml = (file ~ /\.ya?ml$/); depth = 0; inrun = 0; q = ""; hd = "" }

    # for workflow files only look inside run: blocks, keeping line numbers
    yml {
      match($0, /^ */); ind = RLENGTH
      if (inrun && $0 !~ /^ *$/ && ind <= runind) { report_open(); inrun = 0; q = ""; hd = "" }
      if (!inrun && match($0, /^ *(- +)?run: */)) {
        runind = index($0, "run:") - 1
        rest = substr($0, RLENGTH + 1)
        if (rest ~ /^[|>][-+0-9]*$/ || rest == "") { inrun = 1; next }
        $0 = rest
      } else if (!inrun) next
    }

    {
      # blank comments and quoted text, remember quotes across lines
      line = $0; out = ""; kept = ""; prev = " "
      # inside a heredoc body quotes never span lines (prose has apostrophes)
      inbody = 0
      if (hd != "") {
        inbody = 1
        stripped = line; sub(/^[ \t]+/, "", stripped)
        if (stripped == hd) { hd = ""; q = ""; next }
        q = ""
      }
      for (i = 1; i <= length(line); i++) {
        c = substr(line, i, 1)
        if (q != "") {
          if (c == "\\" && q == "\"") { kept = kept substr(line, i, 2); i++; out = out "  "; prev = c; continue }
          else if (c == q) q = ""
          out = out " "; kept = kept c
        } else if (c == "\\") { kept = kept substr(line, i, 2); i++; out = out "  " }
        else if (c == "\047" || c == "\"") { q = c; out = out " "; kept = kept c }
        else if (c == "#" && prev ~ /[ \t;&|(]/) break
        else { out = out c; kept = kept c }
        prev = c
      }
      # a heredoc opened on this line ends at its delimiter word; << is looked for
      # in the blanked text (so not in quotes or comments, nor inside arithmetic)
      # and the delimiter is read from the same place in the kept text
      if (hd == "") {
        off = 0; scan = out
        while (match(scan, "(^|[^<])<<-?") ) {
          pos = off + RSTART + (substr(scan, RSTART, 1) == "<" ? 0 : 1)
          len = RLENGTH - (substr(scan, RSTART, 1) == "<" ? 0 : 1)
          nextc = substr(out, pos + len, 1)
          if (nextc != "<" && !arith_open(out, pos)) {
            rest = substr(kept, pos + len); sub(/^[ \t]+/, "", rest)
            if (match(rest, "^\\\\?[\"\047]?[A-Za-z_][A-Za-z_0-9]*")) {
              hd = substr(rest, RSTART, RLENGTH); gsub(/[^A-Za-z_0-9]/, "", hd)
              break
            }
          }
          off += RSTART + RLENGTH - 1; scan = substr(out, off + 1)
        }
      }
      # nothing in a heredoc body is code
      if (inbody) next
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
    END { report_open() }
  ' "$f")
  if [ -n "$out" ]; then
    printf '%s\n' "$out"
    status=1
  fi
done
exit "$status"
