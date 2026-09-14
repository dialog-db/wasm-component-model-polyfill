# Project Design Documents

This directory holds the Project Design Documents (PDDs) of the polyfill. A
PDD is a record of one design decision. The documents are numbered in the
order they were accepted.

## Purpose

A PDD records what is designed and why, before the implementation starts. It
is the document that engineering plans and code changes refer back to.

## Structure

Every PDD has these sections, in this order:

1. Introduction. A short summary of the design.
2. Goals. What the design must achieve. Use one bullet per goal. Put the
   detail in the body.
3. Non-goals. What the design does not cover. List here anything that is out
   of scope.
4. Body. The substance of the design. Write features as user stories. Center
   the stories on people, not on software.
5. References. External links that the document cites. Link to the most
   direct location of the referenced content.

## Writing Rules

Write every PDD in plain English:

- Use short sentences. Keep descriptive sentences under 25 words and
  instructions under 20 words.
- Use the active voice and simple tenses. Name the actor.
- Use the modal verbs "can", "will", and "must". Do not use "should", "may",
  "might", "could", or "would".
- Do not use semicolons, em-dashes, or contractions.
- Use one word for one meaning through the whole corpus. For example, use
  "runtime layer" for the `wasm_runtime_layer` crate and "backend" for one of
  its implementations.
- Define a concept term at its first use in a document.
- State facts. Delete words that carry no fact.

## Content Rules

Do:

- Match the format, structure, and idioms of the accepted PDDs.
- Use pseudocode and non-specific suggestions where they help the reader.
- Resolve every open question before the document is accepted.
- Write a revision so that it reads as the intended design. Do not narrate
  the change.

Do not:

- Prescribe concrete implementations or directory structures. Engineering
  planning decides those after the design is accepted.
- Reference PDDs that do not exist yet, or anticipate their contents. If
  something is out of scope, list it under Non-goals.
- Reference the implementation status of the project or work in progress.
- Create a sub-heading only to restate a previous PDD. A short backlink is
  enough.
