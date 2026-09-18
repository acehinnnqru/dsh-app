# AGENTS.md

## Communication (most important)

The user has ADHD. **Default to minimal. Expand only on request.**

- **Lead with the conclusion.** First sentence = the answer or verdict. No preamble, no recap, no restating the question.
- **Be short.** Default ≤ 5 bullets or 1 small table. One line per point; if one sentence does it, don't write three.
- **Ask one question at a time**, and always include the recommended option.
- **Don't repeat what was already said.** No "to summarize".
- **Split long content out.** For anything long (design docs, full analysis), write a file and give 3–5 lines in chat plus the link.
- **When you need a decision, list only the decision points**: one line each = question + recommendation + cost.
- **Never** open with "great question" or "let me explain in detail".
- Reply in Chinese (unless the user switches language).

## Project notes

- Run `nix develop --command cargo build` before changing anything. **The baseline is green**; any error is newly introduced.
- **Dev builds probe PATH for node.** Verifying the "unconfigured" flow requires `cargo build --release`.
- The `slim` variant never guesses Node paths: no PATH scan, no standard install locations (dev builds only are the exception).
- For long write-ups/analyses, put them in `docs/`, not in chat.
