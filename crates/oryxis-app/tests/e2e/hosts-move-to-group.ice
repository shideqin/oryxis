viewport: 1200x750
mode: Zen
-----
# A host changes folder without its editor (issue #230): the card's
# kebab offers "Move to group…", the picker carries a "Top level" row,
# and a selection made from the keyboard (Space on the ringed card)
# moves through the selection bar. The folder card's own host count is
# the assertion, real text where a toast would already be gone.
#
# The drag itself is owner QA: the harness has no press-move-release.
click "Skip"
click "Continue without password"
settle 250
click "Type IP or Hostname"
type "web01"
click "Continue"
settle 250
click "My Server"
type "web01"
click "Save"
settle 250
expect "web01"
# A top-level folder from the "+ HOST" split menu.
click (1149.00, 119.00)
settle 250
click "New group"
settle 250
click (900.00, 145.00)
type "Prod"
settle 250
click "Save"
settle 250
expect "0 hosts"
# Kebab -> Move to group… -> the picker, filtered to the folder, picked
# from the keyboard (the folder card carries the same text, so a text
# click would be ambiguous).
click right "web01"
settle 250
click "Move to group…"
settle 250
expect "Top level (no group)"
click "Search groups…"
type "Prod"
settle 200
type down
type enter
settle 250
expect "1 host"
# Keyboard selection: Down into the groups row, Down to the host, Space
# toggles it, and the bar appears.
type down
settle 100
type down
settle 100
type space
settle 250
expect "1 selected"
# Move to the top level through the bar; the folder empties again.
click "Move to group…"
settle 250
click "Search groups…"
type "Top"
settle 200
type down
type enter
settle 250
absent "1 selected"
expect "0 hosts"
# Esc clears a selection before it touches the ring.
type down
settle 100
type down
settle 100
type space
settle 250
expect "1 selected"
type escape
settle 250
absent "1 selected"
