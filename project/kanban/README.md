# Project board

An Obsidian-Kanban-native backlog managed by the `project` command. **`BOARD.md`
is the source of truth for lifecycle and priority** — its lanes are the status
and a card's vertical position is its priority. Card notes live in `issues/` and
hold everything else (title, type, `blocked_by`, detail).

- **See the board:** `project status` (all cards) or `project status <id>`
  (one).
- **Move a card:** `project status set <id> <status>` — enforces the state
  machine. Or drag it in Obsidian (unconstrained by the tool).
- **Reprioritize:** `project prioritize <id> --top|--before <id>|--after <id>`,
  or drag within a lane.
- **New card:** `project new --title "..."` (mints the note + a card in To-do).
- **Check health:** `project lint` (board <-> note consistency).

## Lifecycle

```
To-do -> In Progress -> Needs Review -> Ready -> Accepted
```

`needs-review -> in-progress` is a reviewer bounce; `ready -> todo` is an owner
return. `cancelled` is reachable from any non-accepted state. Accepted and
Cancelled cards move to the Kanban **archive** with the outcome recorded on the
note as `disposition:`.

**Only a human (the product owner) moves a card to `accepted`.**

Do not hand-edit `BOARD.md`'s structure expecting the tool to reconcile it; edit
via the CLI or drag in Obsidian.
