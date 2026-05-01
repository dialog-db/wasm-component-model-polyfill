# Project Design Documents

This directory holds the project's Project Design Documents (PDDs): a
chronological record of the design decisions that shaped the project.

## Purpose

A PDD captures *what* is being designed and *why*, before implementation
begins. It is the durable artifact that engineering plans and code
changes refer back to.

## Structure

Every PDD must include the following sections, in order:

1. **Introduction** — A concise summary of the design.
2. **Goals** — What this design must achieve. One bullet per goal;
   reserve elaboration for the body.
3. **Non-goals** — What this design explicitly does not cover,
   including anything deferred to a future PDD.
4. **Body** — The substance of the design. Express featureful
   improvements as user stories, and prefer to center human persons
   over software, programs, or synthesized intelligences whenever
   possible.
5. **References** — External links relevant to or cited by the
   document. Always link to the most direct location of the referenced
   content.

## Guidelines

**Do:**

- Match the format, structure, and idioms of previously accepted PDDs.
- Use pseudocode and non-specific suggestions where they aid
  understanding.
- Resolve every open question before the document is finalized.

**Do not:**

- Prescribe concrete implementations or project directory structures —
  those are decided during engineering planning after the design is
  accepted.
- Reference future PDDs or anticipate their contents. If something is
  out of scope, list it under Non-goals.
- Reference project implementation status or ongoing work streams.
- Create sub-headings just to re-state or reference something from a
  previous PDD (a concise backlink and caption are just fine)
