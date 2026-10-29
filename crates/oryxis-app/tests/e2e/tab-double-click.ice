viewport: 1200x750
mode: Zen
-----
# Issue #277: a double click on a terminal tab chip does what Settings >
# Interface > "Double-click on a tab" says, and nothing while it is off
# (the default). Not a shortcut: the left button never enters the hotkey
# table, the press is judged by `double_click` like the file lists'.
#
# The numbering is turned on first so the duplicate has a name of its
# own ("2. bash (default)"): a copy of a local shell is labelled like the
# original, and `order` refuses a text shown twice.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (1176, 64)
settle 500
click "Search settings"
type "Tab number"
settle 300
click "Off"
settle 300
click "Before the label"
settle 300
# Close the Settings tab (its X): a panel tab holds a strip slot too, and
# the shell has to be the first number.
click (165, 21)
settle 300

# Off: a double click is two clicks on a tab that is already selected.
timeout 800
type ctrl+shift+l
settle 500
click double "1. bash (default)"
settle 400
absent "Rename Tab"
absent "2. bash (default)"

# Rename. Settings comes back as the second tab and stays there.
click "Personal"
settle 300
click (1176, 64)
settle 500
click "Search settings"
type "Double-click on a tab"
settle 300
click "Does nothing"
settle 300
click "Renames the tab"
settle 300
click "1. bash (default)"
settle 300
click double "1. bash (default)"
settle 400
expect "Rename Tab"
click "Cancel"
settle 300
absent "Rename Tab"
order "1. bash (default)" "2. Settings"

# Duplicate. The copy lands next to the original.
click "2. Settings"
settle 300
# The search is kept but the page is back at its top: typing it again
# reveals the row.
click (95, 68)
type ctrl+a
type "Double-click on a tab"
settle 300
click "Renames the tab"
settle 300
click "Duplicates the tab"
settle 300
click "1. bash (default)"
settle 300
click double "1. bash (default)"
settle 800
absent "Rename Tab"
order "1. bash (default)" "2. bash (default)" "3. Settings"
screenshot tab-double-click-duplicated

# The choice is the vault's: a second session reads it back.
reset
expect "Create host"
click (1176, 64)
settle 500
click "Search settings"
type "Double-click on a tab"
settle 300
expect "Duplicates the tab"
