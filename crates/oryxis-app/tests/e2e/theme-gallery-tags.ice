viewport: 1240x2000
mode: Zen
-----
# The terminal-theme gallery filters by MEASURED tags (issue #230): the
# Dark / Light chips gate on tone, and the typed line matches a tag's
# label or English keyword as well as the theme name, so "warm" finds
# Gruvbox and "light" finds every light palette.
click "Skip"
click "Continue without password"
settle
# Settings (gear) -> Terminal Settings -> the active-theme card opens the gallery.
click (1215, 64)
settle
click "Terminal Settings"
settle
click "Follow app theme (Oryxis Dark)"
settle
expect "Dracula"
expect "Solarized Light"
# Light chip: only the light palettes stay.
click "Light"
settle
absent "Dracula"
expect "Solarized Light"
expect "Paper Light"
# Dark chip: the reverse.
click "Dark"
settle
expect "Dracula"
absent "Solarized Light"
# All again, then a typed tag: "warm" is Gruvbox's measured trait, not
# part of its name.
click "All"
settle
expect "Solarized Light"
click "Filter…"
type "warm"
settle
expect "Gruvbox Dark"
expect "Gruvbox Light"
absent "Nord"
absent "Dracula"
