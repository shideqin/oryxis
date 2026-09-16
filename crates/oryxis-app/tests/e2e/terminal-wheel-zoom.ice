viewport: 1200x750
mode: Zen
-----
# Issue #225: Ctrl+wheel zoom is a CHORD on "Zoom font in" / "Zoom font
# out" (one wheel direction each), not a gesture hard-wired in the
# widget, and a zoom is for the session, never the font-size preference.
# Pinned here:
#
#   * the factory rows carry the "Wheel up" / "Wheel down" chips,
#   * Terminal Settings' "Zoom with Ctrl + wheel" toggle drops BOTH
#     chords (the rows diverge and grow a "Reset") and puts them back,
#   * a zoom leaves the Terminal Font Size stepper on its preference and
#     says so under it, and a restart on the same vault (`reset`) comes
#     back at the preference with nothing to say.
#
# The zoom is run from the command palette: the harness grammar holds no
# modifier across a scroll and types no `=`, so the wheel gesture itself
# (a real Ctrl+wheel, the Windows touchpad pinch) is owner QA. `expect`
# is exact-match, which is what makes "Reset" a usable signal (the only
# other Reset-ish labels on the Shortcuts screen never match it).
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
settle
# Toolbar gear (icon only, no text selector).
click (1175, 64)
settle
click "Shortcuts"
settle
expect "Wheel up"
expect "Wheel down"

click "Terminal Settings"
settle
click (95, 68)
type ctrl+a
type "wheel"
settle
expect "Zoom with Ctrl + wheel"
# Enter reveals the row (scrolls it under the search box).
type enter
settle 500
# Off: both chords leave the binding table.
click (1142, 70)
settle
click "Shortcuts"
settle
absent "Wheel up"
absent "Wheel down"
expect "Reset"
screenshot wheel-zoom-off

# On again: back to the exact factory lists, chips included.
click "Terminal Settings"
settle
click (95, 68)
type ctrl+a
type "wheel"
settle
type enter
settle 500
click (1142, 70)
settle
click "Shortcuts"
settle
expect "Wheel up"
expect "Wheel down"

# A zoom step moves the session, not the preference.
type ctrl+shift+p
settle 400
type "zoom font in"
settle 400
expect "Zoom font in"
type enter
settle 400
click "Terminal Settings"
settle
expect "Terminal Font Size"
expect "Zoomed to 15 for this session"

screenshot wheel-zoom-session

# Same vault, fresh process: the preference is what comes back.
reset
settle
expect "Create host"
click (1175, 64)
settle
click "Terminal Settings"
settle
expect "Terminal Font Size"
absent "Zoomed to 15 for this session"
