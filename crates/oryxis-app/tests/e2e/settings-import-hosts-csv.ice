viewport: 1200x750
mode: Zen
-----
# Settings > Security & Privacy carries the CSV import beside the CSV
# export it reads back (the hub on the dashboard is the other door).
# The picker itself is a native dialog the harness cannot drive; the
# reader and planner are covered by importers::csv_plan_tests.
click "Skip"
click "Continue without password"
expect "Create host"
click (1175, 64)
expect "Security & Privacy"
click "Security & Privacy"
settle 400
expect "Export hosts (CSV)"
expect "Import hosts (CSV)"
expect "Import Vault"
