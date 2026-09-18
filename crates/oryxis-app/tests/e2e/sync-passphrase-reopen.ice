viewport: 1400x1000
mode: Zen
-----
# Issue: the sync passphrase field was pre-filled with the STORED
# (masked) passphrase after a restart, so the next keystroke APPENDED
# to it: the dot run visibly grew, typing was impossible to reason
# about, and the appended value silently swapped the group key, so the
# next round failed with "Crypto error: Decryption failed (wrong
# key?) ". The field now starts EMPTY and the stored value never
# pre-fills it; with a stored passphrase the field is a plain clickable
# masked box (no pencil, no tooltip; the hover border carries it),
# typing never writes through, and the key only changes when a round
# SUCCEEDS with the typed value. The edit field has a reveal eye, so
# what was typed can be read back (the mask is what hid a stray
# character the user could not see). A typed value that disagrees with
# the saved one is only a first attempt: the round falls back to the
# saved passphrase rather than failing, so a stray character (which the
# mask hides) cannot break a sync the saved key could complete, and the
# status line marks the substitution with a warning icon whose tooltip
# carries the explanation.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
settle
# Sync is a feature toggle: enable it on the Plugins screen before the
# Sync section exists in the sidebar. The switch sits on the row's
# right edge (the toggle column is pinned); the row height is
# language-independent (fixed-height rows, one-line descriptions).
click (19, 20)
settle
click "Settings"
settle
click "Features & Plugins"
settle
click (1338, 207)
# Enabling sync mounts an engine that keeps running, so from here on no
# instruction reports done and each one waits out the per-instruction
# timeout instead. Same rule the agent and PTY tests follow; declared
# here rather than in the header so the steps above keep the patient
# default. Measured: 91s -> 30s.
timeout 5000
settle
# The sidebar gained "Sync" under SFTP (fixed-height rows: y 418).
click (60, 418)
settle
expect "Transport"
# The transport row is the first recorded content row: Tab from the
# search rings it, and Right cycles p2p -> sftp -> folder.
click #search-settings
settle
type tab
type right
type right
settle
# Folder card is up. Point it at an existing directory (the snapshot
# is written as <dir>/oryxis-sync.bin), set the group passphrase and
# run the first round so the snapshot is sealed with this key.
# `~/.oryxis` is the SANDBOX vault directory ($HOME is redirected for
# the whole run) and the batch runner wipes it before every test, so
# the round below always starts from "no snapshot yet". A path outside
# it (`/tmp`) would leave a real file on the machine and make the
# record count below depend on whether an earlier run had written one.
click #sync-folder-path
type "~/.oryxis"
settle
click #sync-folder-pass
type "hunter2"
settle
click "Sync Now"
settle 800
expect "Synced, 0 records from the snapshot"
# Restart with the vault intact and reopen the same card. The stored
# passphrase now renders as one clickable masked display (no separate
# Change button): the box itself is the affordance. A wrong typed
# value must fail the round WITHOUT destroying the stored key.
reset
settle
expect "Create host"
click (19, 20)
settle
click "Settings"
settle
click (60, 418)
settle
click "••••••••"
settle
click #sync-folder-pass
type "not-the-key"
settle
click "Sync Now"
settle 800
# What a substitution is NOT is a lost key: the round fell back to the
# stored passphrase, which still opens the snapshot, so it succeeded
# instead of failing and offering the destructive "delete the remote
# snapshot" recovery. Nothing was committed from the field (the stored
# key is already stored), so the edit stays open on the typed text. The
# substitution itself is now marked by a warning icon in FRONT of the
# status line, its explanation in a tooltip: a hover is outside what the
# recorder can do, so the mark is pinned by the steps around it instead
# of by an assertion here.
absent "Decryption failed"
absent "Forgot the passphrase"
expect "records from the snapshot"
# Re-entering the SAVED passphrase syncs on it: the round commits the
# typed value and the read-only masked box comes back. (The Sync now
# click left focus on the button, so re-focus the field before the
# select-all.)
click #sync-folder-pass
type ctrl+a
type backspace
type "hunter2"
settle
click "Sync Now"
settle 800
# The round succeeded, so the typed value was committed and the
# read-only masked box is back (the record count varies with the
# vault's shipped presets, so leaving edit mode is the stable signal).
expect "••••••••"
absent "Decryption failed"
absent "Forgot the passphrase"
