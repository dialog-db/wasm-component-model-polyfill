---
id: 6ffdf7
title: Adopt lib.markdown for the design corpus
type: chore
blocked_by: [c3b98b]
labels: [PDD001, katsuobushi]
created: 2026-09-14T19:34:54Z
---

## What to build
Replace the inline rumdl configuration and the `format:design` command with `katsuobushi.lib.markdown` scoped to the design corpus and the repository README. Reformat the corpus once with Prettier and make sure the plain-English rules in the corpus README still hold after reformatting. Feed `project.markdownExclude` into the exclude list when the board lands.

## Acceptance criteria
- [ ] `markdown format` and `markdown lint` appear in the menu and operate on the design corpus.
- [ ] The `markdown` flake check passes on a clean tree.
- [ ] rumdl and its inline configuration are gone from `flake.nix`.
- [ ] The corpus README's writing rules are unchanged in substance.

