# Repository Agent Assets

This subtree contains tracked commands, snippets, and skills used while
working on OpenAgentd. It does not contain the application's user-owned runtime
configuration.

## Layout

- `commands/*.md`: slash commands (`$ARGUMENTS` is the user's argument).
- `snippets/**/*.md`: one-line shell snippets.
- `skills/oad/<name>/SKILL.md`: repository workflows — `commit`, `debug`
  (including logs and telemetry), `release`, `review`, `testing` (including
  test-first). `skills/guidelines/` holds general coding behavior.

## File contracts

- Command and snippet Markdown files require YAML frontmatter with a
  `description` field.
- Every skill entry point is a `SKILL.md` with `name` (matching its path, e.g.
  `oad/testing`) and `description` frontmatter. Keep detailed supporting
  material in that skill's `reference/` or `scripts/` directory, and keep
  commands and paths in skills true to the Makefiles and scripts they name.
- Commands delegate to skills rather than restating them.
- Keep commands non-destructive by default. Never embed credentials,
  machine-local paths, or copied `.env` values.
- Do not edit ignored `.openagentd/dev/`, `data/`, `state/`, `sessions/`, or
  other runtime output as repository source.

## Checks

```bash
make verify-docs
```
