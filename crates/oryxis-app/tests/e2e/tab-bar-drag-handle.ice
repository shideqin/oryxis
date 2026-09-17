viewport: 1115x600
mode: Zen
-----
# Issue #226: with enough tabs the strip's scrollable fills its slot
# edge to edge, the `+` docks, and every remaining pixel of the bar is a
# button, so the window could not be moved at all. The bar now reserves
# a 40 px drag handle between the docked `+` and the `⋯`.
#
# The harness has no window to drag, so the handle is proven by what
# the pixel is NOT and by the gesture it shares with the strip's empty
# area. Before the fix the docked `+` sat there, and merely moving the
# cursor onto it opens its hover popover (New Tab / split rows), so the
# first assertion is that hovering the handle opens nothing. Then a
# right-click there opens the strip menu (New Tab, nothing destructive),
# the way the strip's slack answers it. The viewport is the reporter's
# width.
settle 250
click "Skip"
settle 250
click "Continue without password"
settle 250
# A live PTY never lets the emulator quiesce, so cap the per-instruction
# wait instead of burning the full timeout on every line.
timeout 500
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
type ctrl+shift+l
settle 250
# Eight tabs at 1115 px overflow the strip: the `⋯` is up and the `+`
# is docked at the strip's trailing edge.
screenshot tab-bar-drag-handle
# The handle: right of the docked `+`, left of the `⋯` (1115 minus the
# chrome, the side-panel toggle, the `⋯` and the 40 px handle itself
# puts it at x 841..881).
move (861.00, 18.00)
settle 250
absent "New Tab"
click right (861.00, 18.00)
settle 250
expect "New Tab"
absent "Close Tab"
type escape
settle 250
absent "New Tab"
