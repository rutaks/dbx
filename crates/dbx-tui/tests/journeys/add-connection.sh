#!/usr/bin/env bash
# Journey add-connection (slice 03): on an empty scratch store, open the form,
# fill it in for the Docker fixture, see a failed test (wrong password) show
# dbx's message with no secret in it, test with the right one, save, see the
# connection in the sidebar, quit, start again and see it still there; then
# edit it (the stored password is used when the field is left blank), and
# remove it after a confirmation.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

WRONG='wrongpw-Qx7-secret'
RIGHT='dbxtui'
# key NAME...: press named keys. typ TEXT: type text literally (tmux takes
# everything after -l as literal text, so it cannot share a command with keys).
key() { tm send-keys -t "$SESSION" "$@"; }
typ() { tm send-keys -t "$SESSION" -l "$1"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

build_bin
scratch_store
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 50-empty 'No connections yet'
expect empty 'Add one with ctrl\+g n'

# prefix then n: the form, with the keys that work in it
key C-g n
wait_for 51-empty-form 'New connection'
expect form 'name'
expect form 'password'
expect form '5432'
expect form 'ctrl\+t test'
expect form '↵ save'
expect form 'esc cancel'

# saving an empty form: the errors sit on their fields
key Enter
wait_for 52-errors 'Name is required\.'
expect errors 'Host is required\.'

# fill it in for the fixture (with the wrong password first)
typ 'fixture pg'
key Tab; typ '127.0.0.1'
key Tab C-u; typ '55433'
key Tab; typ 'postgres'
key Tab; typ "$WRONG"
key Tab; typ 'pagila'
wait_for 53-filled 'pagila'
expect filled 'fixture pg'
expect filled '127\.0\.0\.1'
expect filled '55433'
expect filled '•{18}'
expect_not filled "$WRONG"
expect_not filled 'wrongpw'

# test: dbx's message, no secret, the form stays open
key C-t
wait_for 54-test-failed 'authentication failed for user'
expect test-failed 'New connection'
expect_not test-failed "$WRONG"

# right password: back to the password field, replace it, test again
key BTab C-u; typ "$RIGHT"
key C-t
wait_for 55-test-ok 'Connection successful'
expect_not test-ok 'authentication failed for user'
expect_not test-ok "$RIGHT"
expect test-ok '••••••'

# save: the form closes and the connection is in the sidebar, cursor on it
key Enter
wait_for 56-saved 'fixture pg +postgres'
expect_not saved 'New connection'
expect_not saved 'No connections yet'
expect saved 'e edit'
expect saved 'd remove'
key C-g q
wait_for 57-quit 'EXIT=0'
expect_terminal_restored quit

# start again: still there (the password round-tripped through the store)
tm send-keys -t "$SESSION" "clear; $CLIENT; echo EXIT=\$?" Enter
wait_for 58-restarted 'fixture pg +postgres'
expect restarted '○ fixture pg'

# edit: the form is filled from the connection, the password is not
key e
wait_for 59-edit-form 'Edit connection'
expect edit-form 'fixture pg'
expect edit-form '55433'
expect edit-form '\(unchanged\)'
expect_not edit-form '••'
# esc on an untouched form closes it without asking
key Escape
sleep 0.3
capture 60-edit-closed
expect_not edit-closed 'Edit connection'
expect_not edit-closed 'Discard'

# edit the name; testing with the password left blank uses the stored one
key e
wait_for 61-edit-again 'Edit connection'
key C-u; typ 'pagila local'
key C-t
wait_for 62-edit-test-ok 'Connection successful'
# esc on a changed form asks first, and keeps what was typed when told to
key Escape
wait_for 63-discard 'Discard changes\?'
expect discard '↵ discard'
expect discard 'esc keep editing'
key Escape
sleep 0.3
capture 64-kept
expect kept 'pagila local'
expect_not kept 'Discard changes'
key Enter
wait_for 65-renamed '○ pagila local +postgres'
expect_not renamed 'fixture pg'
expect_not renamed 'Edit connection'

# remove: a confirmation that names it; esc cancels; enter removes
key d
wait_for 66-remove-confirm 'Remove connection'
expect remove-confirm '"pagila local"'
expect remove-confirm '↵ remove'
expect remove-confirm 'esc cancel'
key Escape
sleep 0.3
capture 67-remove-cancelled
expect remove-cancelled '○ pagila local'
expect_not remove-cancelled 'Remove connection'
key d
wait_for 68-remove-again 'Remove connection'
key Enter
wait_for 69-removed 'No connections yet'
expect removed 'Add one with ctrl\+g n'
key C-g q
wait_for 70-after-quit 'EXIT=0'
expect_terminal_restored final

# nothing secret reached any screen
if grep -l -e "$WRONG" -e "$RIGHT" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen: $(grep -l -e "$WRONG" -e "$RIGHT" "$OUT"/*.txt | tr '\n' ' ')"
fi
echo "OK add-connection; captures in $OUT"
