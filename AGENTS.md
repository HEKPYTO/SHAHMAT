# AGENTS.md — SHAHMAT

Scope: every change in this repo. Source of truth for how to work; `docs/SPEC.md` is source of truth for what to build, `docs/PLAN.md` for sequence and gates.

## Engineering principles

- Cut obsolete paths outright: no compatibility layers, fallbacks, or migrations. Delete; migrate every caller in the same change.
- Simplest implementation that fully meets the current requirement. No speculative abstraction, configuration, or indirection.
- Layers: smallest end-to-end working product first, each capability on top of a working product. Never trade working for unfinished complexity.
- Modular components, separated concerns; one concept's definition, rules, and caveats under one heading.
- Established, well-maintained libraries over reimplementation when they cut total complexity or raise reliability.
- Check the current dependency tree (manifest + docs + types) before writing your own implementation or adding a package. No capability claim without a doc/type lookup.
- Long-term architecture only. No stopgaps meant to be replaced later.

## Commits

- Concise commit messages, no extra commentary.
- Commit at phase/task completion or at crucial moments only. No small-change, experimental-design, or pre-breakage checkpoint commits.
- Attribute to the existing git user/email. Never invent contributor identities.

## Privacy — private repo

- Local-only by default: no `push`, no `--push`, no registry publish (`cargo publish`, image push), no remote mention (URLs, `ghcr.io/<org>`, remotes) unless the user explicitly asks in this conversation.
- `docs/PLAN.md` push commands are gated triggers for a future release step, not standing permission.
- No git init / remote add / network publish as a side effect of a build, scaffold, or verification step.

## Docs — README per subdirectory

- Code-adjacent docs live as `README.md` beside the code they describe (root, each `src/` module dir, `examples/`, `docs/`, `outputs/`). Update the affected `README.md` on every change that touches its directory.
- No new central docs folder or top-level markdown beyond what exists (`docs/SPEC.md`, `docs/PLAN.md`, per-dir `README.md`) unless explicitly requested.
- Only `README.md`, `LICENSE`, and in advanced cases `CHANGELOG.md` may be committed as markdown files. No other `.md` files.
- Never mention generation tooling or provenance in docs or replies.

## Skill policy

When skills are available, work in Poteto Mode (`skill://Poteto Mode`, trigger `/poteto-mode`). Its non-negotiables apply to every multi-step task in this repo.

- Start each multi-step task with a todolist whose first item is reading the Poteto Mode Principles section in full. Name each principle that shaped a decision and the choice it changed.
- Route multi-step feature work through `sw-development-loop` when available, with `superpowers` and `ponytail` applied throughout the loop.
- Apply the `unslop` skill to every prose surface, including replies.
- For frontend work, also apply `design-taste-frontend` with `ui-ux-pro-max` with `impeccable` when available.
- Spawn subagents as `poteto-agent` inside playbook steps. Match each task to a Poteto Mode playbook and copy its steps verbatim into the todolist before task-specific todos.
