viewport: 1200x750
mode: Zen
-----
# An import started from inside a folder lands in that folder, and the
# hub says so before anything is picked (issue #230). The pickers are
# native dialogs the harness cannot drive, so the promise line is what
# this test pins: the vault side is covered by
# `import_lands_in_the_target_folder` (oryxis-vault) and the confirm
# sites stamp the same snapshot the line reads.
#
# The root hub must NOT carry the line: an import from the root lands
# at the root, exactly as before.
click "Skip"
click "Continue without password"
settle 250
click "Import"
settle 250
expect "Import hosts"
absent "Into folder: Prod"
click "Cancel"
settle 250
absent "Import hosts"
# A folder, opened.
click "New group"
settle 250
click (900.00, 145.00)
type "Prod"
settle 250
click "Save"
settle 250
click "Prod"
settle 250
# The "+ HOST" split menu inside the folder carries the Import entry.
click (1149.00, 119.00)
settle 250
click "Import"
settle 250
expect "Import hosts"
expect "Into folder: Prod"
# Cancel forgets the folder along with the hub.
click "Cancel"
settle 250
absent "Into folder: Prod"
