<!--
Thanks for the PR. Keep it focused: unrelated refactors belong in their
own PR. CONTRIBUTING.md has the full conventions.
-->

## What and why

<!-- What changed, and the problem it solves. -->

Closes #

## Screenshots

<!-- UI changes: a screenshot or a short capture, before/after if it helps.
     Delete this section otherwise. -->

## Checklist

- [ ] `cargo check --workspace`, `cargo test --workspace --lib --bins` and
      `cargo clippy --workspace --all-targets -- -D warnings` pass locally
- [ ] `cargo test --locked --workspace` passes if a `Cargo.toml` or
      `Cargo.lock` changed
- [ ] New user-facing strings go through `i18n::t("key")`, with the key
      added to all 23 language modules
- [ ] New interactive surfaces (views, modals, menus, row lists) are wired
      into the keynav framework
- [ ] New settings rows have an entry in `settings_index.rs`
- [ ] Tests land in this PR where they apply; no `// TODO` gaps or
      placeholder UI
- [ ] Only the files I touched are formatted (no `cargo fmt --all`)
- [ ] Code comments are in English
