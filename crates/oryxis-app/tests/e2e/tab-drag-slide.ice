viewport: 1200x750
mode: Zen
-----
# In-strip reorder drag, Chrome-style: the strip slides under the
# cursor WHILE it is moving (chip-rectangle hit-testing, not enter
# events - a chip hovered before the drag never republishes an enter),
# widths stay per-tab (no uniform drag resize), and the release keeps
# the order the live-slide left in place.
#
# Three local shells named alpha / beta / gamma. alpha (first chip,
# ~151..265) is grabbed without selecting it (press publishes nothing,
# iced buttons answer on release) and dragged right: past beta's
# midpoint it lands after beta, past gamma's midpoint / right edge it
# lands last. Screenshot mid-drag shows the ghost riding the cursor
# with the chips at their natural widths (beta wide when active, the
# rest hugging): nothing resizes on pickup. The release is over alpha's
# own trailing gap, so no click publishes, and the order stays
# [beta, gamma, alpha]: the reorder was already real.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
timeout 800
type ctrl+shift+l
settle 500
click right "bash (default)"
settle 250
click "Rename Tab"
settle 250
type ctrl+a
type "alpha"
type enter
settle 300
type ctrl+shift+l
settle 500
click right "bash (default)"
settle 250
click "Rename Tab"
settle 250
type ctrl+a
type "beta"
type enter
settle 300
type ctrl+shift+l
settle 700
click right "bash (default)"
settle 250
click "Rename Tab"
settle 250
type ctrl+a
type "gamma"
type enter
settle 300
expect "gamma"
screenshot slide-0-rest
# Grab alpha, not a click.
press (170, 20)
settle 300
screenshot slide-1-press
# Right: past beta's midpoint flips the seam - alpha after beta.
move (380, 20)
settle 300
screenshot slide-2-after-beta
# Farther: past gamma's midpoint (or trailing edge) - alpha last.
move (600, 20)
settle 300
screenshot slide-3-alpha-last
# Release over alpha's own gap: the order the slide left is the order.
release (600, 20)
settle 600
screenshot slide-4-released
expect "alpha"
expect "beta"
expect "gamma"
