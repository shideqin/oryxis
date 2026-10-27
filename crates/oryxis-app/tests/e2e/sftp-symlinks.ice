viewport: 1200x750
mode: Zen
-----
# Symbolic links in the dual-pane browser (issue #272), on the Local pane
# (the one a test can reach): the "New symbolic link" dialog from the
# background menu and from a row's "Create link to this", a link to a
# folder that opens like one, and a delete that removes the link alone.
#
# The dialog's keyboard: the name field has focus when it opens, Tab and
# Shift+Tab move between the two fields with the cursor at the end (the
# `x` typed after Shift+Tab lands at the end of the NAME), Enter creates,
# Esc cancels. The emulator applies a focus a frame after the key that
# asked for it, hence the settle after each Tab.
click "Skip"
click "Continue without password"
settle
type ctrl+shift+e
settle
# Dismiss the host picker: this flow only needs the Local pane.
click (785, 166)
settle
click right (300, 400)
settle
click "New folder"
settle
type "ln-target"
type enter
settle
# Filter to the test's own rows, so they sit at fixed heights: `..`
# (y 165), then the rows from y 194 down, 28 px apart.
click (453, 72)
type "ln-"
settle
# A marker inside the target: what proves a link was followed.
click double (300, 194)
settle
click right (300, 400)
settle
click "New file"
settle
type "ln-marker.txt"
type enter
settle
expect "ln-marker.txt"
click double (300, 165)
settle
absent "ln-marker.txt"
# Esc closes the dialog and makes nothing.
click right (300, 400)
settle
click "New symbolic link"
settle
expect "Link name"
type escape
settle
absent "Link name"
# A relative link, made from the background menu.
click right (300, 400)
settle
click "New symbolic link"
settle
type "ln-link"
type tab
settle 250
type "ln-target"
type shift+tab
settle 250
type "x"
type tab
settle 250
type enter
settle
absent "Link name"
# A link to a folder sorts and opens as a folder.
order "ln-linkx" "ln-target"
wait 1200
click double (300, 194)
settle
expect "ln-marker.txt"
click double (300, 165)
settle
absent "ln-marker.txt"
# From a row: the dialog comes up with the row as its target.
click right (300, 222)
settle
click "Create link to this"
settle
type "ln-other"
type enter
settle
order "ln-linkx" "ln-other" "ln-target"
# Deleting a link removes the link, never the folder it leads to.
click right (300, 194)
settle
click "Delete"
settle
click "Delete"
settle
absent "ln-linkx"
order "ln-other" "ln-target"
wait 1200
click double (300, 222)
settle
expect "ln-marker.txt"
