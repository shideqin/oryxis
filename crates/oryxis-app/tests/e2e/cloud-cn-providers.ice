viewport: 1400x2400
mode: Zen
-----
# J3: Alibaba Cloud and Tencent Cloud are cloud providers like the other
# four. The Plugins screen is where a provider first exists to the
# user, so both rows have to be there; the wizard behind them is only
# reachable with a provider plugin installed, which the wiped sandbox
# (and CI's `-p oryxis-app` build) never has. Runtime QA against real
# accounts is an owner pass with the `aliyun` / `tccli` CLIs configured.
#
# Tall viewport on purpose: eight plugin rows sit below the fold of the
# default window and the emulator's wheel does not reach the settings
# scrollable, so a plain `expect` would miss a row that is there.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
settle
click (19, 20)
settle
click "Settings"
settle
click "Features & Plugins"
settle
expect "Amazon Web Services"
expect "Microsoft Azure"
expect "Alibaba Cloud"
expect "Tencent Cloud"
