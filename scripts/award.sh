#!/bin/sh
# Awards points to a student picked with fzf from those in the room, which
# saves typing the login. Takes the arguments of `quiz award` after the
# login: the points and, optionally, the question id.
#
#   scripts/award.sh 1 q1.md
#
# Sourced, it defines the shell function `award` instead:
#
#   . scripts/award.sh
#   award -0.5
#
# Nothing is awarded when fzf is left with Esc or Ctrl-C. Needs jq and fzf.

award() {
  local login
  login=$(quiz status --students --format jsonl | jq -r '"\(.login)\t\(.name)"' | fzf --tac | cut -f1)
  # `--`, so that a login starting with a dash is not taken for an option.
  [ -n "$login" ] && quiz award -- "$login" "$@"
}

# Run, with the points, rather than sourced without arguments: award right
# away.
if [ $# -gt 0 ]; then
  award "$@"
  exit
fi
